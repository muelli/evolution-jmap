// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reference count and leak verification across libical-glib boundaries.

use std::ffi::CString;
use std::sync::{Mutex, MutexGuard, Once};

use eds_sys::i_cal_component_new_from_string;
use glib_sys::{G_LOG_LEVEL_CRITICAL, G_LOG_LEVEL_WARNING, g_log_set_always_fatal};
use gobject_sys::{GObject, g_object_unref};
use jmap_backend_cal::marshal;

/// Serialize access to the libical builtin timezone table.
///
/// `i_cal_timezone_get_builtin_timezone` reads a global table without locks.
/// Concurrent access causes memory corruption in `strcmp`.
/// Hold this lock during every such call.
fn serialize_libical_timezone_access() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Event fixture in a known timezone.
///
/// The parser inspects properties for `TZID` parameters.
/// It resolves the zone and adds its definition to the envelope.
const ZONED: &str = "BEGIN:VCALENDAR\r\n\
                     VERSION:2.0\r\n\
                     BEGIN:VEVENT\r\n\
                     UID:K1\r\n\
                     SUMMARY:Standup\r\n\
                     DTSTART;TZID=Europe/Zurich:20260810T090000\r\n\
                     DTEND;TZID=Europe/Zurich:20260810T093000\r\n\
                     END:VEVENT\r\n\
                     END:VCALENDAR\r\n";

/// Event fixture with UTC time.
///
/// The parser finds no local zone to define in the envelope.
const UNZONED: &str = "BEGIN:VCALENDAR\r\n\
                       VERSION:2.0\r\n\
                       BEGIN:VEVENT\r\n\
                       UID:K1\r\n\
                       SUMMARY:Standup\r\n\
                       DTSTART:20260810T070000Z\r\n\
                       END:VEVENT\r\n\
                       END:VCALENDAR\r\n";

/// Configure GLib critical and warning logs to abort the test process.
fn glib_complaints_are_failures() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: no preconditions; the call returns the previous mask.
        unsafe { g_log_set_always_fatal(G_LOG_LEVEL_CRITICAL | G_LOG_LEVEL_WARNING) };
    });
}

/// Return the strong reference count of a GObject-derived instance.
///
/// # Safety
///
/// `instance` must point to a live GObject instance.
unsafe fn strong_count<T>(instance: *mut T) -> u32 {
    // SAFETY: caller guarantees a live instance of a GObject type.
    unsafe { (*instance.cast::<GObject>()).ref_count }
}

/// Return baseline reference count from libical parsing before traversal.
fn baseline_count(icalendar: &str) -> u32 {
    let text = CString::new(icalendar).expect("fixture has no interior NUL");
    // SAFETY: `text` is a valid C string; the code releases the reference below.
    unsafe {
        let component = i_cal_component_new_from_string(text.as_ptr());
        assert!(!component.is_null(), "libical would not parse the fixture");
        let count = strong_count(component);
        g_object_unref(component.cast());
        count
    }
}

#[test]
fn a_loaded_component_carries_exactly_the_callers_reference() {
    glib_complaints_are_failures();
    let _lock = serialize_libical_timezone_access();
    for (name, icalendar) in [("zoned", ZONED), ("unzoned", UNZONED)] {
        let baseline = baseline_count(icalendar);
        let component = marshal::component_from_ical(icalendar);
        assert!(
            !component.is_null(),
            "{name}: not read as a calendar object"
        );
        // Verify component reference count matches baseline after traversal.
        // SAFETY: component is live and verified non-null.
        assert_eq!(
            unsafe { strong_count(component) },
            baseline,
            "{name}: the traversal changed the component's own reference count"
        );
        // SAFETY: releases the reference from `component_from_ical`.
        unsafe { marshal::component_unref(component) };
    }
}

#[test]
fn the_zone_a_load_defines_is_owned_by_the_object_that_defines_it() {
    glib_complaints_are_failures();
    let _lock = serialize_libical_timezone_access();
    let component = marshal::component_from_ical(ZONED);
    assert!(!component.is_null());
    // SAFETY: component is a live instance owned by this scope.
    let rendered =
        unsafe { marshal::ical_from_component(component) }.expect("a parsed component renders");
    assert!(
        rendered.contains("BEGIN:VTIMEZONE"),
        "the zone was never defined, so this test is not measuring the clone \
         handoff it is about: {rendered}"
    );
    // Verify envelope owns cloned timezone component.
    // SAFETY: releases the reference from `component_from_ical`.
    unsafe { marshal::component_unref(component) };
}

#[test]
fn loading_a_zoned_object_over_and_over_retains_nothing() {
    glib_complaints_are_failures();
    let _lock = serialize_libical_timezone_access();
    // Verify that loading zoned objects repeatedly does not leak heap memory.
    const LOADS: u64 = 10_000;
    const WINDOWS: usize = 8;
    const NOISE_FLOOR: u64 = 32 * 1024;
    let mut retained = Vec::with_capacity(WINDOWS);
    for _ in 0..WINDOWS {
        let before = allocated_bytes();
        for _ in 0..LOADS {
            load_and_render();
        }
        let growth = allocated_bytes().saturating_sub(before);
        if growth <= NOISE_FLOOR {
            return;
        }
        retained.push(growth);
    }
    panic!(
        "no window of {LOADS} loads retained less than the {NOISE_FLOOR}-byte noise floor — \
         bytes retained per window: {retained:?}. A finite cache settles under the floor; \
         a reference kept on every load (>= ~320 kB/window) never can."
    );
}

/// Execute one round trip of timezone load and render paths.
///
/// The run exercises `component_from_ical` and `icalendar_with_time_zones`.
fn load_and_render() {
    // SAFETY: releases the reference from `component_from_ical` immediately.
    unsafe { marshal::component_unref(marshal::component_from_ical(ZONED)) };
    marshal::icalendar_with_time_zones(ZONED);
}

/// Return total allocated heap bytes via mallinfo2.
fn allocated_bytes() -> u64 {
    // SAFETY: no preconditions; libc call returns counters.
    unsafe { libc::mallinfo2() }.uordblks as u64
}
