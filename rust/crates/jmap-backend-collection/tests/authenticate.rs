// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The piece between the account and the fan-out — `authenticate_sync`, minus
// the instance: an `ESource`, whatever EDS got out of libsecret, and the one
// `ESourceAuthenticationResult` that decides whether Evolution asks the user
// for a password again, gives up, or says nothing.
//
// Three of the answers here are not merely wrong when they are wrong, they are
// stuck: an account that answers ERROR where it meant REQUIRED never prompts and
// so can never be fixed; one that answers REJECTED where it meant ERROR throws
// away a password that was never the problem; and one that answers REQUIRED for
// a server that is simply down prompts forever. The fan-out itself is a closure
// here, because what it does needs a live `ECollectionBackend`, and none of the
// decisions above do.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_REQUIRED, E_CLIENT_ERROR_INVALID_ARG,
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
use jmap_backend_collection::authenticate::{Login, authenticate_with};
use jmap_client::{Credentials, Error};
use jmap_collection_sync::Parts;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// An `ESource` that is not backed by the registry, as in
/// `tests/collection_source.rs` — the account this backend is handed.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        // `e_source_get_extension` walks the registered children of
        // `E_TYPE_SOURCE_EXTENSION`; touching the accessors is what registers
        // them in a test binary.
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
        // SAFETY: a live source and a header constant; the extension is created
        // on demand and owned by the source.
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
        // SAFETY: as above; every setter copies the string it is given.
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

    /// Marks the account as authenticating with OAuth 2.0, the same field
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

    /// Marks the account as authenticating with a pasted API token, the same
    /// field `jmap_backend_core::api_token::source_uses_api_token` reads.
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
        // SAFETY: the reference `e_source_new_with_uid` returned is given back
        // exactly once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// A well-formed account: TLS, a named port, a user, everything switched on.
fn account() -> TestSource {
    TestSource::new()
        .parts(Parts::ALL)
        .authentication("jmap.example.com", 8443, Some("vera@example.com"))
        .secure(true)
}

/// The `ENamedParameters` EDS hands a backend once libsecret answered.
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

/// The `GError` a call set, taken apart before it is freed. The domain and code
/// are asserted rather than the message, because Evolution renders the code —
/// "the password was not accepted" and "a secure connection is not available"
/// are different dialogs, and the message is translated.
struct ErrorSeen {
    domain: GQuark,
    code: i32,
    message: String,
}

/// Runs `authenticate_with` and reports both of the things it answers with.
///
/// The push-credentials seam is a no-op here: every test in this file except
/// the ones about the seam itself only cares about the enum and the `GError`,
/// the same two things this helper reported before that seam existed.
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

/// Like [`run`], but also takes the `push_credentials` closure, for the tests
/// about that seam itself — which record what they were called with in a
/// `RefCell` the way every other test here records what `fan_out` was handed.
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
    // SAFETY: a valid or NULL source, a valid or NULL parameter set, a valid or
    // NULL cancellable and an out-parameter initialised to NULL are what the
    // vfunc receives.
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
    // SAFETY: a live GError ownership of which passed to us.
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

/// A fan-out that must not happen. Every decision this module makes before the
/// server is contacted is one that has to be made *without* contacting it.
fn never(_: Login) -> Result<(), Error> {
    panic!("the fan-out ran for an account that should not have been contacted");
}

#[test]
fn an_account_with_nothing_switched_on_is_accepted_without_being_contacted() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Not an error: the user switched every part off, so there is nothing to
        // discover and nothing to complain about. ERROR here would put a dialog in
        // front of someone for an account they deliberately turned down, and
        // REJECTED would discard a password that was never tried.
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
        // The order the two reads happen in, and the reason it is fixed: an account
        // with every part off never needs a host, so a missing one must not turn it
        // into a broken account. Asking for the server first would report every
        // half-written account as an error the moment the user unticked the last
        // part.
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
        // A prompt cannot supply a host, so REQUIRED here is a dialog that comes
        // back however many times the user answers it.
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
        // The rule is `jmap_backend_core::source::origin`'s and it is applied here
        // because this backend is the first thing to contact the server — and the
        // credentials would be the first thing it sent.
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
        // What EDS passes before it has asked libsecret for anything is NULL. The
        // account names a user, so it is not anonymous — REQUIRED is what turns
        // into the prompt, and anything else is an account that can never be
        // completed.
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
        // `marshal::password`'s rule, reaching this layer: an empty stored password
        // is present, not absent. Reading it as absent would prompt, and a user who
        // answers the prompt with nothing would be prompted again forever; sending
        // it and being told it is wrong terminates.
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
        // This project's own setup UI can write an OAuth 2.0 account with no
        // `[Authentication] User` at all — the identity lives in the consent, not
        // in a typed field. Reading a missing user as "anonymous" the way a plain
        // password account would be read would silently skip OAuth 2.0 entirely
        // and fan the account out with no credentials whatsoever.
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
fn an_api_token_account_is_sent_as_bearer_not_basic() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The item-6 "API Token" method: a stored secret that must reach the
        // fan-out as `Credentials::Bearer`, exactly like `connect_with` already
        // sends it for the address book, calendar and mail backends — never
        // as `Credentials::Basic`, which a Bearer-only JMAP endpoint 401s (and a
        // 401 is what turns into the account's stuck auth-retry loop, since
        // `ConnectError::auth_result` reads it as a wrong password).
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
        // Mirrors `an_account_with_a_user_and_no_stored_password_asks_for_one`:
        // an API-token account with nothing in libsecret yet must prompt, not
        // silently fan out with an empty Bearer token.
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
        // Which is `jmap-mockd` and a development Stalwart. A real server answers
        // it with the 401 that becomes a prompt anyway, so refusing here would only
        // break the case that works.
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
        // Everything the fan-out needs comes out of one read of the account, so
        // that the server this backend discovers from and the server it writes into
        // the children cannot be two different reads that disagree.
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
        // The rule `jmap_backend_core::connect` states for the book and calendar
        // backends, reached from the collection backend's own vfunc: REJECTED is
        // what makes Evolution discard the stored password, so a 403 or a server
        // that is down must not produce it — the user would be asked to fix
        // something a password cannot fix, in a loop.
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
        // GLib's convention, and EDS reads the out-parameter whatever the result
        // is: a `GError` left over from a previous attempt is how an account that
        // is fine ends up reported as broken.
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
        // EWS's `e_collection_backend_authenticate_children()`, mirrored here: a
        // collection that just resolved credentials hands them to its
        // already-running address-book/calendar children immediately, rather than
        // leaving each to hit its own credentials-required cycle before it
        // independently re-fetches the same thing. See `docs/EWS-PARITY.md` Surface
        // 5 and this function's own doc.
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
        // Nothing was freshly authenticated for this login, so there is nothing
        // honest to hand a child that never got a look at it either.
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
        // The fan-out never runs for this account (see
        // `an_account_with_nothing_switched_on_is_accepted_without_being_contacted`),
        // so there is nothing resolved yet to push either.
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
        // It should not happen — EDS constructs the backend *from* a source — but a
        // NULL dereference in `evolution-source-registry` takes every other account
        // in the session down with it.
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
        // The Stop button, reaching a discovery several layers down through a
        // client this function never sees — and, just as important, not staying
        // installed past the call: the scope belongs to this operation, not to the
        // account.
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
        // Reported as GIO's cancellation rather than an EDS client error, so that
        // Evolution can tell a user who pressed Stop from an account that broke.
        // SAFETY: no arguments.
        assert_eq!(error.domain, unsafe { g_io_error_quark() });
        assert_eq!(error.code, G_IO_ERROR_CANCELLED);

        assert!(
            jmap_client::transport::observed().is_none(),
            "the cancellation outlived the operation it belonged to"
        );

        // SAFETY: the reference `g_cancellable_new` returned is given back once.
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
