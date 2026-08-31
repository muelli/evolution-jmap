// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `tests/layout.rs`'s check, for the second class this crate's consumers
// subclass: `ESourceConfigBackend`, the extension Evolution's New Address Book
// and New Calendar dialogs build one candidate per.
//
// It is a separate file rather than four more tests in `layout.rs` because it
// pins a different thing as well as the sizes. `EMailConfigServiceBackend`'s
// class has one non-function field before its vfuncs; this one has two, and its
// four vfunc slots are the entire mechanism by which a JMAP address book or
// calendar can be created, edited or committed from the GUI at all. So each
// slot's offset is asserted individually against the struct the class
// initialiser will write through, and the two `const gchar *` fields — the ones
// a wrong offset turns into a vfunc pointer dereferenced as a string — are
// asserted to be exactly where the header puts them.
//
// Nothing here constructs an `ESourceConfigBackend`: it is an `EExtension` of
// an `ESourceConfig`, which is a `GtkBox` and so needs a display this VM does
// not have. What a test on this machine can do is ask the running GObject type
// system what shape the class really is, which is what these do.

use evo_sys::*;
use std::mem::{offset_of, size_of};
use std::sync::Once;

/// Every test in this file starts here, and it is not boilerplate: without it
/// the file deadlocks under `cargo test`'s default thread pool. Observed, not
/// theorised — two threads parked in `futex_do_wait` for as long as they were
/// left to, one in `source_config_backend_layout_matches_the_gtype_system` and
/// one in `source_config_is_an_opaque_gtk_box`.
///
/// It is an ABBA between GLib's two type locks, and the middle of it is
/// Evolution's own code. `e_source_config_backend_class_init`
/// (`e-util/e-source-config-backend.c`, 3.52.4) sets `extensible_type =
/// E_TYPE_SOURCE_CONFIG` — that is, it *registers a GtkBox subclass* from
/// inside a class initialiser, which GLib runs with its type write lock held.
/// A second thread that is already inside `e_source_config_get_type`'s
/// `g_once_init_enter` wants that same write lock to finish registering, and
/// waits for the first; the first waits for the `g_once`. Neither ever moves.
///
/// So the whole of it is done once, on one thread, before any test does
/// anything in parallel: referencing the backend class runs that class
/// initialiser to completion, which leaves `ESourceConfig` registered, so every
/// later call is the `g_once` fast path and takes no lock at all.
///
/// This is `jmap_backend_core::subclass::ObjectSubclass::class_init_types`'
/// hazard exactly, one library over and with Evolution rather than this project
/// as the class initialiser that reaches out. It is a test-harness concern
/// only: Evolution loads modules and builds dialogs on one thread, so nothing
/// in a real session gets to be the second thread here.
fn warm_up() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let klass = g_type_class_ref(e_source_config_backend_get_type());
        assert!(!klass.is_null(), "g_type_class_ref returned NULL");
        g_type_class_unref(klass);
        gtk_box_get_type();
        // The two derived config classes, on the same thread and for the same
        // reason: each is another GtkBox subclass registered on first ask, so
        // leaving them to whichever test got there first would put the
        // registration back in a race with the class initialiser above.
        e_book_source_config_get_type();
        e_cal_source_config_get_type();
    });
}

/// As in `layout.rs`: `g_type_query()` fills nothing for a classed type whose
/// class has never been referenced, so take a class ref first.
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

/// The same join `layout.rs` makes for the mail config backend, and it has to
/// hold for this class too: the `EExtension` in the parent slot must be the type
/// EDS registered, not a second one minted here, or
/// `jmap-backend-core`'s registration helpers cannot be used to register a
/// subclass of it.
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
/// `backend_name` is what `ESourceConfig` matches a candidate against — it is
/// the `ESourceBackend:backend-name` an address book or calendar source
/// carries, so ours has to be the string the registry modules already answer
/// to. `parent_uid` is the one below it, and NULL for us: a JMAP address book
/// or calendar hangs off the collection account that discovered it, not off one
/// of Evolution's fixed "stub" placeholders, which is the case the header's own
/// comment says to leave this NULL for.
///
/// Both are read as `const gchar *`. A drift of one word here does not crash on
/// a type check; it reads a function pointer as a string and hands it to
/// `strcmp`.
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
/// This is the assertion the whole file exists for. A `class_init` writes these
/// four pointers through the struct declared in this crate, into a class struct
/// Evolution allocated to its own header's shape; every symptom item 34
/// recorded — no entry in the Type list, a NULL backend when editing, a colour
/// widget that never gets the source's colour, an OK button that asserts
/// instead of saving — is what happens when one of the four is missing, and a
/// wrong offset is the same thing plus an indirect call through whatever else
/// was there.
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

/// `ESourceConfig` itself stays a handle, like the pages in `tests/page.rs`:
/// it is a `GtkBox`, so generating it would drag in the GTK class structs this
/// crate deliberately does not know the shape of.
///
/// The claim that licenses passing one around as an opaque pointer is that
/// nothing here reads a field of it — and the claim that licenses treating a
/// `GtkBox *` and an `ESourceConfig *` as the same object, the way C's
/// `GTK_BOX()` does, is this crate's business to check rather than assume. So
/// the running type system is asked, exactly as `tests/gtk.rs` asks it about
/// the widget classes.
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

/// The two classes a backend names as its `extensible_type`, and the one thing
/// the handles above assume about them.
///
/// `EExtensionClass::extensible_type` is a single `GType`, and
/// `e_extensible_load_extensions` instantiates an extension only for the
/// extensible whose type it names — so which of these two a subclass names is
/// the whole of whether its candidate turns up in New Address Book or in New
/// Calendar. Naming `ESourceConfig` itself would put it in both and hand each
/// the other's scratch source, which is why the derived types have to be
/// reachable from here at all.
///
/// Both are opaque handles, so the assertion is the one `tests/gtk.rs` makes
/// about the widget classes: the running type system says they derive from the
/// `ESourceConfig` that `e_source_config_backend_get_config` returns, which is
/// what licenses `allow_creation`'s cast of that return value to an
/// `ECalSourceConfig *` — the Rust spelling of C's `E_CAL_SOURCE_CONFIG()`.
#[test]
fn the_two_dialog_classes_derive_from_the_config_a_backend_is_handed() {
    warm_up();
    for (name, gtype) in [
        // SAFETY: no arguments, and the type system initialises itself.
        ("EBookSourceConfig", unsafe {
            e_book_source_config_get_type()
        }),
        // SAFETY: as above.
        ("ECalSourceConfig", unsafe {
            e_cal_source_config_get_type()
        }),
    ] {
        assert_ne!(gtype, 0, "{name}: get_type() answered the invalid type");
        assert!(
            // SAFETY: `gtype` is a registered type, as just asserted.
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

/// The three source types one `ECalSourceConfig` serves, distinguished by
/// nothing but `e_cal_source_config_get_source_type`'s answer.
///
/// This is not a binding detail. New Calendar, New Task List and New Memo List
/// are the same dialog class with a different value here, so every registered
/// calendar config backend is offered in all three unless its `allow_creation`
/// says otherwise — Evolution's own `cal-config-google` and `cal-config-gtasks`
/// each override it for exactly this reason. A JMAP account has calendars and
/// no task or memo collections, so the constant this crate compares against had
/// better be the one libecal means by "events"; the enum is `#[repr(C)]`
/// integers generated from a header, so the check that it has not been
/// renumbered under us is that the four values are the four distinct ones the
/// header declares, in its order.
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
