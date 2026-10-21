// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Who tells the guests, from the backend's side (`docs/MEETING-SCHEDULING.md`
//! section 4): the capabilities that tell Evolution not to mail the guests
//! itself, and the per-change flag that says the user chose not to send.

use eds_sys::{
    E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER, E_CAL_OPERATION_FLAG_CONFLICT_USE_NEWER,
    E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE, E_CAL_OPERATION_FLAG_NONE, ECalBackendClass,
    ECalBackendSyncClass,
};
use gobject_sys::{g_type_class_ref, g_type_class_unref};
use jmap_backend_cal::backend::{JmapCalBackend, JmapCalBackendClass, parent_class};
use jmap_backend_cal::scheduling::{
    CREATE_MESSAGES, ITIP_SUPPRESS_ON_REMOVE_SUPPORTED, SAVE_SCHEDULES, capabilities, dismissal,
    itip, received, receiving,
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
fn the_inherited_capabilities_gain_the_three_scheduling_ones() {
    let caps = capabilities(Some("refresh-supported,alarm-description"));
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
fn with_nothing_inherited_the_answer_has_no_empty_entry() {
    assert_eq!(split(&capabilities(None)), SCHEDULING_CAPS);
    assert_eq!(capabilities(Some("")), capabilities(None));
}

#[test]
fn a_scheduling_capability_already_inherited_is_not_listed_twice() {
    let caps = capabilities(Some("create-messages,refresh-supported"));
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
    assert_eq!(itip(E_CAL_OPERATION_FLAG_NONE), Itip::Send);
}

#[test]
fn disable_itip_message_suppresses_this_change() {
    assert_eq!(
        itip(E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE),
        Itip::Suppress
    );
}

#[test]
fn disable_itip_message_is_read_as_one_bit_among_the_conflict_flags() {
    assert_eq!(
        itip(E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE | E_CAL_OPERATION_FLAG_CONFLICT_USE_NEWER),
        Itip::Suppress
    );
    assert_eq!(itip(E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER), Itip::Send);
}

#[test]
fn a_change_while_receiving_is_marked_received_and_keeps_the_users_choice() {
    let guard = receiving();
    assert_eq!(
        itip(E_CAL_OPERATION_FLAG_NONE),
        Itip::Received { reply: true }
    );
    assert_eq!(
        itip(E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE),
        Itip::Received { reply: false }
    );
    drop(guard);
    assert_eq!(itip(E_CAL_OPERATION_FLAG_NONE), Itip::Send);
}

#[test]
fn the_receiving_mark_is_restored_by_each_guard_and_stays_on_its_thread() {
    let outer = receiving();
    drop(receiving());
    assert_eq!(
        itip(E_CAL_OPERATION_FLAG_NONE),
        Itip::Received { reply: true },
        "an inner guard keeps the outer mark"
    );
    let other = std::thread::spawn(|| itip(E_CAL_OPERATION_FLAG_NONE))
        .join()
        .unwrap();
    assert_eq!(other, Itip::Send);
    drop(outer);
    assert_eq!(itip(E_CAL_OPERATION_FLAG_NONE), Itip::Send);
}

#[test]
fn a_received_change_stays_received_on_a_thread_with_no_mark() {
    // EDS queues a change it cannot send with its opflags (e-cal-meta-backend.c
    // ecmb_create_object_sync), and later uploads it on another thread.
    let replayed = std::thread::spawn(|| {
        (
            itip(received(E_CAL_OPERATION_FLAG_NONE)),
            itip(received(E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE)),
        )
    })
    .join()
    .unwrap();
    assert_eq!(
        replayed,
        (
            Itip::Received { reply: true },
            Itip::Received { reply: false }
        )
    );
}

#[test]
fn the_received_mark_is_one_bit_outside_the_eds_flags() {
    let flags =
        E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER | E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE;
    assert_eq!(
        received(flags) & flags,
        flags,
        "the other flags pass through"
    );
    let mark = received(flags) & !flags;
    assert_eq!(mark.count_ones(), 1, "the mark is one bit");
    // EDS 3.52 to master define bits 0 to 4. The cache keeps the flags as a guint32.
    assert_eq!(mark & 0x1f, 0, "the mark is not an EDS flag");
    assert!(mark < 1 << 31, "the mark stays positive in a gint");
}

#[test]
fn a_reminder_dismissal_never_asks_the_server_to_send() {
    // EDS's reminder watcher dismisses with no flags (e-reminder-watcher.c).
    assert_eq!(itip(dismissal(E_CAL_OPERATION_FLAG_NONE)), Itip::Suppress);
    assert_eq!(
        itip(dismissal(E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER)),
        Itip::Suppress
    );
    assert_eq!(
        dismissal(E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER)
            & E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER,
        E_CAL_OPERATION_FLAG_CONFLICT_KEEP_SERVER,
        "the other flags pass through"
    );
}

// ---------------------------------------------------------------------------
// the vfunc

#[test]
fn class_init_installs_get_backend_property_over_the_meta_backends() {
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

#[test]
fn class_init_installs_receive_objects_sync_over_the_meta_backends() {
    let gtype = register_static::<JmapCalBackend>();
    // SAFETY: Referencing the registered class invokes class_init and returns the class struct.
    let class: *mut JmapCalBackendClass = unsafe { g_type_class_ref(gtype) }.cast();
    // SAFETY: the class is referenced and leads with the parent's.
    let ours: &ECalBackendSyncClass = unsafe { &(*class).parent_class.parent_class };
    let theirs = &parent_class()
        .expect("the parent class is registered")
        .parent_class;

    let ours = ours.receive_objects_sync.expect("installed");
    let theirs = theirs
        .receive_objects_sync
        .expect("the meta backend receives objects, so there is one to chain up to");
    assert_ne!(ours as usize, theirs as usize);

    // SAFETY: the reference taken above is given back exactly once.
    unsafe { g_type_class_unref(class.cast()) };
}
