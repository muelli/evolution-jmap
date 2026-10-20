// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for `authenticate_sync` without the backend instance. The inputs are an
// `ESource` and the credentials that EDS got from libsecret. The output is the
// `ESourceAuthenticationResult` that tells Evolution to prompt, stop, or accept.
//
// A wrong result can make an account permanently stuck. ERROR instead of
// REQUIRED never prompts, so the user cannot fix the account. REJECTED instead
// of ERROR discards a correct password. REQUIRED for a server that is down
// prompts forever. The fan-out is a closure because it needs a live
// `ECollectionBackend`, and these decisions do not.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_REQUIRED, E_CLIENT_ERROR_DBUS_ERROR, E_CLIENT_ERROR_INVALID_ARG,
    E_CLIENT_ERROR_TLS_NOT_AVAILABLE, E_SOURCE_AUTHENTICATION_ACCEPTED,
    E_SOURCE_AUTHENTICATION_ERROR, E_SOURCE_AUTHENTICATION_REJECTED,
    E_SOURCE_AUTHENTICATION_REQUIRED, E_SOURCE_CREDENTIAL_PASSWORD,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ENamedParameters, ESource, ESourceAuthentication, ESourceAuthenticationResult,
    ESourceCollection, ESourceSecurity, e_client_error_quark, e_named_parameters_free,
    e_named_parameters_new, e_named_parameters_set, e_source_authentication_get_type,
    e_source_authentication_set_host, e_source_authentication_set_method,
    e_source_authentication_set_port, e_source_authentication_set_user,
    e_source_collection_get_type, e_source_collection_set_calendar_enabled,
    e_source_collection_set_contacts_enabled, e_source_collection_set_mail_enabled,
    e_source_get_extension, e_source_new_with_uid, e_source_security_get_type,
    e_source_security_set_secure, e_source_set_enabled,
};
use gio_sys::{
    G_IO_ERROR_CANCELLED, GCancellable, g_cancellable_cancel, g_cancellable_new, g_io_error_quark,
};
use glib_sys::{GError, GFALSE, GQuark, GTRUE, g_error_free};
use gobject_sys::g_object_unref;
use jmap_backend_collection::authenticate::{Login, LoginError, authenticate_with};
use jmap_backend_core::connect::ConnectError;
use jmap_client::{Credentials, Error};
use jmap_collection_sync::Parts;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The account that the backend receives: an `ESource` without a registry, as in
/// `tests/collection_source.rs`.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        // `e_source_get_extension` searches the registered children of
        // `E_TYPE_SOURCE_EXTENSION`. In a test binary, a call to each type
        // accessor registers that type.
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe {
            e_source_collection_get_type();
            e_source_authentication_get_type();
            e_source_security_get_type();
        }

        let uid = CString::new("jmap-collection").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        // SAFETY: a live source.
        unsafe { e_source_set_enabled(source, GTRUE) };
        Self(source)
    }

    fn parts(self, parts: Parts) -> Self {
        // SAFETY: a live source and a header constant. The source creates the
        // extension when necessary and owns it.
        unsafe {
            let collection: *mut ESourceCollection =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_COLLECTION.as_ptr()).cast();
            let flag = |on: bool| if on { GTRUE } else { GFALSE };
            e_source_collection_set_mail_enabled(collection, flag(parts.mail));
            e_source_collection_set_contacts_enabled(collection, flag(parts.contacts));
            e_source_collection_set_calendar_enabled(collection, flag(parts.calendars));
        }
        self
    }

    fn authentication(self, host: &str, port: u16, user: Option<&str>) -> Self {
        let host = CString::new(host).expect("no NUL in a test host");
        let user = user.map(|user| CString::new(user).expect("no NUL in a test user"));
        // SAFETY: as above. Each setter copies the string that it receives.
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_host(auth, host.as_ptr());
            e_source_authentication_set_port(auth, port);
            if let Some(user) = &user {
                e_source_authentication_set_user(auth, user.as_ptr());
            }
        }
        self
    }

    fn secure(self, secure: bool) -> Self {
        // SAFETY: as above.
        unsafe {
            let security: *mut ESourceSecurity =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
            e_source_security_set_secure(security, if secure { GTRUE } else { GFALSE });
        }
        self
    }

    /// Sets the account to use OAuth 2.0. This is the field that
    /// `jmap_backend_core::oauth2::source_uses_oauth2` reads.
    fn oauth2(self) -> Self {
        let method = CString::new("OAuth2").expect("no NUL in a literal");
        // SAFETY: as above.
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_method(auth, method.as_ptr());
        }
        self
    }

    /// Sets the account to use a pasted API token. This is the field that
    /// `jmap_backend_core::api_token::source_uses_api_token` reads.
    fn api_token(self) -> Self {
        let method = CString::new(jmap_backend_core::api_token::API_TOKEN_METHOD.to_bytes())
            .expect("no NUL in a literal");
        // SAFETY: as above.
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_method(auth, method.as_ptr());
        }
        self
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this releases the reference from `e_source_new_with_uid`
        // exactly once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// A valid account: TLS, an explicit port, a user, and all parts enabled.
fn account() -> TestSource {
    TestSource::new()
        .parts(Parts::ALL)
        .authentication("jmap.example.com", 8443, Some("vera@example.com"))
        .secure(true)
}

/// The `ENamedParameters` that EDS gives a backend after libsecret answers.
struct StoredPassword(*mut ENamedParameters);

impl StoredPassword {
    fn new(password: &str) -> Self {
        let password = CString::new(password).expect("no NUL in a test password");
        // SAFETY: no arguments; then a live parameter set, a header constant
        // and a NUL-terminated value, which the call copies.
        unsafe {
            let params = e_named_parameters_new();
            e_named_parameters_set(
                params,
                E_SOURCE_CREDENTIAL_PASSWORD.as_ptr(),
                password.as_ptr(),
            );
            Self(params)
        }
    }
}

impl Drop for StoredPassword {
    fn drop(&mut self) {
        // SAFETY: freed exactly once.
        unsafe { e_named_parameters_free(self.0) };
    }
}

/// The fields of the `GError` that a call set, copied before the error is freed.
/// The tests assert the domain and code, not the message. Evolution shows a
/// different dialog for each code, and the message is translated.
struct ErrorSeen {
    domain: GQuark,
    code: i32,
    message: String,
}

/// Runs `authenticate_with` and returns its result and its `GError`.
///
/// The `push_credentials` closure does nothing here. Only the tests for that
/// closure use [`run_full`].
fn run<F>(
    source: *mut ESource,
    credentials: *const ENamedParameters,
    cancellable: *mut GCancellable,
    fan_out: F,
) -> (ESourceAuthenticationResult, Option<ErrorSeen>)
where
    F: FnOnce(Login) -> Result<(), Error>,
{
    run_full(source, credentials, cancellable, fan_out, |_| {})
}

/// Like [`run`], but also takes the `push_credentials` closure. The tests for
/// that closure record its argument in a `RefCell`.
fn run_full<F, P>(
    source: *mut ESource,
    credentials: *const ENamedParameters,
    cancellable: *mut GCancellable,
    fan_out: F,
    push_credentials: P,
) -> (ESourceAuthenticationResult, Option<ErrorSeen>)
where
    F: FnOnce(Login) -> Result<(), Error>,
    P: FnOnce(*const ENamedParameters),
{
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: the vfunc receives these arguments: a valid or NULL source, a
    // valid or NULL parameter set, a valid or NULL cancellable, and an
    // out-parameter set to NULL.
    let result = unsafe {
        authenticate_with(
            source,
            credentials,
            cancellable,
            &mut error,
            fan_out,
            push_credentials,
        )
    };

    if error.is_null() {
        return (result, None);
    }
    // SAFETY: a live GError, and this function now owns it.
    let seen = unsafe {
        let seen = ErrorSeen {
            domain: (*error).domain,
            code: (*error).code,
            message: CStr::from_ptr((*error).message)
                .to_string_lossy()
                .into_owned(),
        };
        g_error_free(error);
        seen
    };
    (result, Some(seen))
}

/// A fan-out that must not run. The tests that use it check decisions that the
/// backend makes without a connection to the server.
fn never(_: Login) -> Result<(), Error> {
    panic!("the fan-out ran for an account that should not have been contacted");
}

#[test]
fn an_account_with_nothing_switched_on_is_accepted_without_being_contacted() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The user disabled all parts, so there is nothing to discover. ERROR
        // would show a dialog for an account that the user disabled on purpose.
        // REJECTED would discard a password that the backend did not try.
        let source = account().parts(Parts::NONE);

        let (result, error) = run(source.0, ptr::null(), ptr::null_mut(), never);

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        assert!(error.is_none(), "a switched-off account reported an error");
    });
}

#[test]
fn a_switched_off_account_is_not_refused_for_a_server_it_never_needed() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The backend reads the parts before the server. An account with all
        // parts disabled needs no host, so a missing host is not an error here.
        let source = TestSource::new().parts(Parts::NONE);

        let (result, error) = run(source.0, ptr::null(), ptr::null_mut(), never);

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        assert!(error.is_none());
    });
}

#[test]
fn an_account_that_names_no_server_is_an_error_and_never_a_prompt() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A prompt cannot supply a host. REQUIRED here would show the dialog
        // again after each answer.
        let source = TestSource::new().parts(Parts::ALL);

        let (result, error) = run(source.0, ptr::null(), ptr::null_mut(), never);

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ERROR);
        let error = error.expect("a missing host has to say so");
        // SAFETY: no arguments.
        assert_eq!(error.domain, unsafe { e_client_error_quark() });
        assert_eq!(error.code, E_CLIENT_ERROR_INVALID_ARG as i32);
    });
}

#[test]
fn a_plain_http_account_is_refused_before_a_password_is_sent() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The rule comes from `jmap_backend_core::source::origin`. This backend
        // contacts the server first, and its first request would send the
        // credentials.
        let source = account().secure(false);

        let (result, error) = run(source.0, ptr::null(), ptr::null_mut(), never);

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ERROR);
        let error = error.expect("a plaintext account has to say why");
        assert_eq!(error.code, E_CLIENT_ERROR_TLS_NOT_AVAILABLE as i32);
    });
}

#[test]
fn an_account_with_a_user_and_no_stored_password_asks_for_one() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // EDS passes NULL before it asks libsecret. The account names a user, so
        // it is not anonymous. Only REQUIRED causes the prompt that lets the user
        // complete the account.
        let source = account();

        let (result, error) = run(source.0, ptr::null(), ptr::null_mut(), never);

        assert_eq!(result, E_SOURCE_AUTHENTICATION_REQUIRED);
        let error = error.expect("the prompt is worth a reason");
        assert_eq!(error.code, E_CLIENT_ERROR_AUTHENTICATION_REQUIRED as i32);
    });
}

#[test]
fn an_empty_stored_password_is_tried_rather_than_prompted_for() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The rule of `marshal::password`: an empty stored password is present,
        // not absent. If it were absent, an empty answer to the prompt would
        // cause a new prompt forever. A rejection from the server ends the loop.
        let source = account();
        let stored = StoredPassword::new("");
        let seen = RefCell::new(None);

        let (result, _) = run(source.0, stored.0, ptr::null_mut(), |login| {
            *seen.borrow_mut() = Some(login.credentials);
            Ok(())
        });

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        match seen.into_inner() {
            Some(Credentials::Basic { user, password }) => {
                assert_eq!(user, "vera@example.com");
                assert_eq!(password, "");
            }
            other => panic!("an empty stored password was not sent: {other:?}"),
        }
    });
}

#[test]
fn an_oauth2_account_is_never_treated_as_anonymous() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The setup UI of this project can write an OAuth 2.0 account with no
        // `[Authentication] User`. The identity comes from the consent, not from
        // a typed field. If the backend read a missing user as anonymous, it would
        // skip OAuth 2.0 and fan out with no credentials.
        let source = TestSource::new()
            .parts(Parts::ALL)
            .authentication("jmap.example.com", 8443, None)
            .secure(true)
            .oauth2();

        let (result, error) = run(source.0, ptr::null(), ptr::null_mut(), never);

        assert_eq!(result, E_SOURCE_AUTHENTICATION_REQUIRED);
        let error = error.expect("an OAuth 2.0 account with no token has to say so");
        assert_eq!(error.code, E_CLIENT_ERROR_AUTHENTICATION_REQUIRED as i32);
        assert_ne!(
            error.message, "the account has no password yet",
            "an OAuth 2.0 account was routed through the Basic-auth path"
        );
    });
}

#[test]
fn a_secret_store_failure_reports_error_and_dbus_error_code() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let failure = LoginError::Credentials(ConnectError::SecretStore(
            "the login keyring is locked".to_owned(),
        ));
        assert_eq!(failure.auth_result(), E_SOURCE_AUTHENTICATION_ERROR);
        assert_ne!(failure.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);

        let gerror = failure.to_gerror();
        assert!(!gerror.is_null());
        unsafe {
            assert_eq!((*gerror).domain, e_client_error_quark());
            assert_eq!((*gerror).code, E_CLIENT_ERROR_DBUS_ERROR as i32);
            let message = CStr::from_ptr((*gerror).message).to_string_lossy();
            assert!(message.contains("the login keyring is locked"), "{message}");
            glib_sys::g_error_free(gerror);
        }
    });
}

#[test]
fn an_api_token_account_is_sent_as_bearer_not_basic() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The "API Token" method: the stored secret must reach the fan-out as
        // `Credentials::Bearer`, as `connect_with` sends it for the address
        // book, calendar and mail backends. A Bearer-only JMAP endpoint answers
        // `Credentials::Basic` with 401. `ConnectError::auth_result` reads a 401
        // as a wrong password, so the account would retry authentication forever.
        let source = account().api_token();
        let stored = StoredPassword::new("t0k3n");
        let seen = RefCell::new(None);

        let (result, error) = run(source.0, stored.0, ptr::null_mut(), |login| {
            *seen.borrow_mut() = Some(login.credentials);
            Ok(())
        });

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        assert!(error.is_none());
        match seen.into_inner() {
            Some(Credentials::Bearer(token)) => assert_eq!(token, "t0k3n"),
            other => panic!("an API-token account was not sent as Bearer: {other:?}"),
        }
    });
}

#[test]
fn an_api_token_account_with_no_stored_token_asks_for_one() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // As in `an_account_with_a_user_and_no_stored_password_asks_for_one`: an
        // API-token account with no secret in libsecret must prompt. It must not
        // fan out with an empty Bearer token.
        let source = account().api_token();

        let (result, error) = run(source.0, ptr::null(), ptr::null_mut(), never);

        assert_eq!(result, E_SOURCE_AUTHENTICATION_REQUIRED);
        let error = error.expect("the prompt is worth a reason");
        assert_eq!(error.code, E_CLIENT_ERROR_AUTHENTICATION_REQUIRED as i32);
    });
}

#[test]
fn an_account_with_no_user_is_authenticated_anonymously() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `jmap-mockd` and a development Stalwart accept anonymous access. A real
        // server answers with a 401, which causes a prompt. A refusal here would
        // only break the anonymous case.
        let source = TestSource::new()
            .parts(Parts::ALL)
            .authentication("127.0.0.1", 31415, None)
            .secure(false);
        let seen = RefCell::new(None);

        let (result, error) = run(source.0, ptr::null(), ptr::null_mut(), |login| {
            *seen.borrow_mut() = Some(login.credentials);
            Ok(())
        });

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        assert!(error.is_none());
        assert!(matches!(seen.into_inner(), Some(Credentials::None)));
    });
}

#[test]
fn the_fan_out_is_given_the_server_the_account_names_and_the_parts_it_left_on() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The fan-out gets all its data from one read of the account. Thus the
        // discovery server and the server written into the children are the same.
        let source = account().parts(Parts {
            mail: false,
            contacts: true,
            calendars: false,
        });
        let stored = StoredPassword::new("hunter2");
        let seen = RefCell::new(None);

        let (result, _) = run(source.0, stored.0, ptr::null_mut(), |login| {
            *seen.borrow_mut() = Some(login);
            Ok(())
        });

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        let login = seen.into_inner().expect("the fan-out was not run");
        assert_eq!(
            login.server.target,
            jmap_backend_core::source::ConnectTarget::Origin(
                "https://jmap.example.com:8443".into()
            )
        );
        assert_eq!(login.server.connection.host, "jmap.example.com");
        assert_eq!(login.server.connection.port, Some(8443));
        assert_eq!(
            login.parts,
            Parts {
                mail: false,
                contacts: true,
                calendars: false,
            }
        );
        match login.credentials {
            Credentials::Basic { user, password } => {
                assert_eq!(user, "vera@example.com");
                assert_eq!(password, "hunter2");
            }
            other => panic!("the account's user did not reach the fan-out: {other:?}"),
        }
    });
}

#[test]
fn only_a_401_makes_evolution_ask_for_the_password_again() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The rule of `jmap_backend_core::connect` for the book and calendar
        // backends also applies to this vfunc. REJECTED makes Evolution discard
        // the stored password. A 403 or a server that is down must not give
        // REJECTED, because a new password cannot fix them.
        let source = account();
        let stored = StoredPassword::new("hunter2");

        let rejected = run(source.0, stored.0, ptr::null_mut(), |_| {
            Err(Error::Http {
                status: 401,
                problem: None,
            })
        });
        assert_eq!(rejected.0, E_SOURCE_AUTHENTICATION_REJECTED);
        assert!(rejected.1.is_some(), "a rejection has to say so");

        for failure in [
            Error::Http {
                status: 403,
                problem: None,
            },
            Error::Transport("connection refused".to_owned()),
        ] {
            let (result, error) = run(source.0, stored.0, ptr::null_mut(), |_| Err(failure));
            assert_eq!(
                result, E_SOURCE_AUTHENTICATION_ERROR,
                "the password was discarded for a failure it could not have caused"
            );
            assert!(error.is_some());
        }
    });
}

#[test]
fn a_fan_out_that_worked_sets_no_error() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This is the GLib convention. EDS reads the out-parameter for each
        // result, so a stale `GError` would report a good account as broken.
        let source = account();
        let stored = StoredPassword::new("hunter2");

        let (result, error) = run(source.0, stored.0, ptr::null_mut(), |_| Ok(()));

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        assert!(error.is_none(), "a successful authenticate set an error");
    });
}

#[test]
fn a_successful_fan_out_pushes_the_same_credentials_to_already_running_children() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The same as `e_collection_backend_authenticate_children()` in EWS. The
        // collection gives the credentials to its running address-book and
        // calendar children immediately. Otherwise each child gets the same
        // credentials again by itself. See the documentation of `authenticate_with`.
        let source = account();
        let stored = StoredPassword::new("hunter2");
        let pushed = RefCell::new(Vec::new());

        let (result, error) = run_full(
            source.0,
            stored.0,
            ptr::null_mut(),
            |_| Ok(()),
            |credentials| pushed.borrow_mut().push(credentials),
        );

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        assert!(error.is_none());
        assert_eq!(
            pushed.into_inner(),
            vec![stored.0.cast_const()],
            "the fan-out's own credentials were not pushed to the children exactly once"
        );
    });
}

#[test]
fn a_fan_out_that_fails_pushes_no_credentials_to_children() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This login did not authenticate, so there are no credentials to give
        // to the children.
        let source = account();
        let stored = StoredPassword::new("hunter2");
        let pushed = RefCell::new(Vec::new());

        let (result, error) = run_full(
            source.0,
            stored.0,
            ptr::null_mut(),
            |_| {
                Err(Error::Http {
                    status: 401,
                    problem: None,
                })
            },
            |credentials| pushed.borrow_mut().push(credentials),
        );

        assert_eq!(result, E_SOURCE_AUTHENTICATION_REJECTED);
        assert!(error.is_some());
        assert!(
            pushed.into_inner().is_empty(),
            "a rejected fan-out still pushed credentials to the children"
        );
    });
}

#[test]
fn an_account_with_nothing_switched_on_pushes_no_credentials_to_children() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The fan-out does not run for this account (see
        // `an_account_with_nothing_switched_on_is_accepted_without_being_contacted`),
        // so there are no credentials to push.
        let source = account().parts(Parts::NONE);
        let pushed = RefCell::new(Vec::new());

        let (result, error) = run_full(
            source.0,
            ptr::null(),
            ptr::null_mut(),
            never,
            |credentials| pushed.borrow_mut().push(credentials),
        );

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        assert!(error.is_none());
        assert!(pushed.into_inner().is_empty());
    });
}

#[test]
fn a_backend_with_no_account_is_an_error_rather_than_a_crash() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // EDS constructs the backend from a source, so this is not expected. But a
        // NULL dereference in `evolution-source-registry` stops all accounts in
        // the session.
        let (result, error) = run(ptr::null_mut(), ptr::null(), ptr::null_mut(), never);

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ERROR);
        let error = error.expect("a backend with no account has to say so");
        // SAFETY: no arguments.
        assert_eq!(error.domain, unsafe { e_client_error_quark() });
        assert!(
            !error.message.is_empty(),
            "the error carried no message at all"
        );
    });
}

#[test]
fn the_fan_out_is_stopped_by_the_cancellable_and_nothing_after_it_is() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The Stop button must reach a discovery through a client that this
        // function does not see. The cancellation must also end with the call,
        // because it belongs to this operation, not to the account.
        let source = account();
        let stored = StoredPassword::new("hunter2");
        // SAFETY: no arguments; the cancellable is unreffed below.
        let cancellable = unsafe { g_cancellable_new() };
        // SAFETY: a live cancellable.
        unsafe { g_cancellable_cancel(cancellable) };

        let (result, error) = run(source.0, stored.0, cancellable, |_| {
            let observed =
                jmap_client::transport::observed().expect("the fan-out observed no cancellation");
            assert!(
                observed.is_cancelled(),
                "a cancellable that was already cancelled did not reach the fan-out"
            );
            Err(Error::Cancelled)
        });

        assert_eq!(result, E_SOURCE_AUTHENTICATION_ERROR);
        let error = error.expect("a cancelled authenticate has to say so");
        // The error is a GIO cancellation, not an EDS client error. Thus Evolution
        // can tell a user who pressed Stop from a broken account.
        // SAFETY: no arguments.
        assert_eq!(error.domain, unsafe { g_io_error_quark() });
        assert_eq!(error.code, G_IO_ERROR_CANCELLED);

        assert!(
            jmap_client::transport::observed().is_none(),
            "the cancellation outlived the operation it belonged to"
        );

        // SAFETY: this releases the reference from `g_cancellable_new` once.
        unsafe { g_object_unref(cancellable.cast()) };
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_authenticate_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
