// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapStore`: the type Camel instantiates for a JMAP mail account.
//!
//! What it carries is the things a later increment cannot change cheaply: its
//! parent, because every folder vfunc is declared on one of the two candidates;
//! the settings class it is configured through — [`crate::settings`], without
//! which a JMAP account has nowhere to keep a server; and the two slots its
//! state lives in, which are fields of the instance struct and therefore part
//! of a layout the vfuncs read through — the connection, and the folder listing
//! read over it.
//!
//! The `CamelService` vfuncs that fill and empty the first of those slots are
//! [`crate::service`]; `CamelStoreClass`'s own `get_folder_info_sync`, which
//! reads the second, is [`crate::folders`]. Both are installed from `class_init`
//! below.

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eds_sys::{
    CAMEL_STORE_FOLDER_INFO_REFRESH, CamelOfflineStore, CamelOfflineStoreClass, CamelServiceClass,
    CamelStoreClass, CamelStoreGetFolderInfoFlags, camel_offline_store_get_type,
};
use glib_sys::GType;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_mail_sync::{FolderTree, FolderUpdate, MailSync};
use jmap_proto::State;

use crate::connect::StoreError;
use crate::settings::settings_type;

/// A folder listing, and the state it is current as of.
///
/// The tree is behind an [`Arc`] because it outlives the lock: a caller
/// translating it into a `CamelFolderInfo` forest must not hold the store's
/// listing locked while it does, and copying the tree per call would be a walk
/// of every mailbox for an answer that did not change.
struct Listing {
    state: State,
    tree: Arc<FolderTree>,
}

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
    /// The folder tree the connection last answered with, and when.
    ///
    /// A slot of its own rather than a field of the connection, so that a
    /// folder refresh and a reconnect do not queue behind each other. What ties
    /// the two together is an ordering rule instead: a listing is stored while
    /// the connection it was read over is still read-locked, so a
    /// [`store_connection`](JmapStore::store_connection) — which needs that
    /// lock exclusively — cannot slip in between the request and the write and
    /// have its clearing undone by a tree the previous connection produced.
    folders: Slot<RwLock<Option<Listing>>>,
}

impl JmapStore {
    /// Installs `sync` as the live connection, replacing whatever was there.
    ///
    /// Replacing rather than refusing: Camel reconnects a store it believes has
    /// gone away, and the connection being replaced is exactly the one it
    /// believes that about. The old one is dropped — and its socket closed —
    /// when this returns, and the folder listing read over it goes too: a
    /// reconnect happens because something about the account changed, and the
    /// server behind the new connection may not be the one the old tree —
    /// paths, message counts, and the JMAP ids every later request is built
    /// from — describes.
    pub fn store_connection(&self, sync: MailSync) {
        if let Some(connection) = self.connection() {
            let mut connection = write(connection);
            self.forget_folders();
            *connection = Some(sync);
        }
    }

    /// Drops the connection, reporting whether there was one.
    ///
    /// Camel asks a store to disconnect on shutdown whether or not it ever
    /// connected, so "there was none" is a normal outcome rather than a
    /// failure; it is still reported, because `disconnect_sync` is the caller
    /// that wants to know whether it did anything.
    ///
    /// The folder listing goes with it. That changes no answer — with no
    /// connection there is nothing that could serve a tree, and the reconnect
    /// clears it again anyway — but a disconnected account holding its whole
    /// mailbox tree in memory until Evolution quits is dead weight, and the
    /// point of a disconnect is that the account is not in use.
    pub fn drop_connection(&self) -> bool {
        match self.connection() {
            Some(connection) => {
                let mut connection = write(connection);
                self.forget_folders();
                connection.take().is_some()
            }
            None => false,
        }
    }

    /// Whether an operation would find a connection.
    pub fn is_connected(&self) -> bool {
        self.connection()
            .is_some_and(|connection| read(connection).is_some())
    }

    /// The account's folder tree — what `get_folder_info_sync` answers with.
    ///
    /// `flags` is Camel's word verbatim, and the bit that matters here is
    /// `CAMEL_STORE_FOLDER_INFO_REFRESH`: Camel asks a store for its folder
    /// tree constantly, and sets that bit on the few of those calls that mean
    /// "go and look". Without it the listing already in hand is the answer, and
    /// no request is made at all. With it, one `Mailbox/changes` decides
    /// whether the tree has to be walked again — see
    /// [`MailSync::folder_tree_since`], which is where the rule that a mailbox
    /// delta cannot be applied folder by folder lives.
    ///
    /// The first call has nothing in hand and therefore lists whatever the
    /// flags say: an account that opened empty until something asked it to
    /// refresh would be an account with no mail in it.
    ///
    /// The other flags are not read yet. `SUBSCRIBED` and `SUBSCRIPTION_LIST`
    /// ask for the tree filtered to what the user subscribed to, which is a
    /// filter on the tree rather than a different request, and `FAST` asks for
    /// it without message counts, which JMAP includes in the mailbox anyway.
    pub fn folders(
        &self,
        flags: CamelStoreGetFolderInfoFlags,
    ) -> Result<Arc<FolderTree>, StoreError> {
        let (connection, folders) = self
            .connection()
            .zip(self.folder_listing())
            .ok_or(StoreError::Disconnected)?;

        // Held across the request, which is the ordering rule the `folders`
        // field documents: the connection a listing was read over is still ours
        // when the listing is written.
        let connection = read(connection);
        let sync = connection.as_ref().ok_or(StoreError::Disconnected)?;

        let held = read(folders)
            .as_ref()
            .map(|listing| (listing.state.clone(), Arc::clone(&listing.tree)));

        let listing = match held {
            Some((_, tree)) if flags & CAMEL_STORE_FOLDER_INFO_REFRESH == 0 => return Ok(tree),
            Some((state, tree)) => match sync.folder_tree_since(&state)? {
                // The tree is kept, not rebuilt from an equal one: Camel diffs
                // the forests it is handed to decide which folders to announce
                // as created or deleted, and every caller above holds the same
                // `Arc` as before.
                FolderUpdate::Unchanged(state) => Listing { state, tree },
                FolderUpdate::Rebuilt { state, tree } => Listing {
                    state,
                    tree: Arc::new(tree),
                },
            },
            None => {
                let (state, tree) = sync.folder_tree()?;
                Listing {
                    state,
                    tree: Arc::new(tree),
                }
            }
        };

        let tree = Arc::clone(&listing.tree);
        *write(folders) = Some(listing);
        drop(connection);
        Ok(tree)
    }

    /// Drops the folder listing. Called with the connection lock held, by the
    /// two operations that make a listing stop describing the account the store
    /// is pointed at.
    fn forget_folders(&self) {
        if let Some(folders) = self.folder_listing() {
            *write(folders) = None;
        }
    }

    /// An instance outside the GObject type system: zeroed parent bytes and
    /// initialised slots, which is what `instance_init` leaves behind minus the
    /// GObject.
    ///
    /// This exists for the tests, and it is not a shortcut — Camel constructs a
    /// store through `camel_session_add_service`, which needs a `CamelSession`,
    /// which in Evolution is an `EMailSession` over a source registry on the
    /// session bus. Nothing but the slots may be touched through the result: the
    /// parent bytes are a valid bit pattern (every field is a pointer or an
    /// integer, and NULL is a pointer) but they are not a GObject, so passing
    /// one to any Camel function is undefined behaviour.
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value, and an all-zero `Slot` is its
        // documented empty state.
        let store: Box<Self> = Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
        store.connection.init(RwLock::new(None));
        store.folders.init(RwLock::new(None));
        store
    }

    /// The connection slot, or `None` on an instance whose `instance_init` has
    /// not run or whose `finalize` already has.
    fn connection(&self) -> Option<&RwLock<Option<MailSync>>> {
        self.connection.get()
    }

    /// The folder listing slot, with the same caveat.
    fn folder_listing(&self) -> Option<&RwLock<Option<Listing>>> {
        self.folders.get()
    }
}

/// A poisoned lock means some other operation panicked while holding it. What
/// it guards is not damaged by that — a `MailSync` is an HTTP client and an
/// account id, a `Listing` is a tree and a state string — so carrying on is
/// better than taking the store down with whatever already went wrong.
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
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
        let service = class.cast::<CamelServiceClass>();
        unsafe { (*service).settings_type = settings_type() };

        // Connect, authenticate, disconnect. They live in `crate::service`
        // rather than here because what they do is one operation split across
        // three slots by Camel's re-prompt loop, and reads as one file.
        // SAFETY: as above.
        unsafe { crate::service::install_vfuncs(service) };

        // And the folder listing. `CamelStore` leaves `get_folder_info_sync`
        // NULL and `camel_store_get_folder_info_sync` refuses to call a store
        // that has not filled it in, so this line is the difference between an
        // account with folders and one with a runtime warning.
        //
        // SAFETY: the class leads with CamelOfflineStoreClass, which leads with
        // CamelStoreClass — the contract above.
        unsafe { crate::folders::install_vfuncs(class.cast::<CamelStoreClass>()) };
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points at a zeroed instance struct of ours, and a
        // zeroed `Slot` is an empty one.
        unsafe {
            (*instance).connection.init(RwLock::new(None));
            (*instance).folders.init(RwLock::new(None));
        };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the
        // connection — and its socket — outlives the account, and the folder
        // listing leaks with it.
        unsafe {
            (*instance).connection.clear();
            (*instance).folders.clear();
        };
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
