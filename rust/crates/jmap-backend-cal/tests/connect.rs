// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for `connect_sync` and source configuration resolution.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_REQUIRED, E_CLIENT_ERROR_DBUS_ERROR, E_CLIENT_ERROR_INVALID_ARG,
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, E_SOURCE_AUTHENTICATION_ACCEPTED,
    E_SOURCE_AUTHENTICATION_ERROR, E_SOURCE_AUTHENTICATION_REJECTED,
    E_SOURCE_AUTHENTICATION_REQUIRED, E_SOURCE_CREDENTIAL_PASSWORD,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_RESOURCE, E_SOURCE_EXTENSION_SECURITY,
    ESourceAuthentication, ESourceAuthenticationResult, e_client_error_quark,
    e_named_parameters_free, e_named_parameters_new, e_named_parameters_set,
    e_source_authentication_set_host, e_source_authentication_set_method,
    e_source_authentication_set_port, e_source_authentication_set_user, e_source_get_extension,
    e_source_resource_set_identity, e_source_security_set_secure,
};
use glib_sys::GError;
use jmap_backend_cal::connect;
use jmap_backend_core::api_token::API_TOKEN_METHOD;
use jmap_backend_core::connect::{Collection, ConnectError, credentials};
use jmap_backend_core::source::{ConnectTarget, OwnedSource, SourceConfig};
use jmap_client::Credentials;
use jmap_mock::MockServer;
use jmap_proto::calendars::CalendarRights;

mod common;
use common::Fixture;

/// Connect to a calendar with resolved credentials.
///
/// This helper simulates `connect_with`.
/// See `jmap-backend-core/tests/oauth2.rs` for credential selection details.
fn open(
    config: &SourceConfig,
    password: Option<&str>,
) -> Result<connect::OpenCalendar, ConnectError> {
    connect::open_calendar(config, credentials(config.user.as_deref(), password)?)
}

/// Return the opened calendar name for assertion failure messages.
fn expect_error(result: Result<connect::OpenCalendar, ConnectError>) -> ConnectError {
    match result {
        Ok(sync) => panic!("expected a failure, but opened {}", sync.calendar_id()),
        Err(error) => error,
    }
}

#[test]
fn a_source_that_names_no_calendar_gets_the_default_one() {
    let fixture = Fixture::start();
    let sync = open(&fixture.config(), None).expect("connected");

    assert_eq!(sync.calendar_id(), &fixture.ours);
    assert_eq!(sync.account_id(), &fixture.server.account_id());
}

#[test]
fn a_source_that_names_a_calendar_gets_that_one() {
    let fixture = Fixture::start();
    let mut config = fixture.config();
    config.resource_id = Some(fixture.theirs.to_string());

    let sync = open(&config, None).expect("connected");
    assert_eq!(sync.calendar_id(), &fixture.theirs);
}

/// Verify that connecting to an unlisted calendar ID fails.
#[test]
fn a_calendar_the_server_does_not_have_is_refused() {
    let fixture = Fixture::start();
    let mut config = fixture.config();
    config.resource_id = Some("Cal-nonesuch".to_owned());

    match expect_error(open(&config, None)) {
        ConnectError::NoSuchCollection(Collection::Calendar, id) => assert_eq!(id, "Cal-nonesuch"),
        other => panic!("expected NoSuchCollection(Calendar), got {other:?}"),
    }
}

#[test]
fn an_account_with_no_default_calendar_is_an_error_not_a_guess() {
    let (_server, config) = Fixture::start_without_default();
    match expect_error(open(&config, None)) {
        ConnectError::NoDefaultCollection(Collection::Calendar) => {}
        other => panic!("expected NoDefaultCollection(Calendar), got {other:?}"),
    }
}

/// Verify that an account without calendar capability fails.
#[test]
fn an_account_that_offers_no_calendars_is_refused() {
    let server = MockServer::builder()
        .without_capability(jmap_proto::session::CAPABILITY_CALENDARS)
        .start();
    let config = SourceConfig {
        target: ConnectTarget::Origin(server.origin().to_owned()),
        user: None,
        resource_id: None,
        rebase_urls: false,
    };

    let error = expect_error(open(&config, None));
    assert!(
        matches!(error, ConnectError::Client(_)),
        "expected a client error, got {error:?}"
    );
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_ERROR);
}

/// RFC 8620 §2: server may omit primaryAccounts if one account offers calendars.
#[test]
fn a_server_with_no_primary_accounts_resolves_the_sole_personal_calendars_account() {
    let fixture = Fixture::start_with(MockServer::builder().without_primary_accounts());

    let sync = open(&fixture.config(), None).expect("connected");
    assert_eq!(sync.account_id(), &fixture.server.account_id());
}

#[test]
fn a_user_name_with_no_password_asks_evolution_for_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().basic_auth("vera", "hunter2"));
    let mut config = fixture.config();
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, None));
    assert!(matches!(error, ConnectError::CredentialsRequired));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);
}

/// Verify that the client sends the access token as bearer credentials.
#[test]
fn an_access_token_is_sent_as_bearer_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().bearer_token("ya29.a0Af"));

    let sync = connect::open_calendar(&fixture.config(), Credentials::bearer("ya29.a0Af"))
        .expect("connected");
    assert_eq!(sync.calendar_id(), &fixture.ours);
}

/// Verify that 401 on a bearer token maps to `CredentialsRequired`.
#[test]
fn a_401_on_a_bearer_token_reclassifies_to_required_not_rejected() {
    let fixture = Fixture::start_with(MockServer::builder().bearer_token("good-token"));

    let error = expect_error(connect::open_calendar(
        &fixture.config(),
        Credentials::bearer("wrong-token"),
    ));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_REJECTED);

    let reclassified = error.reclassify_oauth2_rejection();
    assert!(
        matches!(reclassified, ConnectError::OAuth2(_)),
        "expected OAuth2, got {reclassified}"
    );
    assert_eq!(reclassified.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);
}

/// Verify that connect failures map to expected EDS `EClientError` codes.
#[test]
fn each_failure_carries_the_client_error_code_evolution_routes_on() {
    for (error, expected) in [
        (
            ConnectError::CredentialsRequired,
            E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
        ),
        (
            ConnectError::OAuth2("consent required".to_owned()),
            E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
        ),
        (
            ConnectError::SecretStore("the keyring is locked".to_owned()),
            E_CLIENT_ERROR_DBUS_ERROR,
        ),
        (
            ConnectError::NoDefaultCollection(Collection::Calendar),
            E_CLIENT_ERROR_INVALID_ARG,
        ),
        (
            ConnectError::NoSuchCollection(Collection::Calendar, "Cal-nonesuch".to_owned()),
            E_CLIENT_ERROR_INVALID_ARG,
        ),
        (
            ConnectError::Client(jmap_client::Error::Transport("down".to_owned())),
            E_CLIENT_ERROR_REPOSITORY_OFFLINE,
        ),
    ] {
        let gerror = error.to_gerror();
        assert!(!gerror.is_null(), "no GError for {error}");
        unsafe {
            assert_eq!(
                (*gerror).domain,
                e_client_error_quark(),
                "domain for {error}"
            );
            // `GError.code` is a signed integer; `EClientError` is unsigned in bindgen.
            assert_eq!((*gerror).code, expected as i32, "code for {error}");
            assert!(!(*gerror).message.is_null(), "no message for {error}");
            glib_sys::g_error_free(gerror);
        }
    }
}

#[test]
fn the_password_is_sent_as_basic_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().basic_auth("vera", "hunter2"));
    let mut config = fixture.config();
    config.user = Some("vera".to_owned());

    let sync = open(&config, Some("hunter2")).expect("connected");
    assert_eq!(sync.calendar_id(), &fixture.ours);
}

/// Distinguish between wrong credentials (`REJECTED`) and server errors (`ERROR`).
#[test]
fn a_wrong_password_is_reported_as_rejected_so_evolution_re_prompts() {
    let fixture = Fixture::start_with(MockServer::builder().basic_auth("vera", "hunter2"));
    let mut config = fixture.config();
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, Some("wrong")));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_REJECTED);
}

#[test]
fn an_unreachable_server_is_an_error_not_a_credentials_problem() {
    let config = SourceConfig {
        // Port 1 is reserved and does not accept connections.
        target: ConnectTarget::Origin("http://127.0.0.1:1".to_owned()),
        user: None,
        resource_id: None,
        rebase_urls: false,
    };
    let error = expect_error(open(&config, None));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_ERROR);
}

// ---------------------------------------------------------------------------
// Tests for `connect_sync` from the `ESource` level.

/// An `ESource` not backed by the registry.
///
/// A null D-Bus connection matches EDS behavior for keyfile sources.
struct TestSource(OwnedSource);

impl TestSource {
    fn new() -> Self {
        Self(OwnedSource::new("jmap-cal-connect-test"))
    }

    fn authentication(&self) -> *mut ESourceAuthentication {
        // SAFETY: source is live and the extension name is a valid constant.
        unsafe {
            e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr())
        }
        .cast()
    }

    /// Configure the source to target `origin`.
    ///
    /// The mock server uses loopback HTTP without TLS.
    fn at(self, origin: &str) -> Self {
        let (host, port) = origin
            .trim_start_matches("http://")
            .split_once(':')
            .expect("the mock origin has a port");
        let host = CString::new(host).expect("no NUL in a host");
        // SAFETY: live extensions and a valid NUL-terminated string.
        unsafe {
            e_source_authentication_set_host(self.authentication(), host.as_ptr());
            e_source_authentication_set_port(self.authentication(), port.parse().expect("a port"));
            e_source_security_set_secure(
                e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_SECURITY.as_ptr())
                    .cast(),
                0,
            );
        }
        self
    }

    /// Set `ESourceResource` identity (calendar ID).
    fn identity(self, id: &str) -> Self {
        let id = CString::new(id).expect("no NUL in an identity");
        // SAFETY: live extensions and a valid NUL-terminated string.
        unsafe {
            e_source_resource_set_identity(
                e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_RESOURCE.as_ptr())
                    .cast(),
                id.as_ptr(),
            )
        };
        self
    }

    fn user(self, user: &str) -> Self {
        let user = CString::new(user).expect("no NUL in a user");
        // SAFETY: live extensions and a valid NUL-terminated string.
        unsafe { e_source_authentication_set_user(self.authentication(), user.as_ptr()) };
        self
    }

    fn method(self, method: &str) -> Self {
        let method = CString::new(method).expect("no NUL in a method");
        // SAFETY: live extensions and a valid NUL-terminated string.
        unsafe { e_source_authentication_set_method(self.authentication(), method.as_ptr()) };
        self
    }
}

/// Output parameters passed by EDS to `connect_sync`.
struct ConnectOuts {
    auth_result: ESourceAuthenticationResult,
    error: *mut GError,
}

impl Default for ConnectOuts {
    fn default() -> Self {
        Self {
            auth_result: E_SOURCE_AUTHENTICATION_REJECTED,
            error: ptr::null_mut(),
        }
    }
}

impl ConnectOuts {
    /// Assert the reported domain and code, and free the error.
    unsafe fn take_error(&mut self, code: u32) {
        unsafe {
            assert!(!self.error.is_null(), "the call failed without an error");
            assert_eq!((*self.error).domain, e_client_error_quark(), "domain");
            assert_eq!((*self.error).code, code as i32, "code");
            glib_sys::g_error_free(self.error);
            self.error = ptr::null_mut();
        }
    }
}

fn connect_from(source: &TestSource, outs: &mut ConnectOuts) -> Option<connect::OpenCalendar> {
    // SAFETY: live `ESource`, null credentials, and valid output parameters.
    unsafe {
        connect::connect(
            source.0.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    }
}

#[test]
fn connecting_from_a_source_opens_the_default_calendar_and_reports_accepted() {
    let fixture = Fixture::start();
    let source = TestSource::new().at(fixture.server.origin());
    let mut outs = ConnectOuts::default();

    let sync = connect_from(&source, &mut outs).expect("the connection was refused");
    assert_eq!(sync.calendar_id(), &fixture.ours);
    assert!(outs.error.is_null(), "a successful connect set an error");
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
}

/// Verify that Resource identity extension selects the target calendar.
#[test]
fn the_resource_identity_selects_the_calendar() {
    let fixture = Fixture::start();
    let source = TestSource::new()
        .at(fixture.server.origin())
        .identity(fixture.theirs.as_ref());
    let mut outs = ConnectOuts::default();

    let sync = connect_from(&source, &mut outs).expect("the connection was refused");
    assert_eq!(sync.calendar_id(), &fixture.theirs);
}

/// Verify that user without password prompts before network request.
#[test]
fn a_source_with_a_user_and_no_password_asks_for_one_before_connecting() {
    let source = TestSource::new().at("http://127.0.0.1:1").user("vera");
    let mut outs = ConnectOuts::default();

    assert!(connect_from(&source, &mut outs).is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_REQUIRED);
    // SAFETY: the failed call populated a valid error pointer.
    unsafe { outs.take_error(E_CLIENT_ERROR_AUTHENTICATION_REQUIRED) };
}

/// Verify that API token source sends bearer credentials.
#[test]
fn an_api_token_source_sends_the_stored_secret_as_bearer_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().bearer_token("t0k3n"));
    let source = TestSource::new()
        .at(fixture.server.origin())
        .method(API_TOKEN_METHOD.to_str().unwrap());
    let mut outs = ConnectOuts::default();

    // SAFETY: live `ENamedParameters`, freed below.
    let credentials = unsafe {
        let params = e_named_parameters_new();
        let value = CString::new("t0k3n").unwrap();
        e_named_parameters_set(
            params,
            E_SOURCE_CREDENTIAL_PASSWORD.as_ptr(),
            value.as_ptr(),
        );
        params
    };

    // SAFETY: live `ESource`, valid credentials, and valid output parameters.
    let sync = unsafe {
        connect::connect(
            source.0.as_ptr(),
            credentials,
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    };
    // SAFETY: test owns the only reference.
    unsafe { e_named_parameters_free(credentials) };

    let sync = sync.expect("the connection was refused");
    assert_eq!(sync.calendar_id(), &fixture.ours);
    assert!(outs.error.is_null(), "a successful connect set an error");
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
}

/// Verify that an API token account without secret does not contact server.
#[test]
fn an_api_token_source_with_no_stored_secret_asks_for_one_before_connecting() {
    let source = TestSource::new()
        .at("http://127.0.0.1:1")
        .method(API_TOKEN_METHOD.to_str().unwrap());
    let mut outs = ConnectOuts::default();

    // SAFETY: live `ESource`, null credentials, and valid output parameters.
    let sync = unsafe {
        connect::connect(
            source.0.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    };

    assert!(sync.is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_REQUIRED);
    // SAFETY: the failed call populated a valid error pointer.
    unsafe { outs.take_error(E_CLIENT_ERROR_AUTHENTICATION_REQUIRED) };
}

/// Verify that an OAuth 2.0 source without token returns `REQUIRED`.
#[test]
fn an_oauth2_source_with_no_token_asks_for_consent_before_connecting() {
    let source = TestSource::new().at("http://127.0.0.1:1").method("OAuth2");
    let mut outs = ConnectOuts::default();

    // SAFETY: live `ESource`, null credentials, and valid output parameters.
    let sync = unsafe {
        connect::connect(
            source.0.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    };

    assert!(sync.is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_REQUIRED);
    // SAFETY: the failed call populated a valid error pointer.
    unsafe { outs.take_error(E_CLIENT_ERROR_AUTHENTICATION_REQUIRED) };
}

/// Verify that source without host returns error rather than prompting.
#[test]
fn a_source_that_names_no_server_is_an_error_rather_than_a_prompt() {
    let source = TestSource::new();
    let mut outs = ConnectOuts::default();

    assert!(connect_from(&source, &mut outs).is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ERROR);
    // SAFETY: live `ESource`, null credentials, and valid output parameters.
    unsafe { outs.take_error(E_CLIENT_ERROR_INVALID_ARG) };
}

/// Verify that null source returns error without dereferencing null.
#[test]
fn a_backend_without_a_source_fails_instead_of_dereferencing_null() {
    let mut outs = ConnectOuts::default();

    // SAFETY: `connect` explicitly accepts a null `ESource`.
    let sync = unsafe {
        connect::connect(
            ptr::null_mut(),
            ptr::null(),
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    };

    assert!(sync.is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ERROR);
    // SAFETY: the failed call populated a valid error pointer.
    unsafe { outs.take_error(E_CLIENT_ERROR_INVALID_ARG) };
}

#[test]
fn a_calendar_with_no_my_rights_opens_writable() {
    let fixture = Fixture::start();
    let open = open(&fixture.config(), None).expect("connected");
    assert!(open.writable);
}

#[test]
fn a_calendar_with_write_rights_opens_writable() {
    let fixture = Fixture::start();
    {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.server.account_id()).unwrap();
        let cal = account.calendars.get_mut(&fixture.theirs).unwrap();
        cal.my_rights = Some(CalendarRights::all());
    }
    let mut config = fixture.config();
    config.resource_id = Some(fixture.theirs.to_string());
    let open = open(&config, None).expect("connected");
    assert!(open.writable);
}

#[test]
fn a_calendar_without_write_rights_opens_read_only() {
    let fixture = Fixture::start();
    {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.server.account_id()).unwrap();
        let cal = account.calendars.get_mut(&fixture.theirs).unwrap();
        cal.my_rights = Some(CalendarRights::read_only());
    }
    let mut config = fixture.config();
    config.resource_id = Some(fixture.theirs.to_string());
    let open = open(&config, None).expect("connected");
    assert!(!open.writable);
}

#[test]
fn connecting_from_a_source_reports_read_only_when_calendar_lacks_write_rights() {
    let fixture = Fixture::start();
    {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.server.account_id()).unwrap();
        let cal = account.calendars.get_mut(&fixture.ours).unwrap();
        cal.my_rights = Some(CalendarRights::read_only());
    }
    let source = TestSource::new().at(fixture.server.origin());
    let mut outs = ConnectOuts::default();

    let sync = connect_from(&source, &mut outs);
    let open = sync.expect("connected");
    assert!(!open.writable);
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
    assert!(outs.error.is_null());
}

#[test]
fn open_calendar_with_explicit_account_resolves_collections_from_that_account() {
    let (fixture, second_account_id, second_calendar_id) = Fixture::start_two_accounts();
    let open = connect::open_calendar_with_account(
        &fixture.config(),
        Credentials::none(),
        Some(second_account_id.as_str()),
    )
    .expect("connected to second account");

    assert_eq!(open.sync.account_id(), &second_account_id);
    assert_eq!(open.sync.calendar_id(), &second_calendar_id);
}

#[test]
fn open_calendar_with_omitted_account_defaults_to_primary_account() {
    let (fixture, second_account_id, _) = Fixture::start_two_accounts();
    let open = connect::open_calendar_with_account(&fixture.config(), Credentials::none(), None)
        .expect("connected to primary account");

    assert_eq!(open.sync.account_id(), &fixture.server.account_id());
    assert_ne!(open.sync.account_id(), &second_account_id);
    assert_eq!(open.sync.calendar_id(), &fixture.ours);
}

#[test]
fn connecting_from_a_source_with_account_id_opens_that_account() {
    let (fixture, second_account_id, second_calendar_id) = Fixture::start_two_accounts();
    let source = TestSource::new()
        .at(fixture.server.origin())
        .identity(&format!("{second_account_id}/{second_calendar_id}"));
    let mut outs = ConnectOuts::default();

    let sync = connect_from(&source, &mut outs).expect("the connection was refused");
    assert_eq!(sync.account_id(), &second_account_id);
    assert_eq!(sync.calendar_id(), &second_calendar_id);
    assert!(outs.error.is_null());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
}

/// Verify that collection child opens the account named in `[Resource] Identity`.
///
/// The source carries keyfile configuration without object data.
#[test]
fn connecting_from_a_source_whose_identity_names_an_account_opens_that_accounts_calendar() {
    let (fixture, second_account_id, _) = Fixture::start_two_accounts();
    // Non-default calendar verifies that collection configuration applies.
    let shared = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&second_account_id).unwrap();
        account.seed_calendar("Shared", false)
    };
    let source = TestSource::new()
        .at(fixture.server.origin())
        .identity(&format!("{second_account_id}/{shared}"));
    let mut outs = ConnectOuts::default();

    let sync = connect_from(&source, &mut outs).expect("the connection was refused");
    assert_eq!(sync.account_id(), &second_account_id);
    assert_eq!(sync.calendar_id(), &shared);
    assert!(outs.error.is_null());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
}

#[test]
fn a_config_whose_resource_names_an_account_opens_that_accounts_calendar() {
    let (fixture, second_account_id, second_default) = Fixture::start_two_accounts();
    let mut config = fixture.config();
    config.resource_id = Some(format!("{second_account_id}/{second_default}"));

    let sync = open(&config, None).expect("connected to the second account");
    assert_eq!(sync.account_id(), &second_account_id);
    assert_eq!(sync.calendar_id(), &second_default);
}

#[test]
fn connecting_from_a_source_with_unknown_account_id_fails() {
    let fixture = Fixture::start();
    let source = TestSource::new()
        .at(fixture.server.origin())
        .identity("non-existent-account-id/some-calendar");
    let mut outs = ConnectOuts::default();

    let sync = connect_from(&source, &mut outs);
    assert!(sync.is_none(), "connection should fail for unknown account");
    assert!(!outs.error.is_null());
    unsafe { outs.take_error(eds_sys::E_CLIENT_ERROR_OTHER_ERROR) };
}
