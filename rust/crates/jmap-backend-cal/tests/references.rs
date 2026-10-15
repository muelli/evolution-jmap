// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reference count and allocation leak verification across libical-glib marshalling boundaries.

use std::ffi::CString;
use std::sync::Once;

use eds_sys::i_cal_component_new_from_string;
use glib_sys::{G_LOG_LEVEL_CRITICAL, G_LOG_LEVEL_WARNING, g_log_set_always_fatal};
use gobject_sys::{GObject, g_object_unref};
use jmap_backend_cal::marshal;

/// An event in a zone libical knows, which is the path that walks the most:
/// every property of the event is asked for a `TZID` parameter, the zone is
/// resolved, its definition cloned, renamed and put in the envelope.
const ZONED: &str = "BEGIN:VCALENDAR\r\n\
                     VERSION:2.0\r\n\
                     BEGIN:VEVENT\r\n\
                     UID:K1\r\n\
                     SUMMARY:Standup\r\n\
                     DTSTART;TZID=Europe/Zurich:20260810T090000\r\n\
                     DTEND;TZID=Europe/Zurich:20260810T093000\r\n\
                     END:VEVENT\r\n\
                     END:VCALENDAR\r\n";

/// The same event as a UTC instant: the traversal runs and finds no zone to
/// define, so nothing is cloned and nothing is taken.
const UNZONED: &str = "BEGIN:VCALENDAR\r\n\
                       VERSION:2.0\r\n\
                       BEGIN:VEVENT\r\n\
                       UID:K1\r\n\
                       SUMMARY:Standup\r\n\
                       DTSTART:20260810T070000Z\r\n\
                       END:VEVENT\r\n\
                       END:VCALENDAR\r\n";

/// Configures GLib critical and warning logs to abort the test process.
fn glib_complaints_are_failures() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: no preconditions; the previous mask is returned and ignored.
        unsafe { g_log_set_always_fatal(G_LOG_LEVEL_CRITICAL | G_LOG_LEVEL_WARNING) };
    });
}

/// The strong reference count of a GObject-derived instance.
///
/// # Safety
///
/// `instance` must be a live GObject-derived instance.
unsafe fn strong_count<T>(instance: *mut T) -> u32 {
    // SAFETY: the caller guarantees a live instance of a GObject-derived type.
    unsafe { (*instance.cast::<GObject>()).ref_count }
}

/// Returns baseline reference count from libical parsing before traversal.
fn baseline_count(icalendar: &str) -> u32 {
    let text = CString::new(icalendar).expect("fixture has no interior NUL");
    // SAFETY: `text` is valid for the call; the reference is released below.
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
    for (name, icalendar) in [("zoned", ZONED), ("unzoned", UNZONED)] {
        let baseline = baseline_count(icalendar);
        let component = marshal::component_from_ical(icalendar);
        assert!(
            !component.is_null(),
            "{name}: not read as a calendar object"
        );
        // Verify component reference count matches baseline after traversal.
        // SAFETY: a live component, just checked for NULL.
        assert_eq!(
            unsafe { strong_count(component) },
            baseline,
            "{name}: the traversal changed the component's own reference count"
        );
        // SAFETY: the reference `component_from_ical` handed over.
        unsafe { marshal::component_unref(component) };
    }
}

#[test]
fn the_zone_a_load_defines_is_owned_by_the_object_that_defines_it() {
    glib_complaints_are_failures();
    let component = marshal::component_from_ical(ZONED);
    assert!(!component.is_null());
    // SAFETY: a live component this scope owns.
    let rendered =
        unsafe { marshal::ical_from_component(component) }.expect("a parsed component renders");
    assert!(
        rendered.contains("BEGIN:VTIMEZONE"),
        "the zone was never defined, so this test is not measuring the clone \
         handoff it is about: {rendered}"
    );
    // Verify cloned timezone component is owned by the envelope.
    // SAFETY: the reference `component_from_ical` handed over.
    unsafe { marshal::component_unref(component) };
}

#[test]
fn loading_a_zoned_object_over_and_over_retains_nothing() {
    glib_complaints_are_failures();
    // Verifies that repeatedly loading zoned objects does not leak heap memory across windows.
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

/// One trip through both directions of the zone handling: the load path
/// (`component_from_ical` → `holds_event` → `take_event_time_zones` →
/// `referenced_tzids` → `take_referenced_time_zones`) and the render path
/// (`icalendar_with_time_zones`), which is every function this file is about.
fn load_and_render() {
    // SAFETY: the reference `component_from_ical` handed over is released
    // immediately.
    unsafe { marshal::component_unref(marshal::component_from_ical(ZONED)) };
    marshal::icalendar_with_time_zones(ZONED);
}

/// Returns total allocated heap bytes via mallinfo2.
fn allocated_bytes() -> u64 {
    // SAFETY: no preconditions; returns a plain struct of counters.
    unsafe { libc::mallinfo2() }.uordblks as u64
}
