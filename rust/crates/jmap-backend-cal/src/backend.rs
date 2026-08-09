// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECalMetaBackend` subclass itself.
//!
//! Everything it does has already been written and tested one layer down —
//! [`connect`] opens the connection, [`ops`] is the vfunc bodies,
//! [`marshal`](crate::marshal) the C conversions. What is left here, and only
//! here, is the part that cannot be tested against a live instance: the
//! instance and class structs, the seven vfunc slots, and the lookup of the
//! connection an instance holds.
//!
//! So the code below is deliberately dull, and it is the address book's
//! `backend.rs` almost line for line. The two are *not* factored together:
//! what looks shared is the shape, not the code — every signature below names
//! `ECalMetaBackend`, `ICalComponent` and the calendar's own class struct, and
//! there is no vfunc slot the two backends could both be installed into. The
//! decisions those slots make are what `jmap-backend-core` already holds.
//!
//! The one exception to the dullness is `get_changes_sync`, which has a third
//! answer — chain up to `ECalMetaBackend`'s own implementation — and that
//! chain-up needs the parent class pointer, which is why it could not live in
//! `ops` with the decision that produces it.
//!
//! ## What is left to the parent
//!
//! `search_sync` and `search_components_sync` stay `ECalMetaBackend`'s.
//! It answers a query by running the S-expression over the offline cache, which
//! for a calendar it has just synced is a complete answer; JMAP's
//! `CalendarEvent/query` cannot express an S-expression, so anything installed
//! here would be a narrower filter replacing a working one.
//!
//! ## What is not wired up yet
//!
//! Cancellation reaches the *connect*, which is the operation that blocks
//! longest, but not the ones after it: a `GCancellable` handed to
//! `list_existing_sync` is still observed by nobody. The mechanism it needs now
//! exists — [`observe`] installs a cancellable for the length of one operation
//! and the client honours it in preference to whatever it was built with, which
//! is what the Camel mail provider's vfuncs do — so what is left here is one
//! line at the top of each vfunc, and the tests to go with them. Until then the
//! flag this backend's client was built with is what answers, which also means
//! a connect the user cancelled leaves a client that refuses everything
//! afterwards.
//!
//! [`observe`]: jmap_backend_core::cancel::observe

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eds_sys::{
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, ECalMetaBackend, ECalMetaBackendClass, ECalOperationFlags,
    EConflictResolution, ENamedParameters, ESourceAuthenticationResult, GTlsCertificateFlags,
    ICalComponent, e_backend_get_source, e_cal_meta_backend_get_type,
    e_cal_meta_backend_set_connected_writable, e_client_error_create,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GSList, GTRUE, GType, gboolean, gchar};
use gobject_sys::g_type_class_peek;
use jmap_backend_core::error::{cstring_lossy, set_raw_gerror};
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::guard_bool;
use jmap_cal_sync::CalSync;

use crate::connect::{self, ACCEPTED_AUTH_RESULT, write_auth_result};
use crate::ops::{self, Outcome};

/// The JMAP calendar backend.
///
/// The name is the one EDS will look the factory's product up by, and it
/// follows the convention every other backend uses (`ECalBackendEws`,
/// `ECalBackendCalDAV`) because the module and factory names are derived from
/// it by hand in the next layer up.
#[repr(C)]
pub struct JmapCalBackend {
    /// GObject's; never read by this code, only handed back to EDS as the
    /// instance pointer it gave us.
    parent: ECalMetaBackend,
    /// The connection, from `connect_sync` to `disconnect_sync`.
    ///
    /// An `RwLock` rather than a `Mutex`: EDS calls the read-only vfuncs from
    /// several threads at once, and serialising them behind one lock would
    /// make a long `list_existing_sync` block every `load_component_sync`
    /// behind it. Only connect and disconnect, which replace the value, need
    /// exclusive access.
    session: Slot<RwLock<Option<CalSync>>>,
}

/// The class struct. Nothing of ours lives in it yet; it exists because
/// GObject needs a size to allocate and a place to put the vfunc slots, and
/// because the factory in the next increment will want to reach it.
#[repr(C)]
pub struct JmapCalBackendClass {
    pub parent_class: ECalMetaBackendClass,
}

impl JmapCalBackend {
    /// Installs `sync` as the live connection, replacing whatever was there.
    ///
    /// Replacing rather than refusing: `connect_sync` is reached again after a
    /// `requires_reconnect`, and the old connection is exactly what is being
    /// replaced. It is dropped — and its socket closed — when this returns.
    pub fn store_connection(&self, sync: CalSync) {
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
    fn session(&self) -> Option<&RwLock<Option<CalSync>>> {
        self.session.get()
    }
}

// SAFETY: both structs are #[repr(C)] and lead with the ECalMetaBackend
// instance and class structs respectively, and ECalMetaBackend derives from
// GObject (via ECalBackendSync, ECalBackend and EBackend).
unsafe impl ObjectSubclass for JmapCalBackend {
    const NAME: &'static CStr = c"ECalBackendJmap";
    type Instance = JmapCalBackend;
    type Class = JmapCalBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the EDS type system initialises itself.
        unsafe { e_cal_meta_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; the slots below are all in that half.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.connect_sync = Some(connect_sync);
        vfuncs.disconnect_sync = Some(disconnect_sync);
        vfuncs.list_existing_sync = Some(list_existing_sync);
        vfuncs.get_changes_sync = Some(get_changes_sync);
        vfuncs.load_component_sync = Some(load_component_sync);
        vfuncs.save_component_sync = Some(save_component_sync);
        vfuncs.remove_component_sync = Some(remove_component_sync);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points at a zeroed instance struct of ours, and a
        // zeroed `Slot` is an empty one.
        unsafe { (*instance).session.init(RwLock::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the
        // connection — and its socket — outlives the calendar.
        unsafe { (*instance).session.clear() };
    }
}

/// The parent's class struct, for the one vfunc that chains up.
///
/// `g_type_class_peek` rather than `_ref`: an initialised class is what having
/// registered a subclass of it guarantees, and taking a reference here would
/// mean giving one back on a path that has no natural place to do so.
pub fn parent_class() -> Option<&'static ECalMetaBackendClass> {
    // SAFETY: peeking a type that has never been referenced returns NULL,
    // which is handled; otherwise the class is alive for as long as the type
    // is, which for an EDS type is the life of the process.
    unsafe {
        g_type_class_peek(<JmapCalBackend as ObjectSubclass>::parent_type())
            .cast::<ECalMetaBackendClass>()
            .as_ref()
    }
}

// ---------------------------------------------------------------------------
// the vfunc slots

unsafe extern "C" fn connect_sync(
    meta_backend: *mut ECalMetaBackend,
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
            // Without this the calendar is read-only in the UI: it is how
            // `ECalMetaBackend` decides whether a connected backend accepts
            // writes. JMAP has no per-calendar "may I write" flag, so the
            // answer is the same as the account's.
            e_cal_meta_backend_set_connected_writable(meta_backend, GTRUE);
            GTRUE
        })
    }
}

unsafe extern "C" fn disconnect_sync(
    meta_backend: *mut ECalMetaBackend,
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
    meta_backend: *mut ECalMetaBackend,
    out_new_sync_tag: *mut *mut gchar,
    out_existing_objects: *mut *mut GSList,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection("list_existing_sync", meta_backend, error, |sync| {
            ops::list_existing(sync, out_new_sync_tag, out_existing_objects, error)
        })
    }
}

#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
unsafe extern "C" fn get_changes_sync(
    meta_backend: *mut ECalMetaBackend,
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
        with_connection("get_changes_sync", meta_backend, error, |sync| {
            match ops::get_changes(
                sync,
                last_sync_tag,
                is_repeat,
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
                        // Unreachable against any EDS that has the meta backend
                        // at all, but silently returning TRUE here would be a
                        // calendar that stays empty and says nothing.
                        None => fail_offline(error),
                    }
                }
            }
        })
    }
}

unsafe extern "C" fn load_component_sync(
    meta_backend: *mut ECalMetaBackend,
    uid: *const gchar,
    _extra: *const gchar,
    out_component: *mut *mut ICalComponent,
    out_extra: *mut *mut gchar,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection("load_component_sync", meta_backend, error, |sync| {
            ops::load_component(sync, uid, out_component, out_extra, error)
        })
    }
}

#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
unsafe extern "C" fn save_component_sync(
    meta_backend: *mut ECalMetaBackend,
    overwrite_existing: gboolean,
    // JMAP can express a conditional write as a `CalendarEvent/set` `ifInState`,
    // but `CalSync` does not send one, so a conflict is resolved the only way
    // this backend can: last writer wins.
    _conflict_resolution: EConflictResolution,
    instances: *const GSList,
    _extra: *const gchar,
    // iTIP scheduling requests, which this milestone does not implement.
    _opflags: ECalOperationFlags,
    out_new_uid: *mut *mut gchar,
    out_new_extra: *mut *mut gchar,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`, and `instances` is EDS's own list of
    // `ECalComponent *`.
    unsafe {
        with_connection("save_component_sync", meta_backend, error, |sync| {
            ops::save_component(
                sync,
                overwrite_existing,
                instances,
                out_new_uid,
                out_new_extra,
                error,
            )
        })
    }
}

#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
unsafe extern "C" fn remove_component_sync(
    meta_backend: *mut ECalMetaBackend,
    _conflict_resolution: EConflictResolution,
    uid: *const gchar,
    _extra: *const gchar,
    _object: *const gchar,
    _opflags: ECalOperationFlags,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection("remove_component_sync", meta_backend, error, |sync| {
            ops::remove_component(sync, uid, error)
        })
    }
}

// ---------------------------------------------------------------------------
// the shared shape

/// Runs `f` against the live connection, under a panic guard.
///
/// # Safety
///
/// `meta_backend` must be NULL or a valid instance of this type, and `error`
/// NULL or a valid, currently-NULL `GError **`.
unsafe fn with_connection(
    context: &str,
    meta_backend: *mut ECalMetaBackend,
    error: *mut *mut GError,
    f: impl FnOnce(&CalSync) -> gboolean,
) -> gboolean {
    unsafe {
        guard_bool(context, error, || {
            let Some(session) = instance(meta_backend).and_then(JmapCalBackend::session) else {
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
unsafe fn instance<'a>(meta_backend: *mut ECalMetaBackend) -> Option<&'a JmapCalBackend> {
    unsafe { meta_backend.cast::<JmapCalBackend>().as_ref() }
}

/// What an operation with no connection behind it reports.
///
/// `REPOSITORY_OFFLINE` rather than `NOT_OPENED`, because the realistic way to
/// get here is a `disconnect_sync` racing an operation — which is what going
/// offline looks like from inside. Reported that way, `ECalMetaBackend` serves
/// its cache and the user sees their appointments; reported as anything else,
/// they see an error for a state they asked for.
///
/// The client error domain rather than the calendar's own: this is not a
/// statement about a component, and `E_CAL_CLIENT_ERROR` has no offline code to
/// make it in.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail_offline(error: *mut *mut GError) -> gboolean {
    let message = cstring_lossy("the JMAP calendar is not connected");
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
/// turn — the alternative is a calendar that stays broken for the rest of the
/// session.
fn read(session: &RwLock<Option<CalSync>>) -> RwLockReadGuard<'_, Option<CalSync>> {
    session.read().unwrap_or_else(PoisonError::into_inner)
}

fn write(session: &RwLock<Option<CalSync>>) -> RwLockWriteGuard<'_, Option<CalSync>> {
    session.write().unwrap_or_else(PoisonError::into_inner)
}
