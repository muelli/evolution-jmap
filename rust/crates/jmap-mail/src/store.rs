// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapStore`: the type Camel instantiates for a JMAP mail account.
//!
//! No vfunc is overridden yet. What the type does carry is the three things a
//! later increment cannot change cheaply: its parent, because every folder
//! vfunc is declared on one of the two candidates; the settings class it is
//! configured through — [`crate::settings`], without which a JMAP account has
//! nowhere to keep a server; and the slot the connection lives in, which is a
//! field of the instance struct and therefore part of a layout the vfuncs will
//! be reading through.

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eds_sys::{
    CamelOfflineStore, CamelOfflineStoreClass, CamelServiceClass, camel_offline_store_get_type,
};
use glib_sys::GType;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_mail_sync::MailSync;

use crate::settings::settings_type;

/// The instance struct. `#[repr(C)]` leading with the parent's instance struct
/// is what makes a `*mut JmapStore` usable as the `CamelStore *` every Camel
/// function takes.
#[repr(C)]
pub struct JmapStore {
    parent: CamelOfflineStore,
    /// The connection, from `connect_sync` to `disconnect_sync`.
    ///
    /// An `RwLock` rather than a `Mutex`, for the same reason as the address
    /// book backend's: Camel drives a store from several threads at once — a
    /// folder refresh, a message fetch and a folder-list update are three
    /// different operations that may all be in flight — and serialising them
    /// behind one lock would make each wait on the slowest. Only connect and
    /// disconnect, which replace the value, need exclusive access.
    connection: Slot<RwLock<Option<MailSync>>>,
}

impl JmapStore {
    /// Installs `sync` as the live connection, replacing whatever was there.
    ///
    /// Replacing rather than refusing: Camel reconnects a store it believes has
    /// gone away, and the connection being replaced is exactly the one it
    /// believes that about. The old one is dropped — and its socket closed —
    /// when this returns.
    pub fn store_connection(&self, sync: MailSync) {
        if let Some(connection) = self.connection() {
            *write(connection) = Some(sync);
        }
    }

    /// Drops the connection, reporting whether there was one.
    ///
    /// Camel asks a store to disconnect on shutdown whether or not it ever
    /// connected, so "there was none" is a normal outcome rather than a
    /// failure; it is still reported, because `disconnect_sync` is the caller
    /// that wants to know whether it did anything.
    pub fn drop_connection(&self) -> bool {
        match self.connection() {
            Some(connection) => write(connection).take().is_some(),
            None => false,
        }
    }

    /// Whether an operation would find a connection.
    pub fn is_connected(&self) -> bool {
        self.connection()
            .is_some_and(|connection| read(connection).is_some())
    }

    /// An instance outside the GObject type system: zeroed parent bytes and an
    /// initialised connection slot, which is what `instance_init` leaves behind
    /// minus the GObject.
    ///
    /// This exists for the tests, and it is not a shortcut — Camel constructs a
    /// store through `camel_session_add_service`, which needs a `CamelSession`,
    /// which in Evolution is an `EMailSession` over a source registry on the
    /// session bus. Nothing but the connection slot may be touched through the
    /// result: the parent bytes are a valid bit pattern (every field is a
    /// pointer or an integer, and NULL is a pointer) but they are not a
    /// GObject, so passing one to any Camel function is undefined behaviour.
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value, and an all-zero `Slot` is its
        // documented empty state.
        let store: Box<Self> = Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
        store.connection.init(RwLock::new(None));
        store
    }

    /// The connection slot, or `None` on an instance whose `instance_init` has
    /// not run or whose `finalize` already has.
    fn connection(&self) -> Option<&RwLock<Option<MailSync>>> {
        self.connection.get()
    }
}

/// A poisoned lock means some other operation panicked while holding it. The
/// connection it guards is not damaged by that — a `MailSync` is an HTTP client
/// and an account id — so carrying on is better than taking the store down with
/// whatever already went wrong.
fn read(lock: &RwLock<Option<MailSync>>) -> RwLockReadGuard<'_, Option<MailSync>> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write(lock: &RwLock<Option<MailSync>>) -> RwLockWriteGuard<'_, Option<MailSync>> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// The class struct, same rule one level up. It will grow overrides of
/// `CamelStoreClass`'s folder vfuncs; today it adds nothing of its own, which
/// is still not the same as *being* `CamelOfflineStoreClass` — the type needs
/// its own class for the overrides to have somewhere to go.
#[repr(C)]
pub struct JmapStoreClass {
    parent_class: CamelOfflineStoreClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelOfflineStore
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelOfflineStore derives from CamelStore, from
// CamelService, from GObject.
unsafe impl ObjectSubclass for JmapStore {
    /// `CamelJmapStore`, not `JmapStore`: Camel's own stores are all
    /// `Camel<Protocol>Store`, and the type name is what a user sees in a
    /// GObject warning about the wrong store type.
    const NAME: &'static CStr = c"CamelJmapStore";
    type Instance = JmapStore;
    type Class = JmapStoreClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_offline_store_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // Which class `camel_service_ref_settings` instantiates when nothing
        // has handed the service a settings object — and, more to the point,
        // which class Evolution's account editor and
        // `e_source_camel_configure_service` expect to configure. Inherited it
        // would be `CamelOfflineSettings`, which carries no server at all, so
        // this one line is what connects a JMAP account to a host.
        //
        // SAFETY: the class leads with CamelOfflineStoreClass, which derives
        // from CamelStoreClass, from CamelServiceClass — the contract above.
        unsafe { (*class.cast::<CamelServiceClass>()).settings_type = settings_type() };
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points at a zeroed instance struct of ours, and a
        // zeroed `Slot` is an empty one.
        unsafe { (*instance).connection.init(RwLock::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the
        // connection — and its socket — outlives the account.
        unsafe { (*instance).connection.clear() };
    }
}

/// Registers the store type, or returns it if it is already registered.
///
/// Statically, unlike the EDS backends' types: a Camel provider is not a
/// `GTypeModule`. Camel dlopens the module and never closes it, and the
/// provider struct it keeps a pointer to names these `GType`s forever, so there
/// is no unload for a dynamic type to be unregistered by — and a type that
/// *could* be unloaded here would be one Camel could still be asked to
/// instantiate.
pub fn store_type() -> GType {
    register_static::<JmapStore>()
}
