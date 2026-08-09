// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `ECollectionBackend` subclass: the type EDS registers and the vfunc slot
// it dispatches through.
//
// Every call here goes *through the class struct*, because that is the only
// thing EDS ever does — and here that matters more than usual. The parent class
// installs a `dup_resource_id` of its own, so a vfunc that is written but not
// installed is not a backend that does nothing; it is a backend that quietly
// uses EDS's default and answers the bare `[Resource] Identity`, which an
// address book and a calendar of the same JMAP id both share.
//
// What is not here is a real instance: constructing one needs an
// `ESourceRegistryServer`, and so a running `evolution-source-registry` on the
// session bus, which neither this VM nor CI has. `JmapCollectionBackend::detached`
// stands in, and is sound for exactly this vfunc — it never touches the backend
// it is handed.

use std::ffi::CString;
use std::mem::size_of;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ECollectionBackend, ECollectionBackendClass, ESource, ESourceResource,
    e_collection_backend_get_type, e_source_address_book_get_type, e_source_calendar_get_type,
    e_source_get_extension, e_source_new_with_uid, e_source_resource_set_identity,
};
use glib_sys::{g_free, gchar};
use gobject_sys::{
    GTypeQuery, g_object_unref, g_type_class_peek, g_type_class_ref, g_type_class_unref,
    g_type_name, g_type_parent, g_type_query,
};
use jmap_backend_collection::backend::{JmapCollectionBackend, JmapCollectionBackendClass};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

/// The class EDS would dispatch through, kept referenced for the test's
/// duration so the vfunc pointers stay valid.
struct Class(*mut JmapCollectionBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = register_static::<JmapCollectionBackend>();
        assert_ne!(gtype, 0, "the backend type did not register");
        // SAFETY: the type is registered, so referencing its class runs
        // class_init and hands back a class struct of our own layout.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    /// The `ECollectionBackendClass` half, which is where the slot lives.
    fn vfuncs(&self) -> &ECollectionBackendClass {
        // SAFETY: the class is referenced and leads with the parent's.
        unsafe { &(*self.0).parent_class }
    }

    /// Calls `dup_resource_id` the way EDS's `collection_backend_load_resources`
    /// does, and takes ownership of what it hands back.
    fn dup_resource_id(&self, source: *mut ESource) -> Option<String> {
        let mut backend = JmapCollectionBackend::detached();
        let dup = self
            .vfuncs()
            .dup_resource_id
            .expect("class_init installed no dup_resource_id");
        // SAFETY: the slot is filled, the source is alive, and the detached
        // instance is never read by this vfunc — see the module comment.
        let raw: *mut gchar = unsafe {
            dup(
                ptr::from_mut(&mut *backend).cast::<ECollectionBackend>(),
                source,
            )
        };
        if raw.is_null() {
            return None;
        }
        // SAFETY: a non-NULL answer is a NUL-terminated GLib allocation this
        // caller owns, exactly as EDS's hash table would.
        let id = unsafe { std::ffi::CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: freed exactly once, as EDS frees it.
        unsafe { g_free(raw.cast()) };
        Some(id)
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: the reference taken in `get` is given back exactly once.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// A child source of the given kind, carrying the given identity.
struct Source(*mut ESource);

impl Source {
    fn new(extension: &str, identity: &str) -> Self {
        // SAFETY: no arguments; see tests/resource_id.rs for why the extension
        // types are touched first.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }
        let uid = CString::new("jmap-collection-child").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");

        let extension = CString::new(extension).expect("no NUL in an extension name");
        let identity = CString::new(identity).expect("no NUL in a test identity");
        // SAFETY: a live source and NUL-terminated names; the extensions are
        // created on demand and owned by the source, and the setter copies.
        unsafe {
            assert!(!e_source_get_extension(source, extension.as_ptr()).is_null());
            let resource: *mut ESourceResource =
                e_source_get_extension(source, E_SOURCE_EXTENSION_RESOURCE.as_ptr()).cast();
            e_source_resource_set_identity(resource, identity.as_ptr());
        }
        Self(source)
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: the reference is given back exactly once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn query(gtype: glib_sys::GType) -> GTypeQuery {
    // SAFETY: the type is registered and its class referenced by the caller.
    unsafe {
        let mut q = std::mem::zeroed::<GTypeQuery>();
        g_type_query(gtype, &mut q);
        q
    }
}

#[test]
fn the_backend_registers_as_a_subclass_of_ecollectionbackend() {
    let gtype = register_static::<JmapCollectionBackend>();
    assert_ne!(gtype, 0, "the backend type did not register");

    // SAFETY: a registered type.
    let parent = unsafe { g_type_parent(gtype) };
    // SAFETY: no arguments.
    assert_eq!(parent, unsafe { e_collection_backend_get_type() });

    // SAFETY: a registered type; the name is owned by the type system.
    let name = unsafe { std::ffi::CStr::from_ptr(g_type_name(gtype)) };
    assert_eq!(name, JmapCollectionBackend::NAME);
}

#[test]
fn the_registered_sizes_are_the_rust_struct_sizes() {
    // The same bet eds-sys's layout test makes one level down, made again for
    // the type this crate declares: GObject allocates what registration told it
    // to, and a mismatch writes vfunc pointers past the end of the class.
    let _class = Class::get();
    let q = query(register_static::<JmapCollectionBackend>());

    assert_eq!(q.instance_size as usize, size_of::<JmapCollectionBackend>());
    assert_eq!(
        q.class_size as usize,
        size_of::<JmapCollectionBackendClass>()
    );
}

#[test]
fn class_init_replaces_the_default_dup_resource_id_rather_than_leaving_it() {
    // `ECollectionBackendClass` comes with a working `dup_resource_id` — it
    // returns `[Resource] Identity` verbatim — so an uninstalled override is
    // invisible until two children of one JMAP id collide.
    let class = Class::get();
    // SAFETY: the parent type's class is alive for as long as ours is.
    let parent: *mut ECollectionBackendClass =
        unsafe { g_type_class_peek(e_collection_backend_get_type()) }.cast();
    assert!(!parent.is_null(), "the parent class was not referenced");

    let ours = class
        .vfuncs()
        .dup_resource_id
        .expect("class_init installed no dup_resource_id");
    // SAFETY: a live class struct.
    let inherited = unsafe { (*parent).dup_resource_id }.expect("EDS installs its own default");

    assert!(
        ours as usize != inherited as usize,
        "the slot still holds EDS's default"
    );
}

#[test]
fn the_installed_vfunc_answers_with_the_kind_as_well_as_the_identity() {
    let class = Class::get();

    assert_eq!(
        class
            .dup_resource_id(Source::new(E_SOURCE_EXTENSION_ADDRESS_BOOK.to_str().unwrap(), "X1").0)
            .as_deref(),
        Some("addressbook:X1")
    );
    assert_eq!(
        class
            .dup_resource_id(Source::new(E_SOURCE_EXTENSION_CALENDAR.to_str().unwrap(), "X1").0)
            .as_deref(),
        Some("calendar:X1"),
        "an address book and a calendar of one JMAP id must not answer alike"
    );
}

#[test]
fn a_source_this_backend_did_not_create_gets_a_null_back() {
    // NULL is EDS's "not one of yours" — and, for a source in this backend's
    // own cache directory, its deletion. Answering it for a foreign source is
    // right; answering it for one of ours would not be.
    let class = Class::get();

    assert_eq!(
        class.dup_resource_id(Source::new("Mail Account", "A1").0),
        None
    );
}
