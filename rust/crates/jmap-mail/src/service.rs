// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `CamelService` vfuncs: `connect_sync`, `authenticate_sync` and
//! `disconnect_sync`.
//!
//! Three slots, one operation. Camel's division of labour is not the obvious
//! one — a service does *not* open its connection in `connect_sync`. It asks
//! its `CamelSession` to authenticate it, and the session, which is the only
//! object that may touch a stored password or put a prompt in front of the
//! user, calls `authenticate_sync` back: once if the password it had works,
//! once more for every password the user types after that. Every Camel store
//! in tree is built this way (IMAPX and POP3 both call
//! `camel_session_authenticate_sync` from their `connect_sync`), and the reason
//! is the re-prompt: a service that opened its own connection would have
//! nowhere to send the user when the password turned out to be wrong.
//!
//! So `connect_sync` here is a short-circuit and a delegation, and everything
//! that actually happens happens in `authenticate_sync`: read the server off
//! the settings, take the password the session has just put on the service,
//! open the account, install it on the store. Which is why the two functions
//! this module exports — the ones a test can reach without a `CamelSession`,
//! an `EMailSession`, a source registry and a session bus — are exactly those
//! steps minus the GObject: [`authenticate`] and [`report_authentication`].
//!
//! ## The verdict, and when it comes with a `GError`
//!
//! `authenticate_sync` answers twice: a [`CamelAuthenticationResult`] and,
//! optionally, an error. The two are not independent.
//! `camel_session_authenticate_sync` reads `REJECTED` as "ask the user for
//! another password and call me again" and keeps looping; it only gives up, and
//! only propagates a `GError`, on `ERROR`. An error set alongside a `REJECTED`
//! is therefore reported for an attempt that has not failed yet — in the best
//! case leaked, in the worst case shown to a user who is being asked for a
//! password at the same time. [`report_authentication`] is where that rule
//! lives, and it is the only place either answer is produced.

use std::ptr;

use eds_sys::{
    CAMEL_AUTHENTICATION_ERROR, CAMEL_SERVICE_ERROR_INVALID, CamelAuthenticationResult,
    CamelService, CamelServiceClass, camel_service_error_quark, camel_service_get_password,
    camel_service_ref_session, camel_service_ref_settings, camel_session_authenticate_sync,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, g_error_new_literal, gboolean, gchar};
use gobject_sys::{GObject, g_object_unref, g_type_class_peek};
use jmap_backend_core::cancel::CancelBridge;
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::guard_bool;
use jmap_client::CancelFlag;

use crate::connect::{ACCEPTED_AUTHENTICATION, StoreError, open_mail};
use crate::server::ServerConfig;
use crate::store::JmapStore;

/// Opens the account `config` names and installs it on `store`.
///
/// The body of `authenticate_sync`, with the GObject taken out: everything
/// above it is reading the two arguments off a `CamelService`, everything below
/// it is [`open_mail`].
///
/// A failure leaves the store exactly as it was, including a connection that
/// was working — Camel re-authenticates a service it already has one for (a
/// password change, a session that lost track of the account), and a store that
/// dropped its connection on the way to being told the new password would stop
/// serving folders it was serving a moment ago. A *success* does replace it,
/// listing and all, because the account was re-authenticated for a reason and
/// the tree the old connection produced may describe a different server.
pub fn authenticate(
    store: &JmapStore,
    config: &ServerConfig,
    password: Option<&str>,
    cancel: CancelFlag,
) -> Result<(), StoreError> {
    let sync = open_mail(config, password, cancel)?;
    store.store_connection(sync);
    Ok(())
}

/// Turns one attempt into the two answers `authenticate_sync` returns.
///
/// Takes ownership of the failure and, where there is one, hands the `GError`
/// it becomes to `error` — or frees it, if the caller did not ask for one.
///
/// The rule this function exists for: only an `ERROR` verdict carries an error.
/// `ACCEPTED` has nothing to report, and `REJECTED` is not a failure Camel is
/// being told about — it is a request for another password, which the session
/// answers by prompting and calling back. See the module docs.
///
/// # Safety
///
/// `error` must be NULL or point at a writable, currently-NULL `GError *`.
pub unsafe fn report_authentication(
    outcome: Result<(), StoreError>,
    error: *mut *mut GError,
) -> CamelAuthenticationResult {
    let Err(failure) = outcome else {
        return ACCEPTED_AUTHENTICATION;
    };

    let result = failure.authentication_result();
    if result == CAMEL_AUTHENTICATION_ERROR {
        // SAFETY: `to_gerror` hands over an owned GError, and `error` meets
        // `set_raw_gerror`'s contract by this function's.
        unsafe { set_raw_gerror(error, failure.to_gerror()) };
    }
    result
}

// ---------------------------------------------------------------------------
// the vfunc slots

/// Installs the three service vfuncs on a class whose first member is a
/// `CamelServiceClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelServiceClass` — which is every descendant of `CamelService`.
pub unsafe fn install_vfuncs(class: *mut CamelServiceClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.connect_sync = Some(connect_sync);
    vfuncs.authenticate_sync = Some(authenticate_sync);
    vfuncs.disconnect_sync = Some(disconnect_sync);
}

/// Asks the session to authenticate this service, unless it already did.
///
/// No connection is opened here — see the module docs. The short-circuit is the
/// same one the address book backend's `connect_sync` makes and for the same
/// reason: Camel reconnects a service whenever it suspects the connection is
/// gone, including when it is not, and re-opening a live one would drop a
/// socket other threads are mid-request on.
unsafe extern "C" fn connect_sync(
    service: *mut CamelService,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NULL-or-valid GCancellable, and an out-parameter that is NULL or
    // writable and currently NULL.
    unsafe {
        guard_bool("connect_sync", error, || {
            let Some(store) = instance(service) else {
                return fail_disconnected(error);
            };
            if store.is_connected() {
                return GTRUE;
            }

            // The one object allowed to read a stored password or prompt for a
            // new one. A service without a session is a service nothing can
            // authenticate, which is the same dead end as having no store.
            let session = camel_service_ref_session(service);
            if session.is_null() {
                return fail_disconnected(error);
            }

            // NULL mechanism: JMAP authenticates over HTTP and offers no SASL
            // mechanisms to pick between, which is also why
            // `query_auth_types_sync` is left alone.
            let connected =
                camel_session_authenticate_sync(session, service, ptr::null(), cancellable, error);
            g_object_unref(session.cast::<GObject>());
            connected
        })
    }
}

/// Opens the account, with the password the session has just put on the
/// service.
unsafe extern "C" fn authenticate_sync(
    service: *mut CamelService,
    // Ignored, and NULL on every call this code provokes: `connect_sync` passes
    // none and nothing advertises any.
    _mechanism: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> CamelAuthenticationResult {
    // A panic is the one outcome `report_authentication` cannot classify: it is
    // a bug in this code rather than an account that failed, so it is reported
    // separately rather than dressed up as a service error the user could act
    // on.
    let outcome = jmap_backend_core::trampoline::guard("authenticate_sync", None, || {
        // SAFETY: Camel's contract for the vfunc, as `connect_sync`'s.
        Some(unsafe { attempt(service, cancellable) })
    });

    match outcome {
        // SAFETY: `error` meets the contract by this vfunc's.
        Some(outcome) => unsafe { report_authentication(outcome, error) },
        None => unsafe { fail_internal(error) },
    }
}

/// Everything `authenticate_sync` does between the guard and the verdict.
///
/// # Safety
///
/// As the vfunc: `service` must be an instance of this type, `cancellable`
/// NULL or valid.
unsafe fn attempt(
    service: *mut CamelService,
    cancellable: *mut GCancellable,
) -> Result<(), StoreError> {
    let store = unsafe { instance(service) }.ok_or(StoreError::Disconnected)?;

    // Camel hands out a reference; the config is read before it is given back,
    // and nothing borrowed from the settings outlives the call.
    // SAFETY: `service` is a valid CamelService, so the settings it returns are
    // a valid CamelSettings — of the class `settings_type` names, which is what
    // `from_settings` requires.
    let settings = unsafe { camel_service_ref_settings(service) };
    let config = unsafe { ServerConfig::from_settings(settings) };
    if !settings.is_null() {
        // SAFETY: the reference `ref_settings` handed over.
        unsafe { g_object_unref(settings.cast::<GObject>()) };
    }
    let config = config?;

    // The session put it there before calling us, and it is the only credential
    // this code ever sees: nothing reads a password out of the settings object,
    // which Evolution serialises into a config file.
    // SAFETY: a borrowed, NULL-or-NUL-terminated string owned by the service;
    // `read_string` copies what it needs.
    let password = unsafe { read_string(camel_service_get_password(service)) };

    // SAFETY: the cancellable is Camel's, and it outlives the call — which is
    // exactly the scope of the bridge.
    let cancel = unsafe { CancelBridge::new(cancellable) };
    authenticate(store, &config, password.as_deref(), cancel.flag().clone())
}

/// Drops the connection, then lets `CamelService` do its half.
///
/// Ours goes first: the parent's `disconnect_sync` is what marks the service
/// disconnected, and a connection still in the slot after that is one a racing
/// operation could pick up and use against a service Camel believes is closed.
/// Dropping one that is not there is not a failure — it is what Camel asks of
/// every service on shutdown, whether or not it ever connected.
unsafe extern "C" fn disconnect_sync(
    service: *mut CamelService,
    clean: gboolean,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        guard_bool("disconnect_sync", error, || {
            if let Some(store) = instance(service) {
                store.drop_connection();
            }
            match parent_service_class().and_then(|class| class.disconnect_sync) {
                Some(chain_up) => chain_up(service, clean, cancellable, error),
                // Unreachable against any Camel that has a CamelService at all,
                // and there is nothing left for this side to do either way.
                None => GTRUE,
            }
        })
    }
}

/// The Rust view of the instance pointer Camel handed us.
///
/// # Safety
///
/// `service` must be NULL or point at an instance of [`JmapStore`]. Camel only
/// dispatches a class's vfuncs on instances of that class, so the cast is the
/// same one every `G_DEFINE_TYPE` store makes.
unsafe fn instance<'a>(service: *mut CamelService) -> Option<&'a JmapStore> {
    unsafe { service.cast::<JmapStore>().as_ref() }
}

/// `CamelOfflineStore`'s class, as the `CamelServiceClass` it leads with, for
/// the one vfunc that chains up.
///
/// `g_type_class_peek` rather than `_ref`: an initialised parent class is what
/// having registered a subclass of it guarantees, and taking a reference here
/// would mean giving one back on a path that has no natural place to do so.
fn parent_service_class() -> Option<&'static CamelServiceClass> {
    // SAFETY: peeking a type nothing has referenced returns NULL, which is
    // handled; otherwise the class outlives the type, which for a Camel type is
    // the life of the process. `CamelOfflineStoreClass` leads with
    // `CamelStoreClass`, which leads with `CamelServiceClass`.
    unsafe {
        g_type_class_peek(<JmapStore as ObjectSubclass>::parent_type())
            .cast::<CamelServiceClass>()
            .as_ref()
    }
}

/// What a vfunc with no store behind it reports: the code that makes Camel
/// connect and ask again rather than showing the account as broken.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail_disconnected(error: *mut *mut GError) -> gboolean {
    // SAFETY: an owned GError, and `error` meets the contract by this
    // function's.
    unsafe { set_raw_gerror(error, StoreError::Disconnected.to_gerror()) };
    GFALSE
}

/// What a panicked `authenticate_sync` reports. `INVALID` rather than
/// `UNAVAILABLE`: the server is not the problem, and telling Camel it is
/// unreachable would have it retry the account forever over a bug that is
/// deterministic. The critical the guard logged is where the detail is.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail_internal(error: *mut *mut GError) -> CamelAuthenticationResult {
    // SAFETY: a live quark and a NUL-terminated literal the call copies.
    let gerror = unsafe {
        g_error_new_literal(
            camel_service_error_quark(),
            CAMEL_SERVICE_ERROR_INVALID as i32,
            c"internal error in the JMAP provider".as_ptr(),
        )
    };
    // SAFETY: an owned GError, and `error` meets the contract by this
    // function's.
    unsafe { set_raw_gerror(error, gerror) };
    CAMEL_AUTHENTICATION_ERROR
}
