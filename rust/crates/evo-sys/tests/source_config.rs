// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The check `tests/layout.rs` makes, for the second class this crate's
// consumers subclass: `ESourceConfigBackend`. Evolution's New Address
// Book and New Calendar dialogs each build one candidate per extension of
// this class.
//
// This is a separate file instead of four more tests in `layout.rs`,
// because it pins down more than the sizes. `EMailConfigServiceBackend`'s
// class has one non-function field before its vfuncs. This class has two
// non-function fields, and its four vfunc slots are the entire mechanism
// for creating, editing, or committing a JMAP address book or calendar
// from the GUI. So this file asserts each slot's offset individually
// against the struct the class initialiser writes through. It also
// asserts the two `const gchar *` fields sit exactly where the header
// puts them: a wrong offset there turns a vfunc pointer into a string
// dereference.
//
// No test here constructs an `ESourceConfigBackend`. It is an
// `EExtension` of an `ESourceConfig`, which is a `GtkBox` and needs a
// display that this VM does not have. A test on this machine can only ask
// the running GObject type system what shape the class really has, which
// is what these tests do.

use evo_sys::*;
use std::mem::{offset_of, size_of};
use std::sync::Once;

/// Every test in this file starts by calling this function. This is not
/// boilerplate. Without it, the file deadlocks under `cargo test`'s
/// default thread pool. This deadlock was observed, not theorized. Two
/// threads parked in `futex_do_wait` for as long as the test run left
/// them there: one thread in
/// `source_config_backend_layout_matches_the_gtype_system`, and one
/// thread in `source_config_is_an_opaque_gtk_box`.
///
/// This is an ABBA deadlock between GLib's two type locks. Evolution's
/// own code sits in the middle of it. `e_source_config_backend_class_init`
/// (`e-util/e-source-config-backend.c`, 3.52.4) sets `extensible_type =
/// E_TYPE_SOURCE_CONFIG`. That means it registers a GtkBox subclass from
/// inside a class initialiser. GLib runs a class initialiser with its
/// type write lock held. A second thread, already inside
/// `e_source_config_get_type`'s `g_once_init_enter`, wants that same
/// write lock to finish registering. That second thread waits for the
/// first thread. The first thread waits for the `g_once` guard. Neither
/// thread ever moves.
///
/// So this function does the whole registration once, on one thread,
/// before any test runs in parallel. Referencing the backend class runs
/// that class initialiser to completion. That leaves `ESourceConfig`
/// registered. So every later call takes the `g_once` fast path, and
/// takes no lock at all.
///
/// This is exactly the same hazard as
/// `jmap_backend_core::subclass::ObjectSubclass::class_init_types`, one
/// library over. Here, Evolution is the class initialiser that reaches
/// out, instead of this project. It is a test-harness concern only:
/// Evolution loads modules and builds dialogs on one thread, so nothing
/// in a real session becomes the second thread here.
fn warm_up() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let klass = g_type_class_ref(e_source_config_backend_get_type());
        assert!(!klass.is_null(), "g_type_class_ref returned NULL");
        g_type_class_unref(klass);
        gtk_box_get_type();
        // The two derived config classes, registered on the same thread
        // and for the same reason. Each is another GtkBox subclass,
        // registered on first use. Leaving them to whichever test runs
        // first would put their registration back in a race with the
        // class initialiser above.
        e_book_source_config_get_type();
        e_cal_source_config_get_type();
    });
}

/// As in `layout.rs`: `g_type_query()` fills nothing for a classed type
/// whose class was never referenced. Take a class reference first.
fn query(gtype: GType) -> GTypeQuery {
    unsafe {
        let klass = g_type_class_ref(gtype);
        assert!(!klass.is_null(), "g_type_class_ref returned NULL");
        let mut q = std::mem::zeroed::<GTypeQuery>();
        g_type_query(gtype, &mut q);
        g_type_class_unref(klass);
        assert_ne!(q.type_, 0, "g_type_query left the query zeroed");
        q
    }
}

#[test]
fn source_config_backend_layout_matches_the_gtype_system() {
    warm_up();
    let q = query(unsafe { e_source_config_backend_get_type() });
    assert_eq!(
        q.instance_size as usize,
        size_of::<ESourceConfigBackend>(),
        "ESourceConfigBackend: instance size disagrees with g_type_query"
    );
    assert_eq!(
        q.class_size as usize,
        size_of::<ESourceConfigBackendClass>(),
        "ESourceConfigBackend: class size disagrees with g_type_query"
    );
}

/// The same join `layout.rs` makes for the mail config backend. It must
/// hold for this class too: the `EExtension` in the parent slot must be
/// the type EDS registered, not a second type minted here. Otherwise,
/// `jmap-backend-core`'s registration helpers cannot register a subclass
/// of it.
#[test]
fn the_parent_is_the_extension_eds_sys_names() {
    warm_up();
    unsafe {
        assert_eq!(
            g_type_parent(e_source_config_backend_get_type()),
            e_extension_get_type(),
            "ESourceConfigBackend no longer derives from EExtension"
        );
    }
    assert_eq!(
        size_of::<ESourceConfigBackend>(),
        size_of::<EExtension>() + size_of::<*mut ()>(),
        "the instance struct is no longer an EExtension plus its private pointer"
    );
}

/// The two fields that are not functions, in the order the header declares
/// them, starting immediately after the inherited class.
///
/// `ESourceConfig` matches a candidate against `backend_name`. This field
/// holds the `ESourceBackend:backend-name` value that an address book or
/// calendar source carries, so this crate's value must be the string the
/// registry modules already answer to. `parent_uid` is the field below
/// it, and this crate leaves it NULL. A JMAP address book or calendar
/// hangs off the collection account that discovered it, not off one of
/// Evolution's fixed "stub" placeholders. The header's own comment says
/// to leave `parent_uid` NULL in that case.
///
/// Both fields are read as `const gchar *`. A drift of one word here does
/// not fail a type check. Instead, it reads a function pointer as a
/// string and passes that string to `strcmp`.
#[test]
fn the_two_name_fields_lead_the_subclass_half() {
    warm_up();
    assert_eq!(
        offset_of!(ESourceConfigBackendClass, backend_name),
        size_of::<EExtensionClass>(),
        "backend_name is no longer the first field of the subclass's half"
    );
    assert_eq!(
        offset_of!(ESourceConfigBackendClass, parent_uid),
        offset_of!(ESourceConfigBackendClass, backend_name) + size_of::<*const ()>(),
        "parent_uid no longer follows backend_name"
    );
}

/// The four slots, each directly after the last, in the header's order.
///
/// This is the assertion the whole file exists for. A `class_init` writes
/// these four pointers through the struct declared in this crate, into a
/// class struct that Evolution allocated to its own header's shape. A
/// missing slot causes one of: no entry in the Type list, a NULL backend
/// when editing, a colour widget that never gets the source's colour, or
/// an OK button that asserts instead of saving. A wrong offset causes the
/// same failure, plus an indirect call through whatever else occupied
/// that offset.
#[test]
fn the_four_vfunc_slots_follow_the_names_in_order() {
    warm_up();
    let mut at = offset_of!(ESourceConfigBackendClass, parent_uid) + size_of::<*const ()>();
    for (name, offset) in [
        (
            "allow_creation",
            offset_of!(ESourceConfigBackendClass, allow_creation),
        ),
        (
            "insert_widgets",
            offset_of!(ESourceConfigBackendClass, insert_widgets),
        ),
        (
            "check_complete",
            offset_of!(ESourceConfigBackendClass, check_complete),
        ),
        (
            "commit_changes",
            offset_of!(ESourceConfigBackendClass, commit_changes),
        ),
    ] {
        assert_eq!(offset, at, "{name} is not where the header puts it");
        at += size_of::<*const ()>();
    }
    assert_eq!(
        at,
        size_of::<ESourceConfigBackendClass>(),
        "the class has grown a field past commit_changes"
    );
}

/// `ESourceConfig` itself stays an opaque handle, like the pages in
/// `tests/page.rs`. It is a `GtkBox`, so generating its layout would drag
/// in the GTK class structs that this crate deliberately avoids knowing
/// the shape of.
///
/// Passing `ESourceConfig` around as an opaque pointer is valid only
/// because nothing here reads a field of it. Treating a `GtkBox *` and an
/// `ESourceConfig *` as the same object, the way C's `GTK_BOX()` does, is
/// a claim this crate must check, not assume. So this test asks the
/// running type system, exactly as `tests/gtk.rs` asks it about the
/// widget classes.
#[test]
fn source_config_is_an_opaque_gtk_box() {
    warm_up();
    assert_eq!(
        size_of::<ESourceConfig>(),
        0,
        "ESourceConfig is no longer an opaque handle"
    );
    unsafe {
        assert_ne!(
            e_source_config_get_type(),
            0,
            "e_source_config_get_type() answered the invalid type"
        );
        assert!(
            g_type_is_a(e_source_config_get_type(), gtk_box_get_type()) != 0,
            "ESourceConfig is no longer a GtkBox"
        );
    }
}

/// The two classes a backend names as its `extensible_type`, and the one
/// assumption the handles above make about them.
///
/// `EExtensionClass::extensible_type` is a single `GType`.
/// `e_extensible_load_extensions` instantiates an extension only for the
/// extensible whose type it names. So which of these two classes a
/// subclass names decides whether its candidate turns up in New Address
/// Book or in New Calendar. Naming `ESourceConfig` itself would put the
/// candidate in both dialogs, and hand each dialog the other's scratch
/// source. That is why this file must reach the derived types at all.
///
/// Both classes are opaque handles. So this test makes the same assertion
/// `tests/gtk.rs` makes about the widget classes: the running type system
/// confirms they derive from the `ESourceConfig` that
/// `e_source_config_backend_get_config` returns. This is what licenses
/// `allow_creation`'s cast of that return value to an `ECalSourceConfig
/// *`, the Rust spelling of C's `E_CAL_SOURCE_CONFIG()`.
#[test]
fn the_two_dialog_classes_derive_from_the_config_a_backend_is_handed() {
    warm_up();
    for (name, gtype) in [
        // SAFETY: This call takes no arguments, and the type system
        // initializes itself.
        ("EBookSourceConfig", unsafe {
            e_book_source_config_get_type()
        }),
        // SAFETY: Same as above.
        ("ECalSourceConfig", unsafe {
            e_cal_source_config_get_type()
        }),
    ] {
        assert_ne!(gtype, 0, "{name}: get_type() answered the invalid type");
        assert!(
            // SAFETY: `gtype` is a registered type, as the code just
            // asserted.
            unsafe { g_type_is_a(gtype, e_source_config_get_type()) } != 0,
            "{name} is no longer an ESourceConfig"
        );
    }
    assert_eq!(
        size_of::<EBookSourceConfig>(),
        0,
        "EBookSourceConfig is no longer an opaque handle"
    );
    assert_eq!(
        size_of::<ECalSourceConfig>(),
        0,
        "ECalSourceConfig is no longer an opaque handle"
    );
}

/// The three source types one `ECalSourceConfig` serves, distinguished
/// only by `e_cal_source_config_get_source_type`'s answer.
///
/// This is not a binding detail. New Calendar, New Task List, and New
/// Memo List are the same dialog class with a different value here. So
/// every registered calendar config backend appears in all three
/// dialogs, unless its `allow_creation` says otherwise. Evolution's own
/// `cal-config-google` and `cal-config-gtasks` each override
/// `allow_creation` for exactly this reason. A JMAP account has
/// calendars and no task or memo collections, so the constant this crate
/// compares against must be the one libecal means by "events". The enum
/// is a `#[repr(C)]` set of integers generated from a header. So the
/// check for renumbering is that the four values are the four distinct
/// ones the header declares, in order.
#[test]
fn the_calendar_source_types_are_the_distinct_values_libecal_declares() {
    let all = [
        E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
        E_CAL_CLIENT_SOURCE_TYPE_TASKS,
        E_CAL_CLIENT_SOURCE_TYPE_MEMOS,
        E_CAL_CLIENT_SOURCE_TYPE_LAST,
    ];
    assert_eq!(
        all,
        [0, 1, 2, 3],
        "ECalClientSourceType has been renumbered"
    );
}
