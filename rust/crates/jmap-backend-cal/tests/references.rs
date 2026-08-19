// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the marshalling leaves behind, counted rather than reasoned about.
//!
//! `tests/marshal.rs` asks what the boundary *produces*; this asks what it
//! costs. Every function in `marshal.rs` that walks an object takes owned
//! references to components, properties and parameters on the way through
//! (libical-glib is a GObject binding, so each getter hands back a reference),
//! and a wrong number of them is invisible to a test that only reads the text
//! that came out: one too few is a use-after-free in
//! `evolution-calendar-factory`, one too many is a leak that grows with every
//! sync. `docs/UNSAFE-AUDIT.md` Pattern C is about exactly that, and
//! `jmap_backend_core::owned::Owned` is the answer to it — these are the tests
//! that the answer holds on real libical objects, rather than only on the plain
//! `GObject` the wrapper's own tests use.

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

/// Makes GLib's own complaints fail the test run.
///
/// This is what actually catches an over-released reference: `g_object_unref`
/// on a pointer whose count already reached zero answers with a
/// `GLib-GObject-CRITICAL`, and by default a critical is printed and execution
/// continues — so a test that only reads the rendered text passes while stderr
/// fills with them, which is exactly what happened when this file was first
/// written and the wrapper deliberately broken to check the tests bite. With the
/// mask set, the same critical aborts the binary and the run is red.
///
/// Process-wide and set once, since every test in this binary wants it.
fn glib_complaints_are_failures() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: no preconditions; the previous mask is returned and ignored.
        unsafe { g_log_set_always_fatal(G_LOG_LEVEL_CRITICAL | G_LOG_LEVEL_WARNING) };
    });
}

/// The strong reference count of a GObject-derived instance.
///
/// Every libical-glib type derives from `GObject` through `ICalObject`, so an
/// instance pointer is a `GObject *` and `ref_count` is the first field after
/// the type instance — the same read
/// `jmap-backend-core/tests/owned.rs` makes on a plain object.
///
/// # Safety
///
/// `instance` must be a live GObject-derived instance.
unsafe fn strong_count<T>(instance: *mut T) -> u32 {
    // SAFETY: the caller guarantees a live instance of a GObject-derived type.
    unsafe { (*instance.cast::<GObject>()).ref_count }
}

/// What libical's own parse hands back, before any of this crate's traversal has
/// touched the object. The number itself is libical's business; that
/// [`marshal::component_from_ical`] agrees with it is this crate's.
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
        // The traversal takes references to the component's events, their
        // properties and their `TZID` parameters, and — for the zoned fixture —
        // to a builtin zone's definition and a clone of it. None of those is a
        // reference to *this* object, so the count the caller is handed has to
        // be the one libical's own parse gives. A reference kept by mistake
        // would read one higher here and the component would never be freed;
        // `load_component_sync` hands this straight to EDS, which unrefs once.
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
    // The clone put into the envelope was handed over with `into_raw`, so the
    // envelope owns it: rendering the component after the fact reaches it, which
    // it could not do if the clone's reference had been dropped as well as
    // taken. Read back through the text rather than through a second reference,
    // because taking one would itself change what is being measured.
    // SAFETY: the reference `component_from_ical` handed over.
    unsafe { marshal::component_unref(component) };
}

#[test]
fn loading_a_zoned_object_over_and_over_retains_nothing() {
    glib_complaints_are_failures();
    // A reference released once too often shows up as a GLib critical — fatal
    // here, see above — within a few iterations. One *kept* too often shows up
    // as allocation that never comes back, which is what this measures and what
    // no assertion about the rendered text can see.
    //
    // The criterion is a window of loads that retains *exactly nothing*, not a
    // byte threshold. Thresholds are the wrong instrument here twice over: too
    // tight and libical's own one-time caching fails the test, too loose and a
    // small leak passes it (measured: a per-load leak of one wrapper is 192
    // bytes, which an RSS-based first draft of this test did not notice at all).
    // A cache is finite, so the growth per window falls to zero and stays there;
    // a leak is per-load, so no window can ever reach zero. Measured on this
    // code: ~350 kB retained by the first window of 10,000, single-digit kB by
    // the second, and exactly 0 by the third or fourth — hence a sequence of
    // windows rather than one warmup and one measurement, and a window count
    // with room to spare over what convergence has been seen to need.
    const LOADS: u64 = 10_000;
    const WINDOWS: usize = 8;
    let mut retained = Vec::with_capacity(WINDOWS);
    for _ in 0..WINDOWS {
        let before = allocated_bytes();
        for _ in 0..LOADS {
            load_and_render();
        }
        let growth = allocated_bytes().saturating_sub(before);
        if growth == 0 {
            return;
        }
        retained.push(growth);
    }
    panic!(
        "no window of {LOADS} loads retained nothing — bytes retained per window: {retained:?}. \
         A finite cache settles; a reference kept on every load does not."
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

/// Bytes this process has allocated from the heap and not yet returned.
///
/// `mallinfo2`'s `uordblks` rather than the resident set, because the resident
/// set is measured in pages and an allocator that never returns them: a leaked
/// libical wrapper is a couple of hundred bytes, so ten thousand of them are
/// well inside the noise of an RSS reading (measured: they were — an RSS-based
/// version of this test passed with the wrapper deliberately leaking) and
/// exactly visible here.
fn allocated_bytes() -> u64 {
    // SAFETY: no preconditions; returns a plain struct of counters.
    unsafe { libc::mallinfo2() }.uordblks as u64
}
