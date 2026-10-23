// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the `ECollectionBackend` subclass: the type that EDS registers and the
// vfunc slots that EDS dispatches through.
//
// Each call goes through the class struct, as EDS does. Each slot already has
// an inherited value. For example, `dup_resource_id` returns the bare
// `[Resource] Identity`, which an address book and a calendar of one JMAP id
// share. `populate` is a placeholder. `authenticate_sync` (on `EBackendClass`,
// two levels up) reports success without a connection. Thus a vfunc that is not
// installed gives a different answer silently, with no error.
//
// These tests have no real instance. An instance needs an
// `ESourceRegistryServer` and a running `evolution-source-registry` on the
// session bus, which this VM and CI do not have.
// `JmapCollectionBackend::detached` is a substitute that is sound only for
// `dup_resource_id`, which does not touch the backend. For the other slots, the
// tests compare the override with the inherited value.

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

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The class that EDS dispatches through. The test holds a reference, so the
/// vfunc pointers stay valid.
struct Class(*mut JmapCollectionBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = register_static::<JmapCollectionBackend>();
        assert_ne!(gtype, 0, "the backend type did not register");
        // SAFETY: the type is registered, so a reference to its class runs
        // class_init and returns a class struct with the layout of this crate.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    /// The `ECollectionBackendClass` part, which holds `populate` and
    /// `dup_resource_id`.
    fn vfuncs(&self) -> &ECollectionBackendClass {
        // SAFETY: the class is referenced and leads with the parent's.
        unsafe { &(*self.0).parent_class }
    }

    /// The `EBackendClass` part, two levels up, which holds `authenticate_sync`.
    fn backend_vfuncs(&self) -> &EBackendClass {
        &self.vfuncs().parent_class
    }

    /// Calls `dup_resource_id` as `collection_backend_load_resources` in EDS
    /// does, and takes ownership of the result.
    fn dup_resource_id(&self, source: *mut ESource) -> Option<String> {
        let mut backend = JmapCollectionBackend::detached();
        let dup = self
            .vfuncs()
            .dup_resource_id
            .expect("class_init installed no dup_resource_id");
        // SAFETY: the slot is filled and the source is alive. This vfunc does
        // not read the detached instance (see the module comment).
        let raw: *mut gchar = unsafe {
            dup(
                ptr::from_mut(&mut *backend).cast::<ECollectionBackend>(),
                source,
            )
        };
        if raw.is_null() {
            return None;
        }
        // SAFETY: a non-NULL result is a NUL-terminated GLib allocation that the
        // caller owns, as the EDS hash table does.
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
        // SAFETY: no arguments. See tests/resource_id.rs for why the extension
        // types must be registered first.
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
        // SAFETY: a live source and NUL-terminated names. The source creates and
        // owns the extensions, and the setter copies the string.
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
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let gtype = register_static::<JmapCollectionBackend>();
        assert_ne!(gtype, 0, "the backend type did not register");

        // SAFETY: a registered type.
        let parent = unsafe { g_type_parent(gtype) };
        // SAFETY: no arguments.
        assert_eq!(parent, unsafe { e_collection_backend_get_type() });

        // SAFETY: a registered type; the name is owned by the type system.
        let name = unsafe { std::ffi::CStr::from_ptr(g_type_name(gtype)) };
        assert_eq!(name, JmapCollectionBackend::NAME);
    });
}

#[test]
fn the_registered_sizes_are_the_rust_struct_sizes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // GObject allocates the sizes given at registration. A mismatch writes
        // vfunc pointers past the end of the class. The eds-sys layout test does
        // the same check for the parent types.
        let _class = Class::get();
        let q = query(register_static::<JmapCollectionBackend>());

        assert_eq!(q.instance_size as usize, size_of::<JmapCollectionBackend>());
        assert_eq!(
            q.class_size as usize,
            size_of::<JmapCollectionBackendClass>()
        );
    });
}

#[test]
fn class_init_replaces_the_default_dup_resource_id_rather_than_leaving_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The inherited `dup_resource_id` returns `[Resource] Identity` unchanged.
        // Thus a missing override is not visible until two children of one JMAP id
        // collide.
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
    });
}

#[test]
fn the_installed_vfunc_answers_with_the_kind_as_well_as_the_identity() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let class = Class::get();

        assert_eq!(
            class
                .dup_resource_id(
                    Source::new(E_SOURCE_EXTENSION_ADDRESS_BOOK.to_str().unwrap(), "X1").0
                )
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
    });
}

#[test]
fn a_source_this_backend_did_not_create_gets_a_null_back() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // For EDS, NULL means "not a source of this backend". For a source in the
        // cache directory of the backend, NULL causes its deletion. Thus NULL is
        // correct only for a foreign source.
        let class = Class::get();

        assert_eq!(
            class.dup_resource_id(Source::new("Mail Account", "A1").0),
            None
        );
    });
}

#[test]
fn class_init_replaces_the_default_populate_rather_than_leaving_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `ECollectionBackendClass::populate` is a placeholder "so subclasses can
        // safely chain up". Without the override, the sidebar is empty. Nothing
        // claims or exports the cached children, and nothing asks EDS for the
        // credentials, so no fan-out occurs. No error or log line shows this.
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
    });
}

#[test]
fn the_installed_populate_is_the_one_the_parent_can_still_be_reached_through() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The vfunc chains up through `g_type_class_peek` of `ECollectionBackend`,
        // not through the class of the instance. For a subclass of this type, the
        // class of the instance gives this populate again, and the recursion
        // overflows the stack. The test asserts only that the pointer found is
        // the parent placeholder. The call needs a live instance and a running
        // `evolution-source-registry`.
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
    });
}

#[test]
fn class_init_replaces_the_default_child_added_rather_than_leaving_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `ECollectionBackendClass::child_added` is a signal class closure that
        // works. It adds the child to the backend table, binds its enabled flag and
        // makes it not removable. Without the override, the children keep the old
        // server. This is not visible until the user moves the account.
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
    });
}

#[test]
fn class_init_replaces_the_default_create_resource_sync_rather_than_leaving_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The EDS default of this slot is a refusal, not a wrong answer.
        // `collection_backend_create_resource()` only calls
        // `g_task_return_new_error (G_IO_ERROR_NOT_SUPPORTED, "%s does not support
        // creating remote resources")`. The default `create_resource_sync` calls it
        // through a closure. Without the override, "New Address Book" shows that
        // message. The override must not chain up, because the parent is the
        // refusal.
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
    });
}

#[test]
fn installing_the_middle_slots_left_the_ones_beside_them_inherited() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // This crate writes `child_added`, `create_resource_sync` and
        // `delete_resource_sync` in the middle of `ECollectionBackendClass`.
        // (`populate` and `dup_resource_id` are at the start.) EDS fills the
        // adjacent slots. A write to the wrong slot compiles, but it replaces a
        // function of a different signature. EDS then calls a bad pointer.
        // `child_removed` follows `child_added`. `create_resource` and
        // `create_resource_finish` follow `create_resource_sync`.
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
        // `delete_resource` and `delete_resource_finish` follow
        // `delete_resource_sync`.
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
    });
}

#[test]
fn class_init_replaces_the_default_delete_resource_sync_rather_than_leaving_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // As for create, the EDS default is a refusal:
        // `collection_backend_delete_resource()` only calls
        // `g_task_return_new_error (G_IO_ERROR_NOT_SUPPORTED, "%s does not support
        // deleting remote resources")`. Thus this override must not chain up. The
        // children have `remote-deletable` set (see `tests/delete_resource.rs`).
        // Without the override, Evolution offers "Delete" and then shows that error.
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
    });
}

/// The `EBackendClass` that holds the EDS defaults, which the overrides of
/// `authenticate_sync` and `get_destination_address` replace.
fn e_backend_class() -> *mut EBackendClass {
    // SAFETY: a reference to the class of this crate initialises all its
    // ancestors, so the grandparent class is alive while that class is.
    unsafe { g_type_class_peek(e_backend_get_type()) }.cast()
}

#[test]
fn class_init_replaces_the_default_authenticate_sync_rather_than_leaving_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `e_backend_class_init` installs `EBackendClass::authenticate_sync`. Its
        // body says "the default implementation just reports success, it's for
        // backends which do not use (nor define) authentication routines". It
        // returns `E_SOURCE_AUTHENTICATION_ACCEPTED` without a connection.
        //
        // Without the override, EDS thinks that the login succeeded. The account
        // goes CONNECTED, but no fan-out runs and no credentials are requested.
        // No error, prompt or log line shows this.
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
    });
}

#[test]
fn installing_authenticate_sync_and_get_destination_address_leaves_prepare_shutdown_inherited() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // This crate writes `authenticate_sync` and `get_destination_address` in
        // the bindgen `EBackendClass`, two levels up. This crate does not control
        // that layout. A wrong offset compiles, but it replaces an adjacent slot
        // with a function of a different signature. EDS then calls a bad pointer.
        // `prepare_shutdown` is between the two written slots, so it must keep the
        // value that the grandparent installed.
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
            ours.prepare_shutdown.map(|f| f as usize),
            inherited.prepare_shutdown.map(|f| f as usize),
            "class_init overwrote prepare_shutdown"
        );
    });
}

#[test]
fn class_init_replaces_the_default_get_destination_address_rather_than_leaving_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The default `backend_get_destination_address` reads the "connectable"
        // property through `e_backend_ref_connectable`. This crate does not set
        // that property, so the default always returns `FALSE`. Then the EDS
        // reachability monitor sees only the general network state, not the
        // account host. As for `authenticate_sync`, nothing shows this failure.
        let class = Class::get();
        let parent = e_backend_class();
        assert!(
            !parent.is_null(),
            "the grandparent class was not referenced"
        );

        let ours = class
            .backend_vfuncs()
            .get_destination_address
            .expect("class_init installed no get_destination_address");
        // SAFETY: a live class struct.
        let inherited = unsafe { (*parent).get_destination_address }
            .expect("EDS installs a default that reads the connectable property");

        assert!(
            ours as usize != inherited as usize,
            "the slot still holds EDS's default, which never sees a host this crate never sets \
             as the backend's connectable"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_backend_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}

#[test]
fn watch_returns_the_same_account_watch_on_every_call() {
    // `watch()` must return the field of the instance, not a new value. The
    // second claim fails, so both calls saw one `AccountWatch`.
    let backend = JmapCollectionBackend::detached();
    assert!(backend.watch().claim_connection());
    assert!(!backend.watch().claim_connection());
}
