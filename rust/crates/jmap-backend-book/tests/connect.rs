// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `connect_sync`, minus the GObject: turning a [`SourceConfig`] and whatever
//! EDS got out of libsecret into a [`BookSync`], against `jmap-mockd`.

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_REQUIRED, E_CLIENT_ERROR_INVALID_ARG,
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, E_SOURCE_AUTHENTICATION_ACCEPTED,
    E_SOURCE_AUTHENTICATION_ERROR, E_SOURCE_AUTHENTICATION_REJECTED,
    E_SOURCE_AUTHENTICATION_REQUIRED, e_client_error_quark,
};
use jmap_backend_book::connect::{self, ConnectError};
use jmap_backend_core::source::SourceConfig;
use jmap_client::transport::CancelFlag;
use jmap_mock::MockServer;
use jmap_proto::Id;

struct Fixture {
    server: MockServer,
    default_book: Option<Id>,
    other_book: Id,
}

impl Fixture {
    /// Two address books; `default_book` is the one flagged `isDefault`.
    fn start(with_default: bool) -> Self {
        Self::start_with(MockServer::builder(), with_default)
    }

    /// The non-default book is seeded *first* on purpose, so that resolving
    /// "the default one" as "the first one" is a visible failure rather than
    /// an accident that happens to pass.
    fn start_with(builder: jmap_mock::MockServerBuilder, with_default: bool) -> Self {
        let server = builder.start();
        let account_id = server.account_id();
        let (other_book, default_book) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            (
                account.seed_address_book("Shared", false),
                with_default.then(|| account.seed_address_book("Personal", true)),
            )
        };
        Self {
            server,
            default_book,
            other_book,
        }
    }

    fn config(&self) -> SourceConfig {
        SourceConfig {
            origin: self.server.origin().to_owned(),
            user: None,
            address_book_id: None,
        }
    }
}

fn open(
    config: &SourceConfig,
    password: Option<&str>,
) -> Result<jmap_book_sync::BookSync, ConnectError> {
    connect::open_book(config, password, CancelFlag::new())
}

/// `BookSync` is not `Debug`, and naming the address book it opened is a more
/// useful failure message than the type name would have been anyway.
fn expect_error(result: Result<jmap_book_sync::BookSync, ConnectError>) -> ConnectError {
    match result {
        Ok(sync) => panic!("expected a failure, but opened {}", sync.address_book_id()),
        Err(error) => error,
    }
}

#[test]
fn a_source_that_names_no_address_book_gets_the_default_one() {
    let fixture = Fixture::start(true);
    let sync = open(&fixture.config(), None).expect("connected");

    assert_eq!(sync.address_book_id(), &fixture.default_book.unwrap());
    assert_eq!(sync.account_id(), &fixture.server.account_id());
}

#[test]
fn a_source_that_names_an_address_book_gets_that_one() {
    let fixture = Fixture::start(true);
    let mut config = fixture.config();
    config.address_book_id = Some(fixture.other_book.to_string());

    let sync = open(&config, None).expect("connected");
    assert_eq!(sync.address_book_id(), &fixture.other_book);
}

/// A typo in a hand-written `.source` must not present as an address book
/// that is merely empty — the user would have no way to tell the difference
/// from a server that lost their contacts.
#[test]
fn an_address_book_the_server_does_not_have_is_refused() {
    let fixture = Fixture::start(true);
    let mut config = fixture.config();
    config.address_book_id = Some("AB-nonesuch".to_owned());

    match expect_error(open(&config, None)) {
        ConnectError::NoSuchAddressBook(id) => assert_eq!(id, "AB-nonesuch"),
        other => panic!("expected NoSuchAddressBook, got {other:?}"),
    }
}

#[test]
fn an_account_with_no_default_address_book_is_an_error_not_a_guess() {
    let fixture = Fixture::start(false);
    match expect_error(open(&fixture.config(), None)) {
        ConnectError::NoDefaultAddressBook => {}
        other => panic!("expected NoDefaultAddressBook, got {other:?}"),
    }
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

#[test]
fn the_password_is_sent_as_basic_credentials() {
    let fixture = Fixture::start_with(MockServer::builder().basic_auth("vera", "hunter2"), true);
    let mut config = fixture.config();
    config.user = Some("vera".to_owned());

    let sync = open(&config, Some("hunter2")).expect("connected");
    assert_eq!(sync.address_book_id(), &fixture.default_book.unwrap());
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
        origin: "http://127.0.0.1:1".to_owned(),
        user: None,
        address_book_id: None,
    };
    let error = expect_error(open(&config, None));
    assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_ERROR);
}

#[test]
fn a_successful_connect_reports_accepted() {
    assert_eq!(
        connect::ACCEPTED_AUTH_RESULT,
        E_SOURCE_AUTHENTICATION_ACCEPTED
    );
}

/// The failures all have to reach Evolution as a `GError` too, and the code
/// is not decoration: `AUTHENTICATION_REQUIRED` is what makes Evolution offer
/// a password prompt, and `REPOSITORY_OFFLINE` is what makes the meta backend
/// serve its cache instead of showing an empty address book.
#[test]
fn each_failure_carries_the_client_error_code_evolution_routes_on() {
    for (error, expected) in [
        (
            ConnectError::CredentialsRequired,
            E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
        ),
        (
            ConnectError::NoDefaultAddressBook,
            E_CLIENT_ERROR_INVALID_ARG,
        ),
        (
            ConnectError::NoSuchAddressBook("AB-nonesuch".to_owned()),
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
