// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The same load-bearing check `eds-sys/tests/layout.rs` makes, for the one
// class M7's module subclasses: ask the running GObject type system what sizes
// it allocates an `EMailConfigServiceBackend` and its class at, and hold them
// against ours. A drift here writes an overridden vfunc pointer into the wrong
// offset of Evolution's own class struct — inside the process the user is
// typing their password into.
//
// It also asserts the join this crate exists to make: the class our subclass
// will chain up through must be *eds-sys's* `EExtensionClass`, not a second
// one generated here. Two `EExtension`s of the same shape and different
// identity would compile and would put the wrong `GObject` in the parent slot.

use evo_sys::*;
use std::mem::size_of;
use std::sync::{Mutex, MutexGuard};

/// GTK 3's hand-written `get_type` once-guards are not thread-safe (the whole
/// story is in tests/gtk.rs), and two tests here reach GtkScrolledWindow's
/// initialisation through Evolution's page classes. From parallel harness
/// threads that deadlocked, not raced: one thread parked in
/// `g_once_init_enter` while the other, inside the same once-guard, blocked
/// on the GObject type lock the first still held (observed 2026-09-02, both
/// stacks through `gtk_scrolled_window_get_type`). Every test here goes
/// through the type system under this lock.
static TYPE_SYSTEM: Mutex<()> = Mutex::new(());

fn type_lock() -> MutexGuard<'static, ()> {
    TYPE_SYSTEM
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `g_type_query()` fills nothing for a classed type whose class has never been
/// referenced, so take a class ref first — the same dance as in `eds-sys`.
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
fn service_backend_layout_matches_the_gtype_system() {
    let _lock = type_lock();
    let q = query(unsafe { e_mail_config_service_backend_get_type() });
    assert_eq!(
        q.instance_size as usize,
        size_of::<EMailConfigServiceBackend>(),
        "EMailConfigServiceBackend: instance size disagrees with g_type_query"
    );
    assert_eq!(
        q.class_size as usize,
        size_of::<EMailConfigServiceBackendClass>(),
        "EMailConfigServiceBackend: class size disagrees with g_type_query"
    );
}

/// What the subclass will chain up to. The `EExtension` in the parent slot has
/// to be the type EDS registered — which is what makes `jmap-backend-core`'s
/// registration helpers, written against `eds-sys`, usable for a class that
/// lives in Evolution rather than in EDS.
#[test]
fn the_parent_is_the_extension_eds_sys_names() {
    let _lock = type_lock();
    unsafe {
        assert_eq!(
            g_type_parent(e_mail_config_service_backend_get_type()),
            e_extension_get_type(),
            "EMailConfigServiceBackend no longer derives from EExtension"
        );
    }
    assert_eq!(
        size_of::<EMailConfigServiceBackend>(),
        size_of::<EExtension>() + size_of::<*mut ()>(),
        "the instance struct is no longer an EExtension plus its private pointer"
    );
}

/// The interface jmap-ui's vacation page fills (`submit`/`submit_finish`
/// written into its slots), held against the running library — a slot
/// written at the wrong offset is a wrong vfunc called with the wrong
/// arguments, inside Evolution's own process.
///
/// `g_type_query` answers nothing for interfaces (checked: it fills zero),
/// so the drift check reads the *default* vtable through our layout instead
/// and holds every slot against what `e_mail_config_page_default_init`
/// (3.52) is known to put there: a translated title, `check_complete` and
/// the three submit slots filled, `setup_defaults` and `commit_changes`
/// left NULL, and a `page_type` that is a real `GtkAssistantPageType`. A
/// shifted layout scrambles that NULL/non-NULL pattern.
#[test]
fn the_page_interface_defaults_read_correctly_through_our_layout() {
    let _lock = type_lock();
    unsafe {
        let gtype = e_mail_config_page_get_type();
        let vtable = g_type_default_interface_ref(gtype);
        assert!(!vtable.is_null(), "no default vtable for EMailConfigPage");
        let iface = &*vtable.cast::<EMailConfigPageInterface>();
        assert!(!iface.title.is_null(), "the default title is gone");
        assert_eq!(iface.sort_order, 0);
        assert!(
            iface.page_type <= 5,
            "page_type is not a GtkAssistantPageType"
        );
        assert!(iface.changed.is_none());
        assert!(iface.setup_defaults.is_none());
        assert!(iface.check_complete.is_some());
        assert!(iface.commit_changes.is_none());
        assert!(iface.submit_sync.is_some());
        assert!(iface.submit.is_some());
        assert!(iface.submit_finish.is_some());
        g_type_default_interface_unref(vtable);
    }
}

/// The class field a service backend is *found* by. Evolution's config pages
/// pick a backend out of every registered `EMailConfigServiceBackend` extension
/// by comparing `backend_name` against a Camel provider's protocol, so it is
/// the first thing M7's `class_init` has to fill in — and it has to land in the
/// first word after the inherited class, or the comparison reads a vfunc
/// pointer as a string.
#[test]
fn backend_name_sits_directly_after_the_inherited_class() {
    assert_eq!(
        std::mem::offset_of!(EMailConfigServiceBackendClass, backend_name),
        size_of::<EExtensionClass>(),
        "backend_name is no longer the first field of the subclass's half"
    );
}
