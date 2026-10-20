// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Who tells the guests, from the backend's side (`docs/MEETING-SCHEDULING.md`
//! section 4): the account switch read from the calendar's `ESource`, the
//! capabilities that tell Evolution not to mail the guests itself, and the
//! per-change flag that says the user chose not to send.

use std::ffi::CString;
use std::ptr;
use std::sync::Once;

use eds_sys::{
    E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER, E_CAL_OPERATION_FLAG_CONFLICT_USE_NEWER,
    E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE, E_CAL_OPERATION_FLAG_NONE,
    E_SOURCE_EXTENSION_WEBDAV_BACKEND, ECalBackendClass, ESource, e_source_get_extension,
    e_source_get_type, e_source_has_extension, e_source_new_with_uid, e_source_webdav_get_type,
    e_source_webdav_set_calendar_auto_schedule,
};
use glib_sys::{GFALSE, GTRUE};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_backend_cal::backend::{JmapCalBackend, JmapCalBackendClass, parent_class};
use jmap_backend_cal::scheduling::{
    CREATE_MESSAGES, ITIP_SUPPRESS_ON_REMOVE_SUPPORTED, SAVE_SCHEDULES, capabilities, dismissal,
    itip, server_handles_invitations,
};
use jmap_backend_core::subclass::register_static;
use jmap_cal_sync::Itip;

const SCHEDULING_CAPS: [&str; 3] = [
    CREATE_MESSAGES,
    SAVE_SCHEDULES,
    ITIP_SUPPRESS_ON_REMOVE_SUPPORTED,
];

fn split(caps: &str) -> Vec<&str> {
    caps.split(',').filter(|cap| !cap.is_empty()).collect()
}

// ---------------------------------------------------------------------------
// the capability string

#[test]
fn the_capability_names_are_the_ones_e_cal_util_h_defines() {
    // `E_CAL_STATIC_CAPABILITY_*` are `#define`d strings, so bindgen has
    // nothing to hand back and a typo here would be a capability Evolution
    // never matches, and every guest mailed twice.
    assert_eq!(CREATE_MESSAGES, "create-messages");
    assert_eq!(SAVE_SCHEDULES, "save-schedules");
    assert_eq!(
        ITIP_SUPPRESS_ON_REMOVE_SUPPORTED,
        "itip-suppress-on-remove-supported"
    );
}

#[test]
fn with_the_switch_on_the_inherited_capabilities_gain_the_three_scheduling_ones() {
    let caps = capabilities(Some("refresh-supported,alarm-description"), true);
    assert_eq!(
        split(&caps),
        [
            "refresh-supported",
            "alarm-description",
            CREATE_MESSAGES,
            SAVE_SCHEDULES,
            ITIP_SUPPRESS_ON_REMOVE_SUPPORTED,
        ]
    );
}

#[test]
fn with_the_switch_off_the_inherited_capabilities_are_answered_unchanged() {
    assert_eq!(
        capabilities(Some("refresh-supported,alarm-description"), false),
        "refresh-supported,alarm-description"
    );
}

#[test]
fn with_nothing_inherited_the_answer_has_no_empty_entry() {
    assert_eq!(split(&capabilities(None, true)), SCHEDULING_CAPS);
    assert_eq!(capabilities(Some(""), true), capabilities(None, true));
    assert_eq!(capabilities(None, false), "");
}

#[test]
fn a_scheduling_capability_already_inherited_is_not_listed_twice() {
    let caps = capabilities(Some("create-messages,refresh-supported"), true);
    assert_eq!(
        split(&caps)
            .iter()
            .filter(|cap| **cap == CREATE_MESSAGES)
            .count(),
        1
    );
    assert!(split(&caps).contains(&"refresh-supported"));
}

// ---------------------------------------------------------------------------
// the per-change flag

#[test]
fn a_change_with_no_flags_lets_the_server_send() {
    assert_eq!(itip(true, E_CAL_OPERATION_FLAG_NONE), Itip::Send);
}

#[test]
fn disable_itip_message_suppresses_this_change() {
    assert_eq!(
        itip(true, E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE),
        Itip::Suppress
    );
}

#[test]
fn disable_itip_message_is_read_as_one_bit_among_the_conflict_flags() {
    assert_eq!(
        itip(
            true,
            E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE | E_CAL_OPERATION_FLAG_CONFLICT_USE_NEWER
        ),
        Itip::Suppress
    );
    assert_eq!(
        itip(true, E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER),
        Itip::Send
    );
}

#[test]
fn with_the_switch_off_no_change_asks_the_server() {
    // Evolution mails the guests itself then, and asking the server as well
    // would invite every one of them twice.
    assert_eq!(itip(false, E_CAL_OPERATION_FLAG_NONE), Itip::Suppress);
    assert_eq!(
        itip(false, E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE),
        Itip::Suppress
    );
}

#[test]
fn a_reminder_dismissal_never_asks_the_server_to_send() {
    // EDS's reminder watcher dismisses with no flags (e-reminder-watcher.c).
    assert_eq!(
        itip(true, dismissal(E_CAL_OPERATION_FLAG_NONE)),
        Itip::Suppress
    );
    assert_eq!(
        itip(true, dismissal(E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER)),
        Itip::Suppress
    );
    assert_eq!(
        dismissal(E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER)
            & E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER,
        E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER,
        "the other flags pass through"
    );
}

/// Registers the GTypes and builds the backend class on one thread, once.
///
/// Call this before a test touches a GType. If one thread registers `ESource`
/// while another runs a `class_init` that needs it, GLib can deadlock.
fn types_ready() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: no arguments. The class reference is kept for the process.
        unsafe {
            e_source_get_type();
            // A lookup by name cannot find the extension class until it is registered.
            e_source_webdav_get_type();
            g_type_class_ref(register_static::<JmapCalBackend>());
        }
    });
}

// ---------------------------------------------------------------------------
// the switch, read from the source

/// An `ESource` not backed by the registry, as in `tests/connect.rs`.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        types_ready();
        let uid = CString::new("jmap-cal-scheduling-test").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a NULL
        // GError out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// Writes `[WebDAV Backend] CalendarAutoSchedule`, creating the group.
    fn auto_schedule(self, on: bool) -> Self {
        // SAFETY: the source is alive, the name is a header constant, and the
        // extension it creates is owned by the source.
        unsafe {
            let webdav = e_source_get_extension(self.0, E_SOURCE_EXTENSION_WEBDAV_BACKEND.as_ptr());
            e_source_webdav_set_calendar_auto_schedule(
                webdav.cast(),
                if on { GTRUE } else { GFALSE },
            );
        }
        self
    }

    fn has_webdav(&self) -> bool {
        // SAFETY: as above.
        unsafe { e_source_has_extension(self.0, E_SOURCE_EXTENSION_WEBDAV_BACKEND.as_ptr()) != 0 }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: the reference from e_source_new_with_uid is given back once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn a_source_that_says_nothing_has_the_server_send_invitations() {
    let source = TestSource::new();
    // SAFETY: a live source.
    assert!(unsafe { server_handles_invitations(source.0) });
}

#[test]
fn reading_the_switch_does_not_add_a_webdav_group_to_the_source() {
    let source = TestSource::new();
    // SAFETY: a live source.
    unsafe { server_handles_invitations(source.0) };
    assert!(!source.has_webdav());
}

#[test]
fn calendar_auto_schedule_false_turns_the_switch_off() {
    let source = TestSource::new().auto_schedule(false);
    // SAFETY: a live source.
    assert!(!unsafe { server_handles_invitations(source.0) });
}

#[test]
fn calendar_auto_schedule_true_keeps_the_switch_on() {
    let source = TestSource::new().auto_schedule(true);
    // SAFETY: a live source.
    assert!(unsafe { server_handles_invitations(source.0) });
}

#[test]
fn without_a_source_nothing_is_claimed() {
    // A backend EDS has not finished constructing, or a detached test
    // instance: claiming the server sends would stop Evolution mailing guests
    // on the strength of nothing.
    // SAFETY: NULL is the documented "no source" answer.
    assert!(!unsafe { server_handles_invitations(ptr::null_mut()) });
}

// ---------------------------------------------------------------------------
// the vfunc

#[test]
fn class_init_installs_get_backend_property_over_the_meta_backends() {
    types_ready();
    let gtype = register_static::<JmapCalBackend>();
    // SAFETY: Referencing the registered class invokes class_init and returns the class struct.
    let class: *mut JmapCalBackendClass = unsafe { g_type_class_ref(gtype) }.cast();
    // SAFETY: the class is referenced and leads with the parent's.
    let ours: &ECalBackendClass = unsafe { &(*class).parent_class.parent_class.parent_class };
    let theirs = &parent_class()
        .expect("the parent class is registered")
        .parent_class
        .parent_class;

    let ours = ours.impl_get_backend_property.expect("installed");
    let theirs = theirs
        .impl_get_backend_property
        .expect("the meta backend answers capabilities, so there is one to chain up to");
    assert_ne!(ours as usize, theirs as usize);

    // SAFETY: the reference taken above is given back exactly once.
    unsafe { g_type_class_unref(class.cast()) };
}
