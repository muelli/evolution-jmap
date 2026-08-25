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
//!
//! ## Two services, one set of vfuncs
//!
//! Camel gives an account two services: the [`crate::store`] it reads from and
//! the [`crate::transport`] it sends through. Both are `CamelService`s, both are
//! configured through the same settings class, and both connect by exactly the
//! steps above — so the vfuncs are written once here and installed on both, over
//! the [`Connected`] trait that says which of the two the instance pointer is.
//! What the trait carries is the only thing that differs: where the connection
//! that opening the account produced is put.
//!
//! ## And the fourth vfunc, which is not about connecting
//!
//! `get_name` is what Camel calls the account in its own sentences — "Cannot
//! get folder … from store …", the progress the user watches, the line an error
//! dialog puts the failure on. `CamelService` provides no default for it: the
//! accessor is a `g_return_val_if_fail (class->get_name != NULL, NULL)`, so a
//! store that leaves the slot empty answers NULL and logs a critical every time
//! Camel mentions it. [`describe`] is the answer, and it is a pure function of
//! the three fields the settings carry so that it can be tested as one.

use std::ptr;

use eds_sys::{
    CAMEL_AUTHENTICATION_ERROR, CAMEL_SERVICE_ERROR_INVALID, CamelAuthenticationResult,
    CamelService, CamelServiceClass, camel_network_settings_dup_host,
    camel_network_settings_dup_user, camel_network_settings_get_port, camel_service_error_quark,
    camel_service_get_password, camel_service_ref_session, camel_service_ref_settings,
    camel_session_authenticate_sync,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, g_error_new_literal, gboolean, gchar};
use gobject_sys::g_type_class_peek;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::{dup_string, read_string};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::guard_bool;
use jmap_client::Credentials;
use jmap_mail_sync::MailSync;

use crate::api_token;
use crate::connect::{
    ACCEPTED_AUTHENTICATION, StoreError, bearer_credentials, open_mail, password_credentials,
};
use crate::oauth2;
use crate::server::{ServerConfig, network, take_string};

/// A service of this provider that holds a JMAP connection: the store, and the
/// transport.
///
/// The three methods are the whole of what the vfuncs below need to know about
/// the instance they were dispatched on. Everything else — reading the settings,
/// asking the session to authenticate, opening the account, classifying the
/// failure — is the same operation on either service, and is written once.
///
/// The connection is *held* rather than returned because the two services keep
/// it differently: the store drops its folder listing with it, and a later
/// increment's transport may keep more beside it. What neither may do is share
/// one, and the trait is why that is not even expressible here — each
/// implementer names its own slot.
///
/// # Safety
///
/// The instance struct of the `GType` an implementer registers must be `Self`,
/// and that type must derive from `CamelService`: the vfuncs are dispatched on
/// a `CamelService *` and read `Self` straight through it, which is the same
/// cast every `G_DEFINE_TYPE` service makes.
pub unsafe trait Connected: ObjectSubclass<Instance = Self> {
    /// Installs `sync` as the live connection, replacing whatever was there.
    fn hold_connection(&self, sync: MailSync);
    /// Drops the connection, if there is one. Not having one is not a failure:
    /// Camel disconnects every service on shutdown, connected or not.
    fn release_connection(&self);
    /// Whether an operation would find a connection.
    fn holds_connection(&self) -> bool;
}

/// Opens the account `config` names and installs it on `service`.
///
/// The body of `authenticate_sync`, with the GObject taken out: everything
/// above it is reading the two arguments off a `CamelService`, everything below
/// it is [`open_mail`].
///
/// A failure leaves the service exactly as it was, including a connection that
/// was working — Camel re-authenticates a service it already has one for (a
/// password change, a session that lost track of the account), and a store that
/// dropped its connection on the way to being told the new password would stop
/// serving folders it was serving a moment ago. A *success* does replace it,
/// listing and all, because the account was re-authenticated for a reason and
/// the tree the old connection produced may describe a different server.
///
/// Cancellation is the caller's, and is not passed: the connection this opens
/// outlives the call that opened it, so a flag built into it would be one this
/// operation could set and no later operation could unset. What stops the
/// connect is the scope the vfunc installed — see [`observe`] — which is also
/// what stops every operation afterwards.
pub fn authenticate<T: Connected>(
    service: &T,
    config: &ServerConfig,
    credentials: Credentials,
) -> Result<(), StoreError> {
    let sync = open_mail(config, credentials)?;
    service.hold_connection(sync);
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

/// What Camel calls this account, in its two documented forms.
///
/// `brief` is the one Camel asks for when the name goes in a folder tree or in
/// the middle of one of its own sentences, so it is the server and nothing
/// else; the other is documented as "complete and mostly unambiguous", which is
/// what the user and the port are here for. JMAP is HTTP and a JMAP account is
/// therefore quite normally one of several on a host — a local server, a test
/// one — so a port the account names belongs in the form whose job is to tell
/// two accounts apart, and stays out of the form whose job is to be short.
///
/// An account with no server yet is named without one. `"JMAP server "` with
/// the host left off is a sentence about a server that is not there, and Camel
/// asks for the name of a service long before anything has configured it.
///
/// The strings are English and untranslated, like the provider's own name and
/// description: there is no message catalogue under this module's domain yet,
/// and inventing calls into one that does not exist would not make them
/// translated.
pub fn describe(host: Option<&str>, port: u16, user: Option<&str>, brief: bool) -> String {
    let Some(host) = host else {
        return "JMAP account".to_owned();
    };
    if brief {
        return format!("JMAP server {host}");
    }
    let server = match port {
        0 => host.to_owned(),
        port => format!("{host}:{port}"),
    };
    match user {
        Some(user) => format!("JMAP service for {user} on {server}"),
        None => format!("JMAP service on {server}"),
    }
}

// ---------------------------------------------------------------------------
// the vfunc slots

/// Installs the four service vfuncs on a class whose first member is a
/// `CamelServiceClass`.
///
/// `T` is the type being registered, and it is what the three connecting vfuncs
/// read the instance pointer as. Naming it here rather than inferring it from
/// the class pointer is the point: a caller that installed the store's vfuncs on
/// the transport's class would have every connect on that service read a
/// `CamelTransport` as a `CamelJmapStore`.
///
/// # Safety
///
/// `class` must point at an initialised class struct of the type `T` registers —
/// which leads with a `CamelServiceClass`, because `T`'s contract has it derive
/// from `CamelService`.
pub unsafe fn install_vfuncs<T: Connected>(class: *mut CamelServiceClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.get_name = Some(get_name);
    vfuncs.connect_sync = Some(connect_sync::<T>);
    vfuncs.authenticate_sync = Some(authenticate_sync::<T>);
    vfuncs.disconnect_sync = Some(disconnect_sync::<T>);
}

/// Answers what this account is called, as a string Camel frees.
///
/// The one vfunc here with no `GError` out-parameter and no failure value:
/// every caller drops the answer straight into a message, and `CamelService`'s
/// accessor turns a missing implementation into NULL and a critical. So a panic
/// becomes the name of an account nothing is known about rather than a NULL —
/// the guard's critical is where the bug is reported.
unsafe extern "C" fn get_name(service: *mut CamelService, brief: gboolean) -> *mut gchar {
    let brief = brief != GFALSE;
    let name = jmap_backend_core::trampoline::guard(
        "get_name",
        describe(None, 0, None, brief),
        // SAFETY: Camel's contract for the vfunc — a valid instance of ours.
        || unsafe { name_of(service, brief) },
    );
    // SAFETY: the caller of `camel_service_get_name` frees the result, which is
    // what `dup_string` produces: a `g_malloc`'d copy.
    unsafe { dup_string(&name) }
}

/// Reads the three fields [`describe`] needs off the service's settings.
///
/// The host in the spelling the *account* uses rather than the punycoded one
/// [`ServerConfig`] takes: nothing connects with this string, and an account
/// configured in an internationalised domain name should be described in the
/// name its owner typed.
///
/// # Safety
///
/// `service` must be a valid `CamelService`.
unsafe fn name_of(service: *mut CamelService, brief: bool) -> String {
    // SAFETY: the contract above; the reference is released when `settings`
    // drops at the end of the scope.
    let settings = unsafe { Owned::from_raw(camel_service_ref_settings(service)) };
    // SAFETY: `settings` is NULL or the `CamelSettings` Camel just handed over.
    let read = unsafe { network(settings.as_ref().map_or(ptr::null_mut(), Owned::as_ptr)) }.map(
        |network| {
            // SAFETY: `network` implements the interface, checked above; the
            // `dup_` accessors return g_malloc'd copies `take_string` frees,
            // rather than pointers into storage another thread may replace.
            unsafe {
                (
                    take_string(camel_network_settings_dup_host(network)),
                    camel_network_settings_get_port(network),
                    take_string(camel_network_settings_dup_user(network)),
                )
            }
        },
    );

    let (host, port, user) = read.unwrap_or((None, 0, None));
    describe(host.as_deref(), port, user.as_deref(), brief)
}

/// Asks the session to authenticate this service, unless it already did.
///
/// No connection is opened here — see the module docs. The short-circuit is the
/// same one the address book backend's `connect_sync` makes and for the same
/// reason: Camel reconnects a service whenever it suspects the connection is
/// gone, including when it is not, and re-opening a live one would drop a
/// socket other threads are mid-request on.
unsafe extern "C" fn connect_sync<T: Connected>(
    service: *mut CamelService,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NULL-or-valid GCancellable, and an out-parameter that is NULL or
    // writable and currently NULL.
    unsafe {
        guard_bool("connect_sync", error, || {
            let Some(connected) = instance::<T>(service) else {
                return fail_disconnected(error);
            };
            if connected.holds_connection() {
                tracing::debug!("reusing existing mail service connection");
                return GTRUE;
            }
            tracing::debug!("initiating mail service connection");

            // The one object allowed to read a stored password or prompt for a
            // new one. A service without a session is a service nothing can
            // authenticate, which is the same dead end as having no store.
            // `session` releases the reference when it drops at the end of the
            // scope.
            let Some(session) = Owned::from_raw(camel_service_ref_session(service)) else {
                return fail_disconnected(error);
            };

            // NULL mechanism: JMAP authenticates over HTTP and offers no SASL
            // mechanisms to pick between, which is also why
            // `query_auth_types_sync` is left alone.
            camel_session_authenticate_sync(
                session.as_ptr(),
                service,
                ptr::null(),
                cancellable,
                error,
            )
        })
    }
}

/// Opens the account, with the password the session has just put on the
/// service.
unsafe extern "C" fn authenticate_sync<T: Connected>(
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
        Some(unsafe { attempt::<T>(service, cancellable) })
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
unsafe fn attempt<T: Connected>(
    service: *mut CamelService,
    cancellable: *mut GCancellable,
) -> Result<(), StoreError> {
    let connected = unsafe { instance::<T>(service) }.ok_or(StoreError::Disconnected)?;

    // Camel hands out a reference; the config is read before it is released,
    // and nothing borrowed from the settings outlives the call. `settings`
    // releases the reference when it drops at the end of the scope.
    // SAFETY: `service` is a valid CamelService, so the settings it returns are
    // a valid CamelSettings — of the class `settings_type` names, which is what
    // `from_settings` requires.
    let settings = unsafe { Owned::from_raw(camel_service_ref_settings(service)) };
    let settings_ptr = settings.as_ref().map_or(ptr::null_mut(), Owned::as_ptr);
    let config = unsafe { ServerConfig::from_settings(settings_ptr) };
    // SAFETY: as above.
    let uses_oauth2 = unsafe { oauth2::uses_oauth2(settings_ptr) };
    // SAFETY: as above.
    let uses_api_token = unsafe { api_token::uses_api_token(settings_ptr) };
    drop(settings);
    let config = config?;

    // SAFETY: the cancellable is Camel's, and it outlives the call — which is
    // exactly the scope this observation wants.
    let _cancel = unsafe { observe(cancellable) };

    // Which authentication scheme this account uses is decided here, the same
    // way `jmap_backend_core::connect::connect_with` decides it for the EDS
    // backends — off the same field Evolution's account editor writes
    // alongside the `ESource` side of the same choice. See `oauth2`'s module
    // docs for why asking a second time here cannot disagree with them.
    let credentials = if uses_oauth2 {
        tracing::debug!(uses_oauth2 = true, "authenticating with OAuth 2.0");
        // The one object allowed to fetch an OAuth 2.0 token, as it is the
        // only object allowed to fetch a password. `session` releases the
        // reference `ref_session` handed over when it drops at the end of the
        // scope.
        // SAFETY: `service` is a valid CamelService by this function's
        // contract.
        let Some(session) = (unsafe { Owned::from_raw(camel_service_ref_session(service)) }) else {
            return Err(StoreError::Disconnected);
        };
        // SAFETY: `service` valid by this function's contract, and
        // `cancellable` satisfies `access_token`'s contract by this
        // function's own.
        let token = unsafe { oauth2::access_token(session.as_ptr(), service, cancellable) };
        Credentials::bearer(token?)
    } else if uses_api_token {
        tracing::debug!(uses_api_token = true, "authenticating with an API token");
        // The pasted token rides the same password prompt Basic uses — see
        // `jmap_backend_core::api_token`'s module docs for why — so it is
        // read exactly where the password is, just below.
        // SAFETY: a borrowed, NULL-or-NUL-terminated string owned by the
        // service; `read_string` copies what it needs.
        let password = unsafe { read_string(camel_service_get_password(service)) };
        bearer_credentials(password.as_deref())
    } else {
        tracing::debug!("authenticating with a password");
        // The session put it there before calling us, and it is the only
        // credential this code ever sees: nothing reads a password out of the
        // settings object, which Evolution serialises into a config file.
        // SAFETY: a borrowed, NULL-or-NUL-terminated string owned by the
        // service; `read_string` copies what it needs.
        let password = unsafe { read_string(camel_service_get_password(service)) };
        password_credentials(config.user.as_deref(), password.as_deref())
    };

    finish_authenticate(uses_oauth2, authenticate(connected, &config, credentials))
}

/// What `attempt` reports, given what [`authenticate`] itself answered.
///
/// Split out from `attempt` so this decision — reclassify only an OAuth 2.0
/// attempt's rejection, leave every other outcome exactly as `authenticate`
/// gave it — is a plain function a test can drive without a `CamelService`,
/// the same way [`authenticate`] itself is split out from the vfunc for a
/// `CamelSession`. See `StoreError::reclassify_oauth2_rejection`'s own doc for
/// why a 401 on a bearer token EDS itself handed over is not "the password
/// was wrong" the way it is for Basic or an API token.
fn finish_authenticate(
    uses_oauth2: bool,
    outcome: Result<(), StoreError>,
) -> Result<(), StoreError> {
    let outcome = if uses_oauth2 {
        outcome.map_err(StoreError::reclassify_oauth2_rejection)
    } else {
        outcome
    };
    match &outcome {
        Ok(()) => tracing::debug!(uses_oauth2, "mail service authenticated"),
        Err(error) => {
            tracing::debug!(uses_oauth2, ?error, "mail service authentication failed");
        }
    }
    outcome
}

/// Drops the connection, then lets `CamelService` do its half.
///
/// Ours goes first: the parent's `disconnect_sync` is what marks the service
/// disconnected, and a connection still in the slot after that is one a racing
/// operation could pick up and use against a service Camel believes is closed.
/// Dropping one that is not there is not a failure — it is what Camel asks of
/// every service on shutdown, whether or not it ever connected.
unsafe extern "C" fn disconnect_sync<T: Connected>(
    service: *mut CamelService,
    clean: gboolean,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        guard_bool("disconnect_sync", error, || {
            tracing::debug!("disconnecting mail service");
            if let Some(connected) = instance::<T>(service) {
                connected.release_connection();
            }
            match parent_service_class::<T>().and_then(|class| class.disconnect_sync) {
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
/// `service` must be NULL or point at an instance of `T`. Camel only dispatches
/// a class's vfuncs on instances of that class, so a vfunc installed by
/// [`install_vfuncs`] for the same `T` satisfies this — and that is the whole
/// of what the type parameter is for.
unsafe fn instance<'a, T: Connected>(service: *mut CamelService) -> Option<&'a T> {
    unsafe { service.cast::<T>().as_ref() }
}

/// `T`'s parent class, as the `CamelServiceClass` it leads with, for the one
/// vfunc that chains up — `CamelOfflineStoreClass` for the store,
/// `CamelTransportClass` for the transport.
///
/// `g_type_class_peek` rather than `_ref`: an initialised parent class is what
/// having registered a subclass of it guarantees, and taking a reference here
/// would mean giving one back on a path that has no natural place to do so.
fn parent_service_class<T: Connected>() -> Option<&'static CamelServiceClass> {
    // SAFETY: peeking a type nothing has referenced returns NULL, which is
    // handled; otherwise the class outlives the type, which for a Camel type is
    // the life of the process. `T`'s parent derives from `CamelService`, so its
    // class leads with a `CamelServiceClass` — the trait's contract.
    unsafe {
        g_type_class_peek(T::parent_type())
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

#[cfg(test)]
mod finish_authenticate_tests {
    use jmap_client::Error;

    use super::finish_authenticate;
    use crate::connect::StoreError;

    #[test]
    fn an_oauth2_401_is_reclassified() {
        let error = StoreError::Client(Error::Http {
            status: 401,
            problem: None,
        });
        let result = finish_authenticate(true, Err(error));
        assert!(
            matches!(result, Err(StoreError::OAuth2(_))),
            "expected OAuth2, got {result:?}"
        );
    }

    #[test]
    fn the_same_401_is_left_alone_for_a_non_oauth2_attempt() {
        let error = StoreError::Client(Error::Http {
            status: 401,
            problem: None,
        });
        let result = finish_authenticate(false, Err(error));
        assert!(
            matches!(result, Err(StoreError::Client(_))),
            "expected Client, got {result:?}"
        );
    }

    #[test]
    fn success_passes_through_unchanged_either_way() {
        assert!(finish_authenticate(true, Ok(())).is_ok());
        assert!(finish_authenticate(false, Ok(())).is_ok());
    }

    /// Records every event this function emits (field name → value), so a
    /// test can assert the credential method and the outcome were traced —
    /// duplicated from `jmap_client::transport`'s own test harness for the
    /// same reason that one gives: this crate depends on `tracing`, not
    /// `tracing-subscriber`, so there is no ready-made capturing layer to
    /// share across crates. This is what item 15's own investigation found
    /// missing: a live send produced no trace to tell the operator which
    /// branch ran, because nothing here emitted any.
    struct CapturingSubscriber {
        captured: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    struct Recorder<'a> {
        sink: &'a std::sync::Mutex<Vec<(String, String)>>,
    }

    impl tracing::field::Visit for Recorder<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.sink
                .lock()
                .unwrap()
                .push((field.name().to_owned(), format!("{value:?}")));
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            self.sink
                .lock()
                .unwrap()
                .push((field.name().to_owned(), value.to_string()));
        }
    }

    impl tracing::Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            event.record(&mut Recorder {
                sink: &self.captured,
            });
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    fn run_captured(
        uses_oauth2: bool,
        outcome: Result<(), StoreError>,
    ) -> (Result<(), StoreError>, Vec<(String, String)>) {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            captured: captured.clone(),
        };
        let result = tracing::subscriber::with_default(subscriber, || {
            finish_authenticate(uses_oauth2, outcome)
        });
        let captured = captured.lock().unwrap().clone();
        (result, captured)
    }

    #[test]
    fn a_successful_oauth2_attempt_traces_which_method_was_used() {
        let (result, captured) = run_captured(true, Ok(()));
        assert!(result.is_ok());
        assert!(
            captured.contains(&("uses_oauth2".to_owned(), "true".to_owned())),
            "expected a uses_oauth2=true field, got {captured:?}"
        );
    }

    #[test]
    fn a_failed_attempt_traces_the_classified_error() {
        let error = StoreError::Client(Error::Http {
            status: 401,
            problem: None,
        });
        let (result, captured) = run_captured(true, Err(error));
        assert!(matches!(result, Err(StoreError::OAuth2(_))));
        assert!(
            captured.iter().any(|(name, _)| name == "error"),
            "expected an error field, got {captured:?}"
        );
    }
}
