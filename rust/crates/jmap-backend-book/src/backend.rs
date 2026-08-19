// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `EBookMetaBackend` subclass itself.
//!
//! Everything it does has already been written and tested one layer down —
//! [`connect`] opens the connection, [`ops`] is the vfunc bodies,
//! [`marshal`](crate::marshal) the C conversions. What is
//! left here, and only here, is the part that cannot be tested against a live
//! instance: the instance and class structs, the seven vfunc slots, and the
//! lookup of the connection an instance holds.
//!
//! So the code below is deliberately dull. Each vfunc is a panic guard, a look
//! in the session slot, and a call into `ops`; the one exception is
//! `get_changes_sync`, which has a third answer — chain up to
//! `EBookMetaBackend`'s own implementation — and that chain-up needs the parent
//! class pointer, which is why it could not live in `ops` with the decision
//! that produces it.
//!
//! ## The Stop button
//!
//! Every operation here is cancellable, and the cancellable that stops it is
//! the one EDS handed *that call* — not the account's. [`observe`], held for
//! the length of the call by `with_connection`, installs it as the
//! cancellation of every request this thread makes, and the client honours it
//! in preference to whatever it was built with. The connection itself is built
//! carrying no flag at all, so a connect the user stopped cannot leave behind a
//! client that refuses everything afterwards.
//!
//! What this does not do is abort a request already blocked in a socket read:
//! cancellation is checked between requests and before one is sent, so a Stop
//! during a slow response waits for that response. `tests/cancellation.rs` is
//! the acceptance suite, and says the same.
//!
//! [`observe`]: jmap_backend_core::cancel::observe

use std::ffi::CStr;
#[cfg(feature = "testing")]
use std::mem::MaybeUninit;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eds_sys::{
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, EBookMetaBackend, EBookMetaBackendClass,
    EConflictResolution, EContact, ENamedParameters, ESourceAuthenticationResult,
    GTlsCertificateFlags, e_backend_get_source, e_book_backend_set_writable,
    e_book_meta_backend_get_type, e_client_error_create,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GSList, GTRUE, GType, gboolean, gchar, guint32};
use gobject_sys::g_type_class_peek;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{cstring_lossy, set_raw_gerror};
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::guard_bool;
use jmap_book_sync::BookSync;

use crate::connect::{self, ACCEPTED_AUTH_RESULT, write_auth_result};
use crate::ops::{self, Outcome};

/// The JMAP address book backend.
///
/// The name is the one EDS will look the factory's product up by, and it
/// follows the convention every other backend uses (`EBookBackendEws`,
/// `EBookBackendCardDAV`) because the module and factory names are derived
/// from it by hand in the next layer up.
#[repr(C)]
pub struct JmapBookBackend {
    /// GObject's; never read by this code, only handed back to EDS as the
    /// instance pointer it gave us.
    parent: EBookMetaBackend,
    /// The connection, from `connect_sync` to `disconnect_sync`.
    ///
    /// An `RwLock` rather than a `Mutex`: EDS calls the read-only vfuncs from
    /// several threads at once, and serialising them behind one lock would
    /// make a long `list_existing_sync` block every `load_contact_sync`
    /// behind it. Only connect and disconnect, which replace the value, need
    /// exclusive access.
    session: Slot<RwLock<Option<BookSync>>>,
}

/// The class struct. Nothing of ours lives in it yet; it exists because
/// GObject needs a size to allocate and a place to put the vfunc slots, and
/// because the factory in the next increment will want to reach it.
#[repr(C)]
pub struct JmapBookBackendClass {
    pub parent_class: EBookMetaBackendClass,
}

impl JmapBookBackend {
    /// Installs `sync` as the live connection, replacing whatever was there.
    ///
    /// Replacing rather than refusing: `connect_sync` is reached again after a
    /// `requires_reconnect`, and the old connection is exactly what is being
    /// replaced. It is dropped — and its socket closed — when this returns.
    pub fn store_connection(&self, sync: BookSync) {
        if let Some(session) = self.session() {
            *write(session) = Some(sync);
        }
    }

    /// Drops the connection, reporting whether there was one.
    pub fn drop_connection(&self) -> bool {
        match self.session() {
            Some(session) => write(session).take().is_some(),
            None => false,
        }
    }

    /// Whether an operation would find a connection.
    pub fn is_connected(&self) -> bool {
        self.session()
            .is_some_and(|session| read(session).is_some())
    }

    /// An instance outside the GObject type system: zeroed parent bytes and an
    /// initialised session slot, which is what `instance_init` leaves behind
    /// minus the GObject.
    ///
    /// This exists for the tests, and it is not a shortcut — a real instance
    /// needs an `ESourceRegistry`, which needs `evolution-source-registry` on
    /// the session bus, which neither the test VM nor CI has. Nothing but the
    /// session slot may be touched through the result: the parent bytes are a
    /// valid bit pattern (every field is a pointer or an integer, and NULL is
    /// a pointer) but they are not a GObject, so passing one to any EDS
    /// function is undefined behaviour.
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value, and an all-zero `Slot` is its
        // documented empty state.
        let backend: Box<Self> = Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
        backend.session.init(RwLock::new(None));
        backend
    }

    /// The connection slot, or `None` on an instance whose `instance_init` has
    /// not run or whose `finalize` already has.
    fn session(&self) -> Option<&RwLock<Option<BookSync>>> {
        self.session.get()
    }
}

// SAFETY: both structs are #[repr(C)] and lead with the EBookMetaBackend
// instance and class structs respectively, and EBookMetaBackend derives from
// GObject (via EBookBackendSync, EBookBackend and EBackend).
unsafe impl ObjectSubclass for JmapBookBackend {
    const NAME: &'static CStr = c"EBookBackendJmap";
    type Instance = JmapBookBackend;
    type Class = JmapBookBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the EDS type system initialises itself.
        unsafe { e_book_meta_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; the slots below are all in that half.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.connect_sync = Some(connect_sync);
        vfuncs.disconnect_sync = Some(disconnect_sync);
        vfuncs.list_existing_sync = Some(list_existing_sync);
        vfuncs.get_changes_sync = Some(get_changes_sync);
        vfuncs.load_contact_sync = Some(load_contact_sync);
        vfuncs.save_contact_sync = Some(save_contact_sync);
        vfuncs.remove_contact_sync = Some(remove_contact_sync);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points at a zeroed instance struct of ours, and a
        // zeroed `Slot` is an empty one.
        unsafe { (*instance).session.init(RwLock::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the
        // connection — and its socket — outlives the address book.
        unsafe { (*instance).session.clear() };
    }
}

/// The parent's class struct, for the one vfunc that chains up.
///
/// `g_type_class_peek` rather than `_ref`: an initialised class is what having
/// registered a subclass of it guarantees, and taking a reference here would
/// mean giving one back on a path that has no natural place to do so.
pub fn parent_class() -> Option<&'static EBookMetaBackendClass> {
    // SAFETY: peeking a type that has never been referenced returns NULL,
    // which is handled; otherwise the class is alive for as long as the type
    // is, which for an EDS type is the life of the process.
    unsafe {
        g_type_class_peek(<JmapBookBackend as ObjectSubclass>::parent_type())
            .cast::<EBookMetaBackendClass>()
            .as_ref()
    }
}

// ---------------------------------------------------------------------------
// the vfunc slots

unsafe extern "C" fn connect_sync(
    meta_backend: *mut EBookMetaBackend,
    credentials: *const ENamedParameters,
    out_auth_result: *mut ESourceAuthenticationResult,
    // Left untouched. They describe a TLS certificate the user might be asked
    // to accept, and the client offers no way to get at one — a bad
    // certificate reaches us as a transport failure and nothing more. Writing
    // a made-up value would put a dialog in front of the user that cannot be
    // answered truthfully.
    _out_certificate_pem: *mut *mut gchar,
    _out_certificate_errors: *mut GTlsCertificateFlags,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: EDS's own contract for the vfunc: a valid instance of ours, a
    // NULL-or-valid ENamedParameters and GCancellable, and out-parameters that
    // are NULL or writable.
    unsafe {
        guard_bool("connect_sync", error, || {
            let Some(backend) = instance(meta_backend) else {
                return fail_offline(error);
            };

            // EDS calls this whenever it suspects the connection is gone,
            // including when it is not. Re-opening a live one would drop a
            // socket other threads are mid-request on.
            if backend.is_connected() {
                write_auth_result(out_auth_result, ACCEPTED_AUTH_RESULT);
                return GTRUE;
            }

            let source = e_backend_get_source(meta_backend.cast());
            let Some(sync) =
                connect::connect(source, credentials, cancellable, out_auth_result, error)
            else {
                return GFALSE;
            };

            backend.store_connection(sync);
            // Without this the address book is read-only: every write comes
            // back as "Permission denied" and Evolution greys the book out.
            // JMAP has no per-book "may I write" flag, so the answer is the
            // same as the account's.
            //
            // This is `e_book_backend_set_writable` and not the meta
            // backend's `set_connected_writable`, which reads like the one to
            // call and is not: the moment this vfunc returns TRUE, EDS's
            // `ebmb_update_connection_values` *overwrites* connected-writable
            // with `e_book_backend_get_writable()` — so the meta backend's
            // setter is undone by the very call that was about to read it,
            // and the flag EDS keeps for opening the book offline is set from
            // this one too. What the vfunc's own documentation asks for.
            e_book_backend_set_writable(meta_backend.cast(), GTRUE);
            GTRUE
        })
    }
}

unsafe extern "C" fn disconnect_sync(
    meta_backend: *mut EBookMetaBackend,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        guard_bool("disconnect_sync", error, || {
            // Dropping a connection that is not there is what EDS asks for on
            // shutdown after a failed connect, so it is a success, not a
            // failure: there is nothing left to do and nothing went wrong.
            if let Some(backend) = instance(meta_backend) {
                backend.drop_connection();
            }
            GTRUE
        })
    }
}

unsafe extern "C" fn list_existing_sync(
    meta_backend: *mut EBookMetaBackend,
    out_new_sync_tag: *mut *mut gchar,
    out_existing_objects: *mut *mut GSList,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "list_existing_sync",
            meta_backend,
            cancellable,
            error,
            |sync| ops::list_existing(sync, out_new_sync_tag, out_existing_objects, error),
        )
    }
}

#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
unsafe extern "C" fn get_changes_sync(
    meta_backend: *mut EBookMetaBackend,
    last_sync_tag: *const gchar,
    is_repeat: gboolean,
    out_new_sync_tag: *mut *mut gchar,
    out_repeat: *mut gboolean,
    out_created_objects: *mut *mut GSList,
    out_modified_objects: *mut *mut GSList,
    out_removed_objects: *mut *mut GSList,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "get_changes_sync",
            meta_backend,
            cancellable,
            error,
            |sync| {
                match ops::get_changes(
                    sync,
                    last_sync_tag,
                    out_new_sync_tag,
                    out_repeat,
                    out_created_objects,
                    out_modified_objects,
                    out_removed_objects,
                    error,
                ) {
                    Outcome::Reported => GTRUE,
                    Outcome::Failed => GFALSE,
                    // Nothing has been written and no error set, which is exactly
                    // the state the parent expects to be called in.
                    Outcome::ListInstead => {
                        match parent_class().and_then(|class| class.get_changes_sync) {
                            Some(chain_up) => chain_up(
                                meta_backend,
                                last_sync_tag,
                                is_repeat,
                                out_new_sync_tag,
                                out_repeat,
                                out_created_objects,
                                out_modified_objects,
                                out_removed_objects,
                                cancellable,
                                error,
                            ),
                            // Unreachable against any EDS that has the meta backend at
                            // all, but silently returning TRUE here would be an address
                            // book that stays empty and says nothing.
                            None => fail_offline(error),
                        }
                    }
                }
            },
        )
    }
}

unsafe extern "C" fn load_contact_sync(
    meta_backend: *mut EBookMetaBackend,
    uid: *const gchar,
    _extra: *const gchar,
    out_contact: *mut *mut EContact,
    out_extra: *mut *mut gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "load_contact_sync",
            meta_backend,
            cancellable,
            error,
            |sync| ops::load_contact(sync, uid, out_contact, out_extra, error),
        )
    }
}

#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
unsafe extern "C" fn save_contact_sync(
    meta_backend: *mut EBookMetaBackend,
    overwrite_existing: gboolean,
    // JMAP has no conditional write on `ContactCard/set`, so a conflict is
    // resolved the only way the protocol allows: last writer wins.
    _conflict_resolution: EConflictResolution,
    contact: *mut EContact,
    _extra: *const gchar,
    _opflags: guint32,
    out_new_uid: *mut *mut gchar,
    out_new_extra: *mut *mut gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "save_contact_sync",
            meta_backend,
            cancellable,
            error,
            |sync| {
                ops::save_contact(
                    sync,
                    overwrite_existing,
                    contact,
                    out_new_uid,
                    out_new_extra,
                    error,
                )
            },
        )
    }
}

#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
unsafe extern "C" fn remove_contact_sync(
    meta_backend: *mut EBookMetaBackend,
    _conflict_resolution: EConflictResolution,
    uid: *const gchar,
    _extra: *const gchar,
    _object: *const gchar,
    _opflags: guint32,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "remove_contact_sync",
            meta_backend,
            cancellable,
            error,
            |sync| ops::remove_contact(sync, uid, error),
        )
    }
}

// ---------------------------------------------------------------------------
// the shared shape

/// Runs `f` against the live connection, under a panic guard and the
/// operation's own cancellation.
///
/// The [`observe`] is here rather than repeated at the top of each vfunc so
/// that reaching the connection and being cancellable are the same act: a
/// vfunc added later cannot get the first without the second. It covers the
/// whole of `f`, which is the whole of the operation's network traffic.
///
/// `disconnect_sync` is the one vfunc that deliberately does not go through
/// here. It makes no request — it drops the connection — and dropping it is
/// what the caller asked for whether or not they then pressed Stop; refusing
/// would leave the backend connected to a socket EDS believes is closed.
///
/// # Safety
///
/// `meta_backend` must be NULL or a valid instance of this type, `cancellable`
/// NULL or a valid `GCancellable` that outlives the call, and `error` NULL or a
/// valid, currently-NULL `GError **`.
///
/// [`observe`]: jmap_backend_core::cancel::observe
unsafe fn with_connection(
    context: &str,
    meta_backend: *mut EBookMetaBackend,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
    f: impl FnOnce(&BookSync) -> gboolean,
) -> gboolean {
    unsafe {
        guard_bool(context, error, || {
            let _cancel = observe(cancellable);
            let Some(session) = instance(meta_backend).and_then(JmapBookBackend::session) else {
                return fail_offline(error);
            };
            match read(session).as_ref() {
                Some(sync) => f(sync),
                None => fail_offline(error),
            }
        })
    }
}

/// The Rust view of an instance pointer EDS handed us.
///
/// # Safety
///
/// `meta_backend` must be NULL or point at an instance of this type. EDS only
/// dispatches a class's vfuncs on instances of that class, so the cast is the
/// same one every `G_DEFINE_TYPE` backend makes.
unsafe fn instance<'a>(meta_backend: *mut EBookMetaBackend) -> Option<&'a JmapBookBackend> {
    unsafe { meta_backend.cast::<JmapBookBackend>().as_ref() }
}

/// What an operation with no connection behind it reports.
///
/// `REPOSITORY_OFFLINE` rather than `NOT_OPENED`, because the realistic way to
/// get here is a `disconnect_sync` racing an operation — which is what going
/// offline looks like from inside. Reported that way, `EBookMetaBackend` serves
/// its cache and the user sees their contacts; reported as anything else, they
/// see an error for a state they asked for.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail_offline(error: *mut *mut GError) -> gboolean {
    let message = cstring_lossy("the JMAP address book is not connected");
    // SAFETY: the code is one of the enum's own values and the message is
    // copied by the call; `error` satisfies the contract by this function's.
    unsafe {
        set_raw_gerror(
            error,
            e_client_error_create(E_CLIENT_ERROR_REPOSITORY_OFFLINE, message.as_ptr()),
        );
    }
    GFALSE
}

/// A poisoned lock means an earlier operation panicked inside the guard; the
/// connection itself is untouched by that, so carry on rather than panic in
/// turn — the alternative is an address book that stays broken for the rest of
/// the session.
fn read(session: &RwLock<Option<BookSync>>) -> RwLockReadGuard<'_, Option<BookSync>> {
    session.read().unwrap_or_else(PoisonError::into_inner)
}

fn write(session: &RwLock<Option<BookSync>>) -> RwLockWriteGuard<'_, Option<BookSync>> {
    session.write().unwrap_or_else(PoisonError::into_inner)
}
