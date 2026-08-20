// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECalMetaBackend` subclass itself.
//!
//! Everything it does has already been written and tested one layer down —
//! [`connect`] opens the connection, [`ops`] is the vfunc bodies,
//! [`marshal`](crate::marshal) the C conversions. What is left here, and only
//! here, is the part that cannot be tested against a live instance: the
//! instance and class structs, the eight vfunc slots, and the lookup of the
//! connection an instance holds.
//!
//! So the code below is deliberately dull, and it is the address book's
//! `backend.rs` almost line for line. The two are *not* factored together:
//! what looks shared is the shape, not the code — every signature below names
//! `ECalMetaBackend`, `ICalComponent` and the calendar's own class struct, and
//! there is no vfunc slot the two backends could both be installed into. The
//! decisions those slots make are what `jmap-backend-core` already holds.
//!
//! The exceptions to the dullness are the two vfuncs that have a third answer
//! — chain up to `ECalMetaBackend`'s own implementation — because that chain-up
//! needs the parent class pointer, which is why neither could live in `ops`
//! with the decision that produces it. `get_changes_sync` chains up to have the
//! whole calendar listed and diffed against the cache; `get_free_busy_sync`
//! chains up to have the account owner's own busy times computed from it.
//!
//! `get_free_busy_sync` is also the one slot here that is not an
//! `ECalMetaBackend` one: it is declared two classes up, on `ECalBackendSync`,
//! and has to be installed there — see its own comment.
//!
//! ## What is left to the parent
//!
//! `search_sync` and `search_components_sync` stay `ECalMetaBackend`'s.
//! It answers a query by running the S-expression over the offline cache, which
//! for a calendar it has just synced is a complete answer; JMAP's
//! `CalendarEvent/query` cannot express an S-expression, so anything installed
//! here would be a narrower filter replacing a working one.
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
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eds_sys::{
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, ECalBackendSync, ECalMetaBackend, ECalMetaBackendClass,
    ECalOperationFlags, EConflictResolution, EDataCal, ENamedParameters,
    ESourceAuthenticationResult, GTlsCertificateFlags, ICalComponent, e_backend_get_source,
    e_cal_backend_set_writable, e_cal_meta_backend_get_type, e_client_error_create, time_t,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GSList, GTRUE, GType, gboolean, gchar};
use gobject_sys::g_type_class_peek;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{cstring_lossy, set_raw_gerror};
use jmap_backend_core::instance::Slot;
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, guard_bool, guard_value};
use jmap_cal_sync::CalSync;

use crate::connect::{self, ACCEPTED_AUTH_RESULT, write_auth_result};
use crate::marshal;
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
    /// The colour last read from, or pushed to, the server — `source_changed`'s
    /// baseline. `source_changed` is EDS's own single-flight, worker-thread
    /// signal (see the module comment), never a concurrent one, so a plain
    /// `RwLock` (not contended, but matching `session`'s discipline rather than
    /// introducing a second one) is enough.
    last_known_color: Slot<RwLock<Option<String>>>,
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
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value, and an all-zero `Slot` is its
        // documented empty state.
        let backend: Box<Self> = unsafe { zeroed_box() };
        backend.session.init(RwLock::new(None));
        backend.last_known_color.init(RwLock::new(None));
        backend
    }

    /// The connection slot, or `None` on an instance whose `instance_init` has
    /// not run or whose `finalize` already has.
    fn session(&self) -> Option<&RwLock<Option<CalSync>>> {
        self.session.get()
    }

    /// The `source_changed` baseline slot, or `None` under the same
    /// circumstances as [`JmapCalBackend::session`].
    fn last_known_color(&self) -> Option<&RwLock<Option<String>>> {
        self.last_known_color.get()
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
        vfuncs.source_changed = Some(source_changed);

        // Two levels up rather than one. `get_free_busy_sync` is declared on
        // `ECalBackendSyncClass`, which `ECalMetaBackend` merely fills in like
        // any other subclass — and `e_cal_backend_sync_get_free_busy` looks it
        // up there. Written into the `ECalMetaBackendClass` half instead it
        // would compile, install, and never once be called.
        vfuncs.parent_class.get_free_busy_sync = Some(get_free_busy_sync);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points at a zeroed instance struct of ours, and a
        // zeroed `Slot` is an empty one.
        unsafe {
            (*instance).session.init(RwLock::new(None));
            (*instance).last_known_color.init(RwLock::new(None));
        }
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the
        // connection — and its socket — outlives the calendar.
        unsafe {
            (*instance).session.clear();
            (*instance).last_known_color.clear();
        }
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
            // Without this the calendar is read-only: every write comes back
            // as "Permission denied" and Evolution greys the calendar out.
            // JMAP has no per-calendar "may I write" flag, so the answer is
            // the same as the account's.
            //
            // This is `e_cal_backend_set_writable` and not the meta backend's
            // `set_connected_writable`, which reads like the one to call and
            // is not — the same trap the address book's `connect_sync` walks
            // past for the same reason: the moment this vfunc returns TRUE,
            // EDS's `ecmb_update_connection_values` *overwrites*
            // connected-writable with `e_cal_backend_get_writable()`, so the
            // meta backend's setter is undone by the very call that was about
            // to read it. Setting the backend's flag sets both, and
            // connected-writable is what EDS restores the backend's flag from
            // when it opens the calendar offline. What the vfunc's own
            // documentation asks for.
            e_cal_backend_set_writable(meta_backend.cast(), GTRUE);
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
        with_connection(
            "get_changes_sync",
            meta_backend,
            cancellable,
            error,
            |sync| {
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
            },
        )
    }
}

unsafe extern "C" fn load_component_sync(
    meta_backend: *mut ECalMetaBackend,
    uid: *const gchar,
    _extra: *const gchar,
    out_component: *mut *mut ICalComponent,
    out_extra: *mut *mut gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "load_component_sync",
            meta_backend,
            cancellable,
            error,
            |sync| ops::load_component(sync, uid, out_component, out_extra, error),
        )
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
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`, and `instances` is EDS's own list of
    // `ECalComponent *`.
    unsafe {
        with_connection(
            "save_component_sync",
            meta_backend,
            cancellable,
            error,
            |sync| {
                // The backend *is* the calendar's timezone cache: `ECalBackend`
                // implements `ETimezoneCache`, and a `VTIMEZONE` a client sent
                // is filed there rather than in the component. So the zone of
                // an appointment that came from an invitation is reachable
                // exactly here, and nowhere further down.
                ops::save_component(
                    sync,
                    overwrite_existing,
                    instances,
                    meta_backend.cast(),
                    out_new_uid,
                    out_new_extra,
                    error,
                )
            },
        )
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
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "remove_component_sync",
            meta_backend,
            cancellable,
            error,
            |sync| ops::remove_component(sync, uid, error),
        )
    }
}

/// Pushes a local colour edit — `source_changed`.
///
/// EDS already does the work the module comment describes as the hard part:
/// this is called on `ECalMetaBackend`'s own dedicated worker thread,
/// single-flight-guarded against a second firing while one is in flight, for
/// every change to the account's `ESource` — not just the colour, and not
/// just once per real edit, since the periodic refresh EDS schedules on top
/// of it fires it again regardless. [`ops::on_source_changed`] is what turns
/// that into "push only a genuine difference", against a baseline held in
/// this instance rather than recomputed from nothing each time.
///
/// No connection is not a failure here — unlike every other vfunc in this
/// file there is nothing to report back to EDS (the slot is `void`), and a
/// disconnected account's next successful connect starts this baseline over
/// from whatever colour that reconnect's own populate leaves on the
/// `ESource`, which is the same "diff from what is there" rule this vfunc
/// always follows.
///
/// # Safety
///
/// `meta_backend` must be NULL or a valid instance of this type, whose
/// `ESource` carries a "Calendar" extension — `ECalMetaBackend` never calls
/// this before `connect_sync` has succeeded once, by which point this
/// backend's own `child_source` write path has already created it.
unsafe extern "C" fn source_changed(meta_backend: *mut ECalMetaBackend) {
    // SAFETY: EDS's own contract for the vfunc: a valid instance of ours.
    unsafe {
        guard("source_changed", (), || {
            let Some(backend) = instance(meta_backend) else {
                return;
            };
            let Some(session) = backend.session() else {
                return;
            };
            let Some(baseline_slot) = backend.last_known_color() else {
                return;
            };

            let guard = read(session);
            let Some(sync) = guard.as_ref() else {
                return;
            };

            let source = e_backend_get_source(meta_backend.cast());
            // SAFETY: `source` is the `ESource` of a valid instance, per this
            // function's own safety contract.
            let current = marshal::selectable_color(source);

            let mut baseline = write(baseline_slot);
            match ops::on_source_changed(sync, current.as_deref(), baseline.as_deref()) {
                ops::ColorOutcome::Unchanged | ops::ColorOutcome::Failed => {}
                ops::ColorOutcome::Pushed(pushed) => *baseline = pushed,
            }
        });
    }
}

/// The one vfunc here that is not an `ECalMetaBackend` slot, does not return a
/// `gboolean`, and does not go through [`with_connection`].
///
/// All three follow from the same thing: it has a useful answer even with no
/// connection. `ECalMetaBackend`'s own implementation computes the account
/// owner's busy times from the offline cache, so "not connected" is a reason to
/// chain up, not to report `REPOSITORY_OFFLINE` — which is what
/// [`with_connection`] would do, and which would leave the meeting editor with
/// nothing where it could have had the organiser's own diary. The CalDAV
/// backend arranges the same fallback the same way.
///
/// Being `void` is why the guard is [`guard_value`] with a `()` fallback: a
/// panic still cannot cross into C, and still sets `error`, but there is no
/// return value to turn into a failure.
#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
unsafe extern "C" fn get_free_busy_sync(
    backend: *mut ECalBackendSync,
    cal: *mut EDataCal,
    cancellable: *mut GCancellable,
    users: *const GSList,
    start: time_t,
    end: time_t,
    out_freebusy: *mut *mut GSList,
    error: *mut *mut GError,
) {
    // SAFETY: EDS's own contract for the vfunc: a valid instance of ours, a
    // NULL-or-valid GCancellable and user list, and out-parameters that are
    // NULL or writable.
    unsafe {
        guard_value("get_free_busy_sync", error, (), || {
            let _cancel = observe(cancellable);
            // The read guard is scoped so it is released before the chain-up:
            // the parent goes off into the EDS cache, and holding this
            // backend's connection lock across that would block a concurrent
            // connect or disconnect for no reason.
            let outcome = {
                let session = instance(backend.cast()).and_then(JmapCalBackend::session);
                let guard = session.map(read);
                match guard.as_deref().and_then(Option::as_ref) {
                    Some(sync) => ops::get_free_busy(sync, users, start, end, out_freebusy, error),
                    None => ops::FreeBusyOutcome::NothingKnown,
                }
            };

            if matches!(outcome, ops::FreeBusyOutcome::NothingKnown)
                && let Some(chain_up) =
                    parent_class().and_then(|class| class.parent_class.get_free_busy_sync)
            {
                // Nothing has been written and no error set, which is the state
                // the parent expects to be called in — and it must be, since it
                // will `g_set_error` into the same `GError **`.
                chain_up(
                    backend,
                    cal,
                    cancellable,
                    users,
                    start,
                    end,
                    out_freebusy,
                    error,
                );
            }
        })
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
    meta_backend: *mut ECalMetaBackend,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
    f: impl FnOnce(&CalSync) -> gboolean,
) -> gboolean {
    unsafe {
        guard_bool(context, error, || {
            let _cancel = observe(cancellable);
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
/// value it guards is untouched by that, so carry on rather than panic in
/// turn — the alternative is a calendar that stays broken for the rest of the
/// session. Generic over both slots this file keeps behind an `RwLock`: the
/// connection and the colour baseline.
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}
