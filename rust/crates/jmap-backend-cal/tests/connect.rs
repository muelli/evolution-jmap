// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `connect_sync`, minus the GObject: turning a [`SourceConfig`] and whatever
//! EDS got out of libsecret into a [`CalSync`], against `jmap-mockd`.
//!
//! The mirror of `jmap-backend-book`'s `connect` test, and deliberately so:
//! everything but "which collection is being resolved" now lives in
//! `jmap-backend-core::connect`, and two tests that disagree about the rules
//! would be the first sign that a backend grew its own.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_REQUIRED, E_CLIENT_ERROR_DBUS_ERROR, E_CLIENT_ERROR_INVALID_ARG,
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, E_SOURCE_AUTHENTICATION_ACCEPTED,
    E_SOURCE_AUTHENTICATION_ERROR, E_SOURCE_AUTHENTICATION_REJECTED,
    E_SOURCE_AUTHENTICATION_REQUIRED, E_SOURCE_CREDENTIAL_PASSWORD,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_RESOURCE, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceAuthenticationResult, e_client_error_quark,
    e_named_parameters_free, e_named_parameters_new, e_named_parameters_set,
    e_source_authentication_set_host, e_source_authentication_set_method,
    e_source_authentication_set_port, e_source_authentication_set_user, e_source_get_extension,
    e_source_new_with_uid, e_source_resource_set_identity, e_source_security_set_secure,
};
use glib_sys::GError;
use gobject_sys::g_object_unref;
use jmap_backend_cal::connect;
use jmap_backend_core::api_token::API_TOKEN_METHOD;
use jmap_backend_core::connect::{Collection, ConnectError, credentials};
use jmap_backend_core::source::{ConnectTarget, SourceConfig};
use jmap_cal_sync::CalSync;
use jmap_client::Credentials;
use jmap_mock::MockServer;
use jmap_proto::Id;

struct Fixture {
    server: MockServer,
    default_calendar: Option<Id>,
    other_calendar: Id,
}

impl Fixture {
    /// Two calendars; `default_calendar` is the one flagged `isDefault`.
    fn start(with_default: bool) -> Self {
        Self::start_with(MockServer::builder(), with_default)
    }

    /// The non-default calendar is seeded *first* on purpose, so that resolving
    /// "the default one" as "the first one" is a visible failure rather than an
    /// accident that happens to pass.
    fn start_with(builder: jmap_mock::MockServerBuilder, with_default: bool) -> Self {
        let server = builder.start();
        let account_id = server.account_id();
        let (other_calendar, default_calendar) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            (
                account.seed_calendar("Team", false),
                with_default.then(|| account.seed_calendar("Personal", true)),
            )
        };
        Self {
            server,
            default_calendar,
            other_calendar,
        }
    }

    fn config(&self) -> SourceConfig {
        SourceConfig {
            target: ConnectTarget::Origin(self.server.origin().to_owned()),
            user: None,
            resource_id: None,
            rebase_urls: false,
        }
    }
}

/// `open_calendar` takes resolved credentials; this stands in for the caller
/// that resolves them, which is `connect_with`. See the address book backend's
/// copy of this helper for why it takes the password path, and
/// `jmap-backend-core/tests/oauth2.rs` for the choice between the two.
fn open(config: &SourceConfig, password: Option<&str>) -> Result<CalSync, ConnectError> {
    connect::open_calendar(config, credentials(config.user.as_deref(), password)?)
}

/// `CalSync` is not `Debug`, and naming the calendar it opened is a more useful
/// failure message than the type name would have been anyway.
fn expect_error(result: Result<CalSync, ConnectError>) -> ConnectError {
    match result {
        Ok(sync) => panic!("expected a failure, but opened {}", sync.calendar_id()),
        Err(error) => error,
    }
}

#[test]
fn a_source_that_names_no_calendar_gets_the_default_one() {
    let fixture = Fixture::start(true);
    let sync = open(&fixture.config(), None).expect("connected");

    assert_eq!(sync.calendar_id(), &fixture.default_calendar.unwrap());
    assert_eq!(sync.account_id(), &fixture.server.account_id());
}

#[test]
fn a_source_that_names_a_calendar_gets_that_one() {
    let fixture = Fixture::start(true);
    let mut config = fixture.config();
    config.resource_id = Some(fixture.other_calendar.to_string());

    let sync = open(&config, None).expect("connected");
    assert_eq!(sync.calendar_id(), &fixture.other_calendar);
}

/// A typo in a hand-written `.source` must not present as a calendar that is
/// merely empty — the user would have no way to tell the difference from a
/// server that lost their appointments.
#[test]
fn a_calendar_the_server_does_not_have_is_refused() {
    let fixture = Fixture::start(true);
    let mut config = fixture.config();
    config.resource_id = Some("Cal-nonesuch".to_owned());

    match expect_error(open(&config, None)) {
        ConnectError::NoSuchCollection(Collection::Calendar, id) => assert_eq!(id, "Cal-nonesuch"),
        other => panic!("expected NoSuchCollection(Calendar), got {other:?}"),
    }
}

#[test]
fn an_account_with_no_default_calendar_is_an_error_not_a_guess() {
    let fixture = Fixture::start(false);
    match expect_error(open(&fixture.config(), None)) {
        ConnectError::NoDefaultCollection(Collection::Calendar) => {}
        other => panic!("expected NoDefaultCollection(Calendar), got {other:?}"),
    }
}

/// The calendar resolves its account under `urn:ietf:params:jmap:calendars`.
/// A server that offers contacts but not calendars must be refused rather than
/// synced against whatever account the other capability happens to name.
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

/// RFC 8620 §2: a server "MAY" omit `primaryAccounts` entirely. Such a
/// server is still usable when exactly one personal account offers the
/// capability — the same inference the collection backend (M6) already
/// relies on to decide whether to fan out a calendar child at all, so a
/// source that backend created must be able to connect, not fail forever
/// with "no primary account".
#[test]
fn a_server_with_no_primary_accounts_resolves_the_sole_personal_calendars_account() {
    let fixture = Fixture::start_with(MockServer::builder().without_primary_accounts(), true);

    let sync = open(&fixture.config(), None).expect("connected");
    assert_eq!(sync.account_id(), &fixture.server.account_id());
}

#[test]
fn a_user_name_with_no_password_asks_evolution_for_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().basic_auth("vera", "hunter2"), true);
    let mut config = fixture.config();
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, None));
    assert!(matches!(error, ConnectError::CredentialsRequired));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);
}

/// The mirror of the address book backend's bearer test — see it for what this
/// is proving and what it deliberately is not.
#[test]
fn an_access_token_is_sent_as_bearer_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().bearer_token("ya29.a0Af"), true);

    let sync = connect::open_calendar(&fixture.config(), Credentials::bearer("ya29.a0Af"))
        .expect("connected");
    assert_eq!(sync.calendar_id(), &fixture.default_calendar.unwrap());
}

/// The EDS-side sibling of `jmap_mail::connect::StoreError`'s reclassification:
/// a 401 on a bearer token this backend itself just sent must not be treated
/// like a wrong Basic password, because `REJECTED` here tells EDS to throw away
/// a refresh token a transient rejection has not invalidated.
#[test]
fn a_401_on_a_bearer_token_reclassifies_to_required_not_rejected() {
    let fixture = Fixture::start_with(MockServer::builder().bearer_token("good-token"), true);

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

/// The failures all have to reach Evolution as a `GError` too, and the code
/// is not decoration: `AUTHENTICATION_REQUIRED` is what makes Evolution offer
/// a password prompt, and `REPOSITORY_OFFLINE` is what makes the meta backend
/// serve its cache instead of showing an empty calendar.
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
            // `GError.code` is a plain int; the EClientError enum is
            // unsigned, which is bindgen's reading of the C enum.
            assert_eq!((*gerror).code, expected as i32, "code for {error}");
            assert!(!(*gerror).message.is_null(), "no message for {error}");
            glib_sys::g_error_free(gerror);
        }
    }
}

#[test]
fn the_password_is_sent_as_basic_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().basic_auth("vera", "hunter2"), true);
    let mut config = fixture.config();
    config.user = Some("vera".to_owned());

    let sync = open(&config, Some("hunter2")).expect("connected");
    assert_eq!(sync.calendar_id(), &fixture.default_calendar.unwrap());
}

/// EDS re-prompts on `REJECTED` and gives up on `ERROR`, so a wrong password
/// has to be told apart from a broken server.
#[test]
fn a_wrong_password_is_reported_as_rejected_so_evolution_re_prompts() {
    let fixture = Fixture::start_with(MockServer::builder().basic_auth("vera", "hunter2"), true);
    let mut config = fixture.config();
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, Some("wrong")));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_REJECTED);
}

#[test]
fn an_unreachable_server_is_an_error_not_a_credentials_problem() {
    let config = SourceConfig {
        // Port 1 is reserved and nothing listens there.
        target: ConnectTarget::Origin("http://127.0.0.1:1".to_owned()),
        user: None,
        resource_id: None,
        rebase_urls: false,
    };
    let error = expect_error(open(&config, None));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_ERROR);
}

// ---------------------------------------------------------------------------
// `connect_sync` from the ESource down, which is everything the subclass does
// not do itself.

/// An `ESource` that is not backed by the registry. `e_source_new_with_uid`
/// with a NULL D-Bus object is what EDS itself uses for a source read from a
/// keyfile, so the extension machinery behaves as it does inside a backend.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        let uid = CString::new("jmap-cal-connect-test").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a NULL
        // GError out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn authentication(&self) -> *mut ESourceAuthentication {
        // SAFETY: the source is alive and the name is a header constant; the
        // extension is created on demand and owned by the source.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) }.cast()
    }

    /// Points the source at `origin`, which the mock server hands out as
    /// `http://127.0.0.1:<port>`. TLS is switched off explicitly, which
    /// `SourceConfig` only tolerates because the host is loopback.
    fn at(self, origin: &str) -> Self {
        let (host, port) = origin
            .trim_start_matches("http://")
            .split_once(':')
            .expect("the mock origin has a port");
        let host = CString::new(host).expect("no NUL in a host");
        // SAFETY: live extensions and a NUL-terminated string the setter
        // copies.
        unsafe {
            e_source_authentication_set_host(self.authentication(), host.as_ptr());
            e_source_authentication_set_port(self.authentication(), port.parse().expect("a port"));
            e_source_security_set_secure(
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast(),
                0,
            );
        }
        self
    }

    /// `ESourceResource:identity` — the one field whose *meaning* differs
    /// between the two backends, which is why `SourceConfig` calls it
    /// `resource_id` and neither an address book nor a calendar.
    fn identity(self, id: &str) -> Self {
        let id = CString::new(id).expect("no NUL in an identity");
        // SAFETY: as `at`.
        unsafe {
            e_source_resource_set_identity(
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_RESOURCE.as_ptr()).cast(),
                id.as_ptr(),
            )
        };
        self
    }

    fn user(self, user: &str) -> Self {
        let user = CString::new(user).expect("no NUL in a user");
        // SAFETY: as `at`.
        unsafe { e_source_authentication_set_user(self.authentication(), user.as_ptr()) };
        self
    }

    fn method(self, method: &str) -> Self {
        let method = CString::new(method).expect("no NUL in a method");
        // SAFETY: as `at`.
        unsafe { e_source_authentication_set_method(self.authentication(), method.as_ptr()) };
        self
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: the reference from e_source_new_with_uid is given back once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The out-parameters EDS hands `connect_sync`, started at values it never
/// passes: a body that writes neither would otherwise be indistinguishable
/// from one that answers correctly.
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
    /// Asserts the reported domain and code, and frees the error.
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

fn connect_from(source: &TestSource, outs: &mut ConnectOuts) -> Option<CalSync> {
    // SAFETY: a live ESource, no credentials, no cancellable, and two writable
    // out-parameters — what EDS passes on a first connect.
    unsafe {
        connect::connect(
            source.0,
            ptr::null(),
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    }
}

#[test]
fn connecting_from_a_source_opens_the_default_calendar_and_reports_accepted() {
    let fixture = Fixture::start(true);
    let source = TestSource::new().at(fixture.server.origin());
    let mut outs = ConnectOuts::default();

    let sync = connect_from(&source, &mut outs).expect("the connection was refused");
    assert_eq!(
        sync.calendar_id(),
        fixture.default_calendar.as_ref().expect("seeded")
    );
    assert!(outs.error.is_null(), "a successful connect set an error");
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
}

/// `[Resource] Identity=` is a calendar id here and an address book id in the
/// other backend. Reading it as anything else — or not reading it at all —
/// would silently sync the default calendar instead of the named one.
#[test]
fn the_resource_identity_selects_the_calendar() {
    let fixture = Fixture::start(true);
    let source = TestSource::new()
        .at(fixture.server.origin())
        .identity(fixture.other_calendar.as_ref());
    let mut outs = ConnectOuts::default();

    let sync = connect_from(&source, &mut outs).expect("the connection was refused");
    assert_eq!(sync.calendar_id(), &fixture.other_calendar);
}

/// The prompt has to happen before anything is sent, so a source that names a
/// user and has no password yet must not reach the server at all — which is
/// what asserting on a server that was never started proves.
#[test]
fn a_source_with_a_user_and_no_password_asks_for_one_before_connecting() {
    let source = TestSource::new().at("http://127.0.0.1:1").user("vera");
    let mut outs = ConnectOuts::default();

    assert!(connect_from(&source, &mut outs).is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_REQUIRED);
    // SAFETY: the call failed, so it owns an error it handed over.
    unsafe { outs.take_error(E_CLIENT_ERROR_AUTHENTICATION_REQUIRED) };
}

/// The API-token method's whole point: an account with no user name at all
/// reaches the server as `Authorization: Bearer …`, off the same
/// `E_SOURCE_CREDENTIAL_PASSWORD` slot Basic reads its password from — proved
/// end to end, `[Authentication] Method` on a real `ESource` down to the
/// wire, the way `connecting_from_a_source_opens_the_default_calendar_and_reports_accepted`
/// proves the password path.
#[test]
fn an_api_token_source_sends_the_stored_secret_as_bearer_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().bearer_token("t0k3n"), true);
    let source = TestSource::new()
        .at(fixture.server.origin())
        .method(API_TOKEN_METHOD.to_str().unwrap());
    let mut outs = ConnectOuts::default();

    // SAFETY: a live `ENamedParameters`, freed below.
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

    // SAFETY: a live ESource, the credentials just built, no cancellable and
    // two writable out-parameters — what EDS passes on a re-connect with a
    // prompted secret in hand.
    let sync = unsafe {
        connect::connect(
            source.0,
            credentials,
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    };
    // SAFETY: this test owns the only reference.
    unsafe { e_named_parameters_free(credentials) };

    let sync = sync.expect("the connection was refused");
    assert_eq!(
        sync.calendar_id(),
        fixture.default_calendar.as_ref().expect("seeded")
    );
    assert!(outs.error.is_null(), "a successful connect set an error");
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
}

/// The same prompt-before-sending rule the password path follows: an
/// API-token account with no stored secret yet must not reach the server
/// either.
#[test]
fn an_api_token_source_with_no_stored_secret_asks_for_one_before_connecting() {
    let source = TestSource::new()
        .at("http://127.0.0.1:1")
        .method(API_TOKEN_METHOD.to_str().unwrap());
    let mut outs = ConnectOuts::default();

    // SAFETY: as `a_source_with_a_user_and_no_password_asks_for_one_before_connecting`.
    let sync = unsafe {
        connect::connect(
            source.0,
            ptr::null(),
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    };

    assert!(sync.is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_REQUIRED);
    // SAFETY: the call failed, so it owns an error it handed over.
    unsafe { outs.take_error(E_CLIENT_ERROR_AUTHENTICATION_REQUIRED) };
}

/// An OAuth 2.0 source with no token (e.g. fresh account or no registered service)
/// must fail with REQUIRED to trigger the consent window.
#[test]
fn an_oauth2_source_with_no_token_asks_for_consent_before_connecting() {
    let source = TestSource::new().at("http://127.0.0.1:1").method("OAuth2");
    let mut outs = ConnectOuts::default();

    // SAFETY: as above.
    let sync = unsafe {
        connect::connect(
            source.0,
            ptr::null(),
            ptr::null_mut(),
            &mut outs.auth_result,
            &mut outs.error,
        )
    };

    assert!(sync.is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_REQUIRED);
    // SAFETY: the call failed, so it owns an error it handed over.
    unsafe { outs.take_error(E_CLIENT_ERROR_AUTHENTICATION_REQUIRED) };
}

/// A source with no host is a misconfigured account: no password prompt fixes
/// it, so it must not be reported as a credentials problem.
#[test]
fn a_source_that_names_no_server_is_an_error_rather_than_a_prompt() {
    let source = TestSource::new();
    let mut outs = ConnectOuts::default();

    assert!(connect_from(&source, &mut outs).is_none());
    assert_eq!(outs.auth_result, E_SOURCE_AUTHENTICATION_ERROR);
    // SAFETY: as above.
    unsafe { outs.take_error(E_CLIENT_ERROR_INVALID_ARG) };
}

/// EDS constructs a backend *from* a source, so this cannot happen — but a
/// NULL dereference here takes `evolution-calendar-factory` down and with it
/// every other calendar in the process.
#[test]
fn a_backend_without_a_source_fails_instead_of_dereferencing_null() {
    let mut outs = ConnectOuts::default();

    // SAFETY: a NULL ESource is explicitly allowed by `connect`.
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
    // SAFETY: as above.
    unsafe { outs.take_error(E_CLIENT_ERROR_INVALID_ARG) };
}
