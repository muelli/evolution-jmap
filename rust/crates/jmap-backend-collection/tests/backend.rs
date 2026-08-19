// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `ECollectionBackend` subclass: the type EDS registers and the three vfunc
// slots it dispatches through.
//
// Every call here goes *through the class struct*, because that is the only
// thing EDS ever does — and here that matters more than usual, because all three
// slots already hold something. `dup_resource_id` inherits a working
// implementation that answers the bare `[Resource] Identity`, which an address
// book and a calendar of the same JMAP id both share; `populate` inherits a
// placeholder; and `authenticate_sync`, two levels up on `EBackendClass`,
// inherits one that reports success without contacting anything. So a vfunc
// written but not installed is never a backend that does nothing — it is one
// that quietly answers something else, and no error says so.
//
// What is not here is a real instance: constructing one needs an
// `ESourceRegistryServer`, and so a running `evolution-source-registry` on the
// session bus, which neither this VM nor CI has. `JmapCollectionBackend::detached`
// stands in, and is sound for exactly one of the three — `dup_resource_id`,
// which never touches the backend it is handed. The other two can only be held
// against the slot their override has to displace.

use std::ffi::CString;
use std::mem::size_of;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    EBackendClass, ECollectionBackend, ECollectionBackendClass, ESource, ESourceResource,
    e_backend_get_type, e_collection_backend_get_type, e_source_address_book_get_type,
    e_source_calendar_get_type, e_source_get_extension, e_source_new_with_uid,
    e_source_resource_set_identity,
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

    /// The `ECollectionBackendClass` half, which is where `populate` and
    /// `dup_resource_id` live.
    fn vfuncs(&self) -> &ECollectionBackendClass {
        // SAFETY: the class is referenced and leads with the parent's.
        unsafe { &(*self.0).parent_class }
    }

    /// The `EBackendClass` half, two levels up, which is where
    /// `authenticate_sync` lives — a grandparent's vfunc, not the collection
    /// backend's own.
    fn backend_vfuncs(&self) -> &EBackendClass {
        &self.vfuncs().parent_class
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

#[test]
fn class_init_replaces_the_default_populate_rather_than_leaving_it() {
    // `ECollectionBackendClass::populate` is a placeholder — "so subclasses can
    // safely chain up" — so an override that is written but not installed is a
    // backend whose sidebar is simply empty: nothing claims the cached children
    // of previous sessions, nothing exports them, and nothing ever asks EDS for
    // the account's credentials, so no fan-out happens either. There is no error
    // and no log line anywhere in that, which is why the slot itself is a test.
    let class = Class::get();
    // SAFETY: the parent type's class is alive for as long as ours is.
    let parent: *mut ECollectionBackendClass =
        unsafe { g_type_class_peek(e_collection_backend_get_type()) }.cast();
    assert!(!parent.is_null(), "the parent class was not referenced");

    let ours = class
        .vfuncs()
        .populate
        .expect("class_init installed no populate");
    // SAFETY: a live class struct.
    let inherited = unsafe { (*parent).populate }.expect("EDS installs a placeholder");

    assert!(
        ours as usize != inherited as usize,
        "the slot still holds EDS's placeholder"
    );
}

#[test]
fn the_installed_populate_is_the_one_the_parent_can_still_be_reached_through() {
    // The chain-up the vfunc makes, from the other end: it reaches the parent's
    // populate through `g_type_class_peek` of `ECollectionBackend` rather than
    // through the instance's own class, which for a further subclass of ours
    // would point back at our own slot and recurse until the stack ran out. What
    // is asserted here is only that the pointer that walk finds is the parent's
    // placeholder and not ours — the call itself needs a live instance, and so a
    // running `evolution-source-registry`.
    let class = Class::get();
    // SAFETY: as above.
    let parent: *mut ECollectionBackendClass =
        unsafe { g_type_class_peek(e_collection_backend_get_type()) }.cast();
    // SAFETY: a live class struct.
    let inherited = unsafe { (*parent).populate }.expect("EDS installs a placeholder");
    let ours = class.vfuncs().populate.expect("class_init installed one");

    assert!(
        inherited as usize != ours as usize,
        "chaining up through the parent type's class would call our own populate"
    );
}

#[test]
fn class_init_replaces_the_default_child_added_rather_than_leaving_it() {
    // `ECollectionBackendClass::child_added` is a signal class closure with a
    // working body — it inserts the child into the backend's table, binds its
    // enabled flag and makes it non-removable — so an override that is written
    // but not installed is not a backend that breaks. It is one whose children
    // go on naming whatever server the account named when they were written,
    // which is invisible until the user moves the account.
    let class = Class::get();
    // SAFETY: the parent type's class is alive for as long as ours is.
    let parent: *mut ECollectionBackendClass =
        unsafe { g_type_class_peek(e_collection_backend_get_type()) }.cast();
    assert!(!parent.is_null(), "the parent class was not referenced");

    let ours = class
        .vfuncs()
        .child_added
        .expect("class_init installed no child_added");
    // SAFETY: a live class struct.
    let inherited = unsafe { (*parent).child_added }.expect("EDS installs its own");

    assert!(
        ours as usize != inherited as usize,
        "the slot still holds EDS's default, and the chain-up would recurse"
    );
}

#[test]
fn class_init_replaces_the_default_create_resource_sync_rather_than_leaving_it() {
    // The one slot of the five whose EDS default is a *refusal* rather than a
    // wrong answer: `collection_backend_create_resource()` does nothing but
    // `g_task_return_new_error (G_IO_ERROR_NOT_SUPPORTED, "%s does not support
    // creating remote resources")`, which the default `create_resource_sync`
    // drives through a closure. So an override that is written but not installed
    // is not a silent misbehaviour — it is Evolution answering "New Address
    // Book" with that message — and it is also why this override must NOT chain
    // up: the parent is the refusal being replaced.
    let class = Class::get();
    // SAFETY: the parent type's class is alive for as long as ours is.
    let parent: *mut ECollectionBackendClass =
        unsafe { g_type_class_peek(e_collection_backend_get_type()) }.cast();
    assert!(!parent.is_null(), "the parent class was not referenced");

    let ours = class
        .vfuncs()
        .create_resource_sync
        .expect("class_init installed no create_resource_sync");
    // SAFETY: a live class struct.
    let inherited = unsafe { (*parent).create_resource_sync }
        .expect("EDS installs a default that refuses every create");

    assert!(
        ours as usize != inherited as usize,
        "the slot still holds EDS's default, which refuses every create"
    );
}

#[test]
fn installing_the_middle_slots_left_the_ones_beside_them_inherited() {
    // `child_added` and `create_resource_sync` are the slots this crate writes
    // into the middle of `ECollectionBackendClass` — `dup_resource_id` and
    // `populate` sit at the front of it — and their neighbours are the other
    // signal closure and the rest of the resource vfuncs, all of which EDS fills
    // in. A write one slot out does not fail to compile; it replaces a function
    // of another signature, which is a call through a bad pointer the first time
    // EDS uses it. `create_resource` and `create_resource_finish` sit
    // immediately *after* the slot this crate now writes, so they are the two
    // that pin it from the far side.
    let class = Class::get();
    // SAFETY: as above.
    let parent: *mut ECollectionBackendClass =
        unsafe { g_type_class_peek(e_collection_backend_get_type()) }.cast();
    assert!(!parent.is_null(), "the parent class was not referenced");

    let ours = class.vfuncs();
    // SAFETY: a live class struct.
    let inherited = unsafe { &*parent };

    assert_eq!(
        ours.child_removed.map(|f| f as usize),
        inherited.child_removed.map(|f| f as usize),
        "class_init overwrote child_removed"
    );
    assert_eq!(
        ours.create_resource.map(|f| f as usize),
        inherited.create_resource.map(|f| f as usize),
        "class_init overwrote create_resource"
    );
    assert_eq!(
        ours.create_resource_finish.map(|f| f as usize),
        inherited.create_resource_finish.map(|f| f as usize),
        "class_init overwrote create_resource_finish"
    );
    // `delete_resource` and `delete_resource_finish` sit immediately after
    // `delete_resource_sync`, which this crate now writes too, so they are what
    // pins *that* slot from the far side — the same job `create_resource` and
    // `create_resource_finish` do for the create one.
    assert_eq!(
        ours.delete_resource.map(|f| f as usize),
        inherited.delete_resource.map(|f| f as usize),
        "class_init overwrote delete_resource"
    );
    assert_eq!(
        ours.delete_resource_finish.map(|f| f as usize),
        inherited.delete_resource_finish.map(|f| f as usize),
        "class_init overwrote delete_resource_finish"
    );
}

#[test]
fn class_init_replaces_the_default_delete_resource_sync_rather_than_leaving_it() {
    // The create slot's twin, and EDS's default is the same kind of thing:
    // `collection_backend_delete_resource()` does nothing but
    // `g_task_return_new_error (G_IO_ERROR_NOT_SUPPORTED, "%s does not support
    // deleting remote resources")`. So this override must not chain up either —
    // and leaving the slot uninstalled while `remote-deletable` is set on the
    // children (see `tests/delete_resource.rs`) would be the worst of both: a
    // "Delete" Evolution offers and answers with that message.
    let class = Class::get();
    // SAFETY: the parent type's class is alive for as long as ours is.
    let parent: *mut ECollectionBackendClass =
        unsafe { g_type_class_peek(e_collection_backend_get_type()) }.cast();
    assert!(!parent.is_null(), "the parent class was not referenced");

    let ours = class
        .vfuncs()
        .delete_resource_sync
        .expect("class_init installed no delete_resource_sync");
    // SAFETY: a live class struct.
    let inherited = unsafe { (*parent).delete_resource_sync }
        .expect("EDS installs a default that refuses every delete");

    assert!(
        ours as usize != inherited as usize,
        "the slot still holds EDS's default, which refuses every delete"
    );
}

/// The `EBackendClass` EDS installed its own defaults into, which is what an
/// override of `authenticate_sync` has to displace.
fn e_backend_class() -> *mut EBackendClass {
    // SAFETY: referencing our own class initialises the whole ancestry, so the
    // grandparent's class is alive for as long as ours is.
    unsafe { g_type_class_peek(e_backend_get_type()) }.cast()
}

#[test]
fn class_init_replaces_the_default_authenticate_sync_rather_than_leaving_it() {
    // The worst default of the three. `EBackendClass::authenticate_sync` is
    // installed by `e_backend_class_init` and its body is one line — "the
    // default implementation just reports success, it's for backends which do
    // not use (nor define) authentication routines" — so it returns
    // `E_SOURCE_AUTHENTICATION_ACCEPTED` without contacting anything.
    //
    // An override that is written but not installed is therefore not a backend
    // that fails to log in. It is one that EDS believes logged in: the account
    // goes CONNECTED, no fan-out ever runs, no credentials are ever asked for,
    // and there is no error, no prompt and no log line anywhere in it. That is
    // invisible in a way a NULL slot would not be, which is why the slot is a
    // test of its own.
    let class = Class::get();
    let parent = e_backend_class();
    assert!(
        !parent.is_null(),
        "the grandparent class was not referenced"
    );

    let ours = class
        .backend_vfuncs()
        .authenticate_sync
        .expect("class_init installed no authenticate_sync");
    // SAFETY: a live class struct.
    let inherited = unsafe { (*parent).authenticate_sync }
        .expect("EDS installs a default that accepts every account");

    assert!(
        ours as usize != inherited as usize,
        "the slot still holds EDS's default, which accepts without contacting anything"
    );
}

#[test]
fn installing_authenticate_sync_leaves_the_other_ebackend_slots_inherited() {
    // `authenticate_sync` is the first slot this crate writes into a half of
    // the class struct it does not own the layout of — bindgen's
    // `EBackendClass`, two levels up, sitting between GObject's class and
    // `ECollectionBackendClass`'s own vfuncs. A wrong offset there does not
    // fail to compile; it silently overwrites a neighbouring slot with a
    // function of a different signature, which is a call through a bad pointer
    // the first time EDS uses it. The two neighbours EDS fills in are
    // `get_destination_address` and `prepare_shutdown`, so they are what pins
    // it: both must still be exactly what the grandparent installed.
    let class = Class::get();
    let parent = e_backend_class();
    assert!(
        !parent.is_null(),
        "the grandparent class was not referenced"
    );

    let ours = class.backend_vfuncs();
    // SAFETY: a live class struct.
    let inherited = unsafe { &*parent };

    assert_eq!(
        ours.get_destination_address.map(|f| f as usize),
        inherited.get_destination_address.map(|f| f as usize),
        "class_init overwrote get_destination_address"
    );
    assert_eq!(
        ours.prepare_shutdown.map(|f| f as usize),
        inherited.prepare_shutdown.map(|f| f as usize),
        "class_init overwrote prepare_shutdown"
    );
}
