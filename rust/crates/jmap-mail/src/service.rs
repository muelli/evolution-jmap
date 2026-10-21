// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `CamelService` vfuncs: `connect_sync`, `authenticate_sync` and
//! `disconnect_sync`.
//!
//! Three slots do one operation. A Camel service does *not* open its connection
//! in `connect_sync`. It asks its `CamelSession` to authenticate it. The session
//! is the only object that can read a stored password or show a prompt. It
//! calls `authenticate_sync` back: once if its password works, and once more for
//! each password that the user types after that. Each Camel store in tree works
//! this way (IMAPX and POP3 both call `camel_session_authenticate_sync` from
//! their `connect_sync`). The reason is the new prompt: a service that opens its
//! own connection cannot ask the user again when the password is wrong.
//!
//! Thus `connect_sync` here only does a short-circuit and a delegation.
//! `authenticate_sync` does the work: it reads the server from the settings,
//! takes the password that the session put on the service, opens the account,
//! and installs it on the store. Two exported functions do these steps without
//! the GObject: [`authenticate`] and [`report_authentication`]. A test can call
//! them without a `CamelSession`, an `EMailSession`, a source registry and a
//! session bus.
//!
//! `connect_sync` also selects *which mechanism* it names. For an OAuth 2.0
//! account, the mechanism decides if the session calls `authenticate_sync`
//! before it shows a consent window. [`crate::sasl`] holds that mechanism and
//! the reasons for it.
//!
//! ## The verdict, and when it comes with a `GError`
//!
//! `authenticate_sync` gives two answers: a [`CamelAuthenticationResult`] and,
//! optionally, an error. The two answers are not independent.
//! `camel_session_authenticate_sync` reads `REJECTED` as "ask the user for
//! another password and call me again", and continues its loop. It stops, and
//! gives a `GError` to its caller, only on `ERROR`. Thus an error set with a
//! `REJECTED` reports an attempt that has not failed yet. In the best case, the
//! error leaks. In the worst case, the user sees it during a password prompt.
//! [`report_authentication`] applies that rule, and it is the only place that
//! makes either answer.
//!
//! ## Two services, one set of vfuncs
//!
//! Camel gives an account two services: the [`crate::store`] that it reads
//! from, and the [`crate::transport`] that it sends through. Both are
//! `CamelService`s, both use the same settings class, and both connect with the
//! steps above. Thus this module writes the vfuncs once and installs them on
//! both. The [`Connected`] trait tells which of the two the instance pointer
//! is. The only difference is in the trait: where the new connection goes.
//!
//! ## The fourth vfunc, which is not about connecting
//!
//! `get_name` gives the name that Camel uses for the account in its own
//! messages: "Cannot get folder … from store …", the progress text, and the
//! failure line in an error dialog. `CamelService` gives no default for it. The
//! accessor is a `g_return_val_if_fail (class->get_name != NULL, NULL)`, so a
//! store with an empty slot gives NULL and logs a critical each time. [`describe`]
//! gives the name. It is a pure function of three settings fields, so a test can
//! call it directly.

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
/// The vfuncs below need only these three methods from their instance. The
/// other steps (read the settings, ask the session to authenticate, open the
/// account, classify the failure) are the same on both services.
///
/// The service *holds* the connection, and does not return it, because the two
/// services keep it differently. The store drops its folder listing with it.
/// The two services must never share one connection: each implementer names
/// its own slot, so the trait cannot express a shared one.
///
/// # Safety
///
/// The instance struct of the `GType` that an implementer registers must be
/// `Self`, and that type must derive from `CamelService`. The vfuncs get a
/// `CamelService *` and read `Self` directly through it. Each `G_DEFINE_TYPE`
/// service does the same cast.
pub unsafe trait Connected: ObjectSubclass<Instance = Self> {
    /// Installs `sync` as the live connection, and replaces the previous one.
    fn hold_connection(&self, sync: MailSync);
    /// Drops the connection, if there is one. No connection is not a failure:
    /// Camel disconnects each service on shutdown, connected or not.
    fn release_connection(&self);
    /// Tells if an operation finds a connection.
    fn holds_connection(&self) -> bool;
}

/// Opens the account that `config` names, and installs it on `service`.
///
/// This is the body of `authenticate_sync` without the GObject. Before it, the
/// vfunc reads the two arguments from a `CamelService`. Below it is
/// [`open_mail`].
///
/// A failure keeps the service as it was, with its working connection. Camel
/// authenticates a connected service again after a password change, or when
/// the session loses track of the account. A store that dropped its connection
/// before the new password arrives stops serving its folders. A *success*
/// replaces the connection and the listing. The account was authenticated
/// again for a reason, and the old tree can describe a different server.
///
/// This function does not get the caller's cancellable. The new connection
/// lives longer than this call, so a flag in it could not be cleared by a later
/// operation. The scope that the vfunc installs (see [`observe`]) stops the
/// connect, and it also stops each later operation.
pub fn authenticate<T: Connected>(
    service: &T,
    config: &ServerConfig,
    credentials: Credentials,
) -> Result<(), StoreError> {
    let sync = open_mail(config, credentials)?;
    service.hold_connection(sync);
    Ok(())
}

/// Changes one attempt into the two answers that `authenticate_sync` returns.
///
/// Takes ownership of the failure. If there is a failure, it gives the
/// resulting `GError` to `error`, or frees it if the caller did not ask for one.
///
/// The rule: only an `ERROR` verdict carries an error. `ACCEPTED` has nothing to
/// report. `REJECTED` is not a failure report. It requests another password, and
/// the session answers with a prompt and a new call. See the module docs.
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

/// The name of this account in Camel, in its two documented forms.
///
/// Camel asks for the `brief` form for a folder tree or for the middle of its
/// own messages, so it holds only the server. The documentation calls the
/// other form "complete and mostly unambiguous", so it adds the user and the
/// port. JMAP is HTTP, so one host often has several JMAP accounts (a local
/// server, a test server). Thus the port goes in the form that tells accounts
/// apart, and stays out of the short form.
///
/// An account with no server yet gets a name without one. `"JMAP server "`
/// without a host names a server that does not exist. Camel asks for the name
/// of a service before any configuration is done.
///
/// The strings are English and not translated, like the name and description
/// of the provider. This module's domain has no message catalogue yet, and
/// calls into a catalogue that does not exist do not translate them.
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
/// `T` is the type that the caller registers. The three connect vfuncs read the
/// instance pointer as `T`. The caller names `T` explicitly, and the class
/// pointer does not set it. Otherwise, the store's vfuncs on the transport's
/// class make each connect read a `CamelTransport` as a `CamelJmapStore`.
///
/// # Safety
///
/// `class` must point at an initialised class struct of the type that `T`
/// registers. That struct starts with a `CamelServiceClass`, because the
/// contract of `T` makes it derive from `CamelService`.
pub unsafe fn install_vfuncs<T: Connected>(class: *mut CamelServiceClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.get_name = Some(get_name);
    vfuncs.connect_sync = Some(connect_sync::<T>);
    vfuncs.authenticate_sync = Some(authenticate_sync::<T>);
    vfuncs.disconnect_sync = Some(disconnect_sync::<T>);
}

/// Gives the name of this account, as a string that Camel frees.
///
/// This is the only vfunc here with no `GError` out-parameter and no failure
/// value. Each caller puts the answer directly into a message, and the
/// `CamelService` accessor changes a missing implementation into NULL and a
/// critical. Thus a panic gives the name of an unknown account, not a NULL.
/// The critical from the guard reports the bug.
unsafe extern "C" fn get_name(service: *mut CamelService, brief: gboolean) -> *mut gchar {
    let brief = brief != GFALSE;
    let name = jmap_backend_core::trampoline::guard(
        "get_name",
        describe(None, 0, None, brief),
        // SAFETY: Camel's contract for the vfunc: a valid instance of ours.
        || unsafe { name_of(service, brief) },
    );
    // SAFETY: the caller of `camel_service_get_name` frees the result, which is
    // what `dup_string` produces: a `g_malloc`'d copy.
    unsafe { dup_string(&name) }
}

/// Reads the three fields that [`describe`] needs from the service's settings.
///
/// The host keeps the spelling of the *account*, not the punycode form that
/// [`ServerConfig`] uses. No connection uses this string. An account with an
/// internationalised domain name gets the name that its owner typed.
///
/// # Safety
///
/// `service` must be a valid `CamelService`.
unsafe fn name_of(service: *mut CamelService, brief: bool) -> String {
    // SAFETY: the contract above. `settings` releases the reference when it
    // drops at the end of the scope.
    let settings = unsafe { Owned::from_raw(camel_service_ref_settings(service)) };
    // SAFETY: `settings` is NULL or the `CamelSettings` that Camel just gave.
    let read = unsafe { network(settings.as_ref().map_or(ptr::null_mut(), Owned::as_ptr)) }.map(
        |network| {
            // SAFETY: `network` implements the interface, checked above. The
            // `dup_` accessors return g_malloc'd copies that `take_string`
            // frees, not pointers into storage that another thread can replace.
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

/// Asks the session to authenticate this service, if it has no connection yet.
///
/// This function opens no connection (see the module docs). The address book
/// backend's `connect_sync` does the same short-circuit, for the same reason.
/// Camel connects a service again each time it suspects a lost connection,
/// also when the connection is live. To open a live connection again drops a
/// socket that other threads use for requests in progress.
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

            // The session is the only object that can read a stored password
            // or prompt for a new one. Without a session, nothing can
            // authenticate the service, the same failure as no store.
            // `session` releases the reference when it drops at the end of the
            // scope.
            let Some(session) = Owned::from_raw(camel_service_ref_session(service)) else {
                return fail_disconnected(error);
            };

            // The mechanism for the session's authentication. For an OAuth 2.0
            // account it is [`crate::sasl::MECHANISM`], and this name gives
            // the account a silent attempt. `mail_ui_session_authenticate_sync`
            // (e-mail-ui-session.c, 3.52) finds the string with
            // `camel_sasl_authtype`. For a mechanism that needs no password, it
            // calls this service's `authenticate_sync` once before any prompt.
            // It shows the consent window only if that attempt gives REJECTED.
            // A NULL mechanism skips that branch and prompts first. That is the
            // consent window at each send that GNOME/evolution#3382 reports.
            //
            // The mechanism is the name of the SASL, not the value of the
            // account's `auth-mechanism` field. `uses_oauth2` also accepts the
            // generic EDS spelling `"OAuth2"` (`method_is_oauth2`), and no
            // `CamelSasl` and no `EOAuth2Service` has that name. That value
            // fails both lookups of the session. Our name makes the silent
            // attempt *and* its `e_oauth2_services_is_oauth2_alias` recovery
            // work for an account with either spelling.
            //
            // Each other credential kind keeps the NULL, for a reason. The user
            // must *type* a password or an API token, so they come from the
            // prompt-first loop of the session. Neither has a SASL mechanism:
            // Basic and Bearer are HTTP, and the client negotiates them. For
            // the same reason, this code does not override
            // `query_auth_types_sync`.
            let settings = Owned::from_raw(camel_service_ref_settings(service));
            let mechanism = crate::sasl::mechanism_for(
                settings.as_ref().map_or(ptr::null_mut(), Owned::as_ptr),
            );

            camel_session_authenticate_sync(
                session.as_ptr(),
                service,
                mechanism,
                cancellable,
                error,
            )
        })
    }
}

/// Opens the account, with the password that the session just put on the
/// service.
unsafe extern "C" fn authenticate_sync<T: Connected>(
    service: *mut CamelService,
    // Not used, on purpose. On the OAuth 2.0 path, the session gives back
    // [`crate::sasl::MECHANISM`], and [`attempt`] reads the same fact from the
    // settings. On each other path it is NULL. The mechanism decides if the
    // session calls this vfunc before a prompt. When this vfunc runs, that
    // decision is already made.
    _mechanism: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> CamelAuthenticationResult {
    // `report_authentication` cannot classify a panic. A panic is a bug in this
    // code, not an account failure. Thus this code reports it separately, not
    // as a service error that the user can correct.
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

/// The steps of `authenticate_sync` between the guard and the verdict.
///
/// # Safety
///
/// As the vfunc: `service` must be an instance of this type, and `cancellable`
/// must be NULL or valid.
unsafe fn attempt<T: Connected>(
    service: *mut CamelService,
    cancellable: *mut GCancellable,
) -> Result<(), StoreError> {
    let connected = unsafe { instance::<T>(service) }.ok_or(StoreError::Disconnected)?;

    // Camel gives a reference. This code reads the config before it releases
    // the reference, and no borrow from the settings lives longer than the
    // call. `settings` releases the reference when it drops at the end of the
    // scope.
    // SAFETY: `service` is a valid CamelService, so it returns a valid
    // CamelSettings of the class that `settings_type` names. `from_settings`
    // requires that.
    let settings = unsafe { Owned::from_raw(camel_service_ref_settings(service)) };
    let settings_ptr = settings.as_ref().map_or(ptr::null_mut(), Owned::as_ptr);
    let config = unsafe { ServerConfig::from_settings(settings_ptr) };
    // SAFETY: as above.
    let uses_oauth2 = unsafe { oauth2::uses_oauth2(settings_ptr) };
    // SAFETY: as above.
    let uses_api_token = unsafe { api_token::uses_api_token(settings_ptr) };
    drop(settings);
    let config = config?;

    // SAFETY: the cancellable is Camel's, and it lives longer than the call.
    // This observation needs exactly that scope.
    let _cancel = unsafe { observe(cancellable) };

    // This code selects the authentication scheme of the account, the same way
    // `jmap_backend_core::connect::connect_with` does for the EDS backends. Both
    // read the field that Evolution's account editor writes together with the
    // `ESource` side of the same choice. The module docs of `oauth2` tell why
    // this second check cannot disagree with them.
    let credentials = if uses_oauth2 {
        tracing::debug!(uses_oauth2 = true, "authenticating with OAuth 2.0");
        // The session is the only object that can get an OAuth 2.0 token, as
        // it is the only object that can get a password. `session` releases
        // the reference from `ref_session` when it drops at the end of the
        // scope.
        // SAFETY: `service` is a valid CamelService by this function's
        // contract.
        let Some(session) = (unsafe { Owned::from_raw(camel_service_ref_session(service)) }) else {
            return Err(StoreError::Disconnected);
        };
        // SAFETY: `service` is valid by this function's contract, and
        // `cancellable` meets the contract of `access_token` by this
        // function's own.
        let token = unsafe { oauth2::access_token(session.as_ptr(), service, cancellable) };
        Credentials::bearer(token?)
    } else if uses_api_token {
        tracing::debug!(uses_api_token = true, "authenticating with an API token");
        // The pasted token uses the same password prompt as Basic (the module
        // docs of `jmap_backend_core::api_token` tell why). Thus this code
        // reads it from the same place as the password, below.
        // SAFETY: a borrowed, NULL-or-NUL-terminated string owned by the
        // service; `read_string` copies what it needs.
        let password = unsafe { read_string(camel_service_get_password(service)) };
        bearer_credentials(password.as_deref())
    } else {
        tracing::debug!("authenticating with a password");
        // The session puts the password there before it calls this vfunc. It
        // is the only credential that this code sees. Nothing reads a password
        // from the settings object, which Evolution writes to a config file.
        // SAFETY: a borrowed, NULL-or-NUL-terminated string owned by the
        // service; `read_string` copies what it needs.
        let password = unsafe { read_string(camel_service_get_password(service)) };
        password_credentials(config.user.as_deref(), password.as_deref())
    };

    finish_authenticate(uses_oauth2, authenticate(connected, &config, credentials))
}

/// The result of `attempt`, from the answer of [`authenticate`].
///
/// The decision is to reclassify only the rejection of an OAuth 2.0 attempt,
/// and to keep each other outcome from `authenticate` as it is. It is a
/// separate plain function so a test can call it without a `CamelService`.
/// [`authenticate`] is separate from the vfunc for the same reason, for a
/// `CamelSession`. The doc of `StoreError::reclassify_oauth2_rejection` tells
/// why a 401 on a bearer token from EDS does not mean "wrong password", as it
/// does for Basic or an API token.
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

/// Drops the connection, then lets `CamelService` do its part.
///
/// Our part goes first. The parent's `disconnect_sync` marks the service
/// disconnected. After that, a racing operation can use a connection still in
/// the slot, on a service that Camel thinks is closed. To drop a missing
/// connection is not a failure. Camel asks each service for it on shutdown,
/// connected or not.
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
                // Each Camel with a CamelService has this slot. If not, this
                // side has nothing more to do.
                None => GTRUE,
            }
        })
    }
}

/// The Rust view of the instance pointer from Camel.
///
/// # Safety
///
/// `service` must be NULL or point at an instance of `T`. Camel calls the
/// vfuncs of a class only on instances of that class. Thus a vfunc that
/// [`install_vfuncs`] installs for the same `T` meets this contract. The type
/// parameter exists for that purpose only.
unsafe fn instance<'a, T: Connected>(service: *mut CamelService) -> Option<&'a T> {
    unsafe { service.cast::<T>().as_ref() }
}

/// The parent class of `T`, as its leading `CamelServiceClass`, for the one
/// vfunc that chains up: `CamelOfflineStoreClass` for the store,
/// `CamelTransportClass` for the transport.
///
/// The parent of `T` derives from `CamelService`, so its class starts with a
/// `CamelServiceClass`. The trait's contract makes this true.
fn parent_service_class<T: Connected>() -> Option<&'static CamelServiceClass> {
    // SAFETY: the contract above.
    unsafe { jmap_backend_core::subclass::parent_class::<CamelServiceClass>(T::parent_type()) }
}

/// The report of a vfunc with no store. Its code makes Camel connect and ask
/// again, and not show the account as broken.
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

/// The report of an `authenticate_sync` that panicked. It uses `INVALID`, not
/// `UNAVAILABLE`, because the server is not the problem. With `UNAVAILABLE`,
/// Camel tries the account again forever, but the bug always occurs again. The
/// critical that the guard logged holds the details.
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
    use jmap_tracing_capture::capture;

    use super::finish_authenticate;
    use crate::connect::StoreError;

    #[test]
    fn an_oauth2_401_is_reclassified() {
        let error = StoreError::Client(Error::Http {
            status: 401,
            problem: None,
        });
        let (result, _) = capture(|| finish_authenticate(true, Err(error)));
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
        let (result, _) = capture(|| finish_authenticate(false, Err(error)));
        assert!(
            matches!(result, Err(StoreError::Client(_))),
            "expected Client, got {result:?}"
        );
    }

    #[test]
    fn success_passes_through_unchanged_either_way() {
        assert!(capture(|| finish_authenticate(true, Ok(()))).0.is_ok());
        assert!(capture(|| finish_authenticate(false, Ok(()))).0.is_ok());
    }

    #[test]
    fn a_successful_oauth2_attempt_traces_which_method_was_used() {
        let (result, captured) = capture(|| finish_authenticate(true, Ok(())));
        assert!(result.is_ok());
        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "uses_oauth2" && value == "true"),
            "expected a uses_oauth2=true field, got {captured:?}"
        );
    }

    #[test]
    fn a_failed_attempt_traces_the_classified_error() {
        let error = StoreError::Client(Error::Http {
            status: 401,
            problem: None,
        });
        let (result, captured) = capture(|| finish_authenticate(true, Err(error)));
        assert!(matches!(result, Err(StoreError::OAuth2(_))));
        assert!(
            captured.iter().any(|(_, name, _)| name == "error"),
            "expected an error field, got {captured:?}"
        );
    }
}
