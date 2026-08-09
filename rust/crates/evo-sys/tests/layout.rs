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
