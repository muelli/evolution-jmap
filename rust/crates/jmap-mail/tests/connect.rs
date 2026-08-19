// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening a JMAP mail account against `jmap-mockd`, and the connection the
//! store keeps between `connect_sync` and `disconnect_sync`.
//!
//! The sibling of `jmap-backend-book`'s `connect` test, and the differences
//! between the two are the point of most of what follows: Camel classifies a
//! failed connection with a three-valued `CamelAuthenticationResult` where EDS
//! uses a four-valued `ESourceAuthenticationResult`, and it branches on its own
//! `CAMEL_SERVICE_ERROR` codes where EDS branches on `E_CLIENT_ERROR` ones. The
//! one thing that must *not* differ is which failure means "the password was
//! wrong", which is why that question is asked of `jmap-backend-core` on both
//! sides and only checked here.

use eds_sys::{
    CAMEL_AUTHENTICATION_ACCEPTED, CAMEL_AUTHENTICATION_ERROR, CAMEL_AUTHENTICATION_REJECTED,
    CAMEL_FOLDER_ERROR_INVALID_UID, CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
    CAMEL_SERVICE_ERROR_INVALID, CAMEL_SERVICE_ERROR_UNAVAILABLE, CAMEL_SERVICE_ERROR_URL_INVALID,
    camel_folder_error_quark, camel_service_error_quark,
};
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_client::{Client, Credentials, Error};
use jmap_mail::connect::{ACCEPTED_AUTHENTICATION, StoreError, open_mail, password_credentials};
use jmap_mail::server::ServerConfig;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::{MailSync, SyncError};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_MAIL;

fn config(server: &MockServer) -> ServerConfig {
    ServerConfig {
        target: ConnectTarget::Origin(server.origin().to_owned()),
        user: None,
    }
}

fn open(config: &ServerConfig, password: Option<&str>) -> Result<MailSync, StoreError> {
    open_mail(
        config,
        password_credentials(config.user.as_deref(), password),
    )
}

/// `MailSync` is not `Debug`, and naming the account it opened is a more useful
/// failure message than the type name would have been anyway.
fn expect_error(result: Result<MailSync, StoreError>) -> StoreError {
    match result {
        Ok(sync) => panic!(
            "expected a failure, but opened account {}",
            sync.account_id()
        ),
        Err(error) => error,
    }
}

#[test]
fn an_account_opens_on_the_primary_mail_account_the_session_names() {
    let server = MockServer::builder().start();
    let sync = open(&config(&server), None).expect("connected");

    assert_eq!(sync.account_id(), &server.account_id());
}

/// A store resolves its account under `urn:ietf:params:jmap:mail`, the way the
/// address book backend resolves its own under `:contacts`. An account that
/// offers only contacts is not a mail account, and saying so is better than
/// listing an empty folder tree.
#[test]
fn an_account_that_offers_no_mail_is_refused() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_MAIL)
        .start();

    let error = expect_error(open(&config(&server), None));
    assert!(
        matches!(error, StoreError::Client(_)),
        "expected a client error, got {error:?}"
    );
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_ERROR);
}

/// RFC 8620 §2: a server "MAY" omit `primaryAccounts` entirely. Such a
/// server is still usable when exactly one personal account offers the
/// capability — the same inference the collection backend (M6) already
/// relies on to decide whether to fan out a mail child at all, so a source
/// that backend created must be able to connect, not fail forever with "no
/// primary account".
#[test]
fn an_account_opens_on_the_sole_personal_mail_account_when_the_server_names_none() {
    let server = MockServer::builder().without_primary_accounts().start();
    let sync = open(&config(&server), None).expect("connected");

    assert_eq!(sync.account_id(), &server.account_id());
}

#[test]
fn the_password_is_sent_as_basic_credentials() {
    let server = MockServer::builder().basic_auth("vera", "hunter2").start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let sync = open(&config, Some("hunter2")).expect("connected");
    assert_eq!(sync.account_id(), &server.account_id());
}

/// The other half of the credentials story: an account that authenticates
/// with OAuth 2.0 reaches the server as `Authorization: Bearer …`, with no
/// user name involved at all. Which accounts those are, and where the token
/// comes from, is `jmap_mail::oauth2`'s — what is proved here is that a
/// resolved bearer credential survives the trip to the wire, the same way
/// the password does, and the same way `jmap-backend-book`'s
/// `an_access_token_is_sent_as_bearer_credentials` proves it for the EDS
/// side.
#[test]
fn an_access_token_is_sent_as_bearer_credentials() {
    let server = MockServer::builder().bearer_token("ya29.a0Af").start();

    let sync = open_mail(&config(&server), Credentials::bearer("ya29.a0Af")).expect("connected");
    assert_eq!(sync.account_id(), &server.account_id());
}

/// Camel re-prompts on `REJECTED` and gives up on `ERROR`, so a wrong password
/// has to be told apart from a broken server — the same distinction the EDS
/// backends draw, over a different enum.
#[test]
fn a_wrong_password_is_reported_as_rejected_so_camel_re_prompts() {
    let server = MockServer::builder().basic_auth("vera", "hunter2").start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, Some("wrong")));
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_REJECTED);
}

/// `CamelAuthenticationResult` has no `REQUIRED`, so there is nothing for a
/// store to answer "ask the user first" with. The prompt is what a `REJECTED`
/// produces, and the 401 the server sends back is what produces that — so an
/// account with a user and no password yet has to reach the server rather than
/// refuse in advance, which is the opposite of what the EDS side does.
#[test]
fn a_user_with_no_password_is_refused_by_the_server_rather_than_in_advance() {
    let server = MockServer::builder().basic_auth("vera", "hunter2").start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, None));
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_REJECTED);
}

/// And it reaches the server *without* the user's name attached to an empty
/// password: a server that counts failed attempts must not be handed one for
/// an account that simply has not been asked for its password yet.
#[test]
fn no_password_means_no_credentials_rather_than_an_empty_one() {
    // A server that would *accept* `vera` with an empty password, which is
    // what makes the refusal below evidence rather than a coincidence.
    let server = MockServer::builder().basic_auth("vera", "").start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, None));
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_REJECTED);
}

/// A server that is down, or one that says the account may not do this, is not
/// a password problem: re-prompting cannot fix either, and doing so forever is
/// how an account becomes unusable.
#[test]
fn only_a_401_makes_camel_ask_for_the_password_again() {
    assert_eq!(
        StoreError::Client(Error::Http {
            status: 401,
            problem: None,
        })
        .authentication_result(),
        CAMEL_AUTHENTICATION_REJECTED
    );
    for error in [
        StoreError::Client(Error::Http {
            status: 403,
            problem: None,
        }),
        StoreError::Client(Error::Transport("down".to_owned())),
        StoreError::Client(Error::Cancelled),
        StoreError::Config(SourceError::MissingHost),
        // Not a wrong password, and reported the same way: see
        // `StoreError::OAuth2`'s own doc comment for why a failed token
        // fetch is `ERROR` rather than `REJECTED` here, the opposite of the
        // EDS side's choice for the identical failure.
        StoreError::OAuth2("no token".to_owned()),
    ] {
        assert_eq!(
            error.authentication_result(),
            CAMEL_AUTHENTICATION_ERROR,
            "authentication result for {error}"
        );
    }
}

#[test]
fn an_unreachable_server_is_an_error_not_a_credentials_problem() {
    let config = ServerConfig {
        // Port 1 is reserved and nothing listens there.
        target: ConnectTarget::Origin("http://127.0.0.1:1".to_owned()),
        user: None,
    };
    let error = expect_error(open(&config, None));
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_ERROR);
}

#[test]
fn a_successful_connect_reports_accepted() {
    assert_eq!(ACCEPTED_AUTHENTICATION, CAMEL_AUTHENTICATION_ACCEPTED);
}

/// The codes are not decoration. `UNAVAILABLE` is what tells Camel the server
/// is unreachable — the mail-side equivalent of `E_CLIENT_ERROR_REPOSITORY_OFFLINE`,
/// and the difference between a store that serves its summary cache and one
/// that reports the account as broken. `CANT_AUTHENTICATE` and `URL_INVALID`
/// are what a user is shown, and `URL_INVALID` in particular says "edit the
/// account" where `UNAVAILABLE` says "try later".
#[test]
fn each_failure_carries_the_camel_service_error_code_evolution_routes_on() {
    for (error, expected) in [
        (
            StoreError::Config(SourceError::MissingHost),
            CAMEL_SERVICE_ERROR_URL_INVALID,
        ),
        (
            StoreError::Config(SourceError::InsecureTransport(
                "jmap.example.com".to_owned(),
            )),
            CAMEL_SERVICE_ERROR_URL_INVALID,
        ),
        (
            StoreError::Client(Error::Transport("down".to_owned())),
            CAMEL_SERVICE_ERROR_UNAVAILABLE,
        ),
        (
            StoreError::Client(Error::Http {
                status: 401,
                problem: None,
            }),
            CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
        ),
        (
            StoreError::Client(Error::Http {
                status: 500,
                problem: None,
            }),
            CAMEL_SERVICE_ERROR_INVALID,
        ),
        // An address the account has no identity for. `INVALID` rather than
        // `UNAVAILABLE`: the account works, the server is there, and this send
        // would fail the same way against every one of them — a retry, which is
        // what `UNAVAILABLE` asks Evolution for, cannot help.
        (
            StoreError::NoIdentity("alice@example.com".to_owned()),
            CAMEL_SERVICE_ERROR_INVALID,
        ),
        // The account cannot prove who it is, same as a wrong password —
        // `CANT_AUTHENTICATE` rather than the generic `INVALID` says so.
        (
            StoreError::OAuth2("no token".to_owned()),
            CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
        ),
    ] {
        let gerror = error.to_gerror();
        assert!(!gerror.is_null(), "no GError for {error}");
        // SAFETY: `to_gerror` handed over an owned GError, freed below.
        unsafe {
            assert_eq!(
                (*gerror).domain,
                camel_service_error_quark(),
                "domain for {error}"
            );
            // `GError.code` is a plain int; the CamelServiceError enum is
            // unsigned, which is bindgen's reading of the C enum.
            assert_eq!((*gerror).code, expected as i32, "code for {error}");
            assert!(!(*gerror).message.is_null(), "no message for {error}");
            glib_sys::g_error_free(gerror);
        }
    }
}

/// A uid the account no longer holds is the folder's domain, not the service's.
///
/// The same reasoning as [`StoreError::NoFolder`]'s, one level down: nothing is
/// wrong with the connection, and a service error would report a working
/// account as broken because one message went away between a listing and a
/// click. `CAMEL_FOLDER_ERROR_INVALID_UID` is the code Camel's own providers
/// use for it, and Evolution reads it as "that message is gone" rather than as
/// a reason to take the account offline.
#[test]
fn a_message_the_account_no_longer_holds_is_reported_in_camels_folder_domain() {
    let error = StoreError::NoMessage("E17".to_owned());
    let gerror = error.to_gerror();
    assert!(!gerror.is_null());

    // SAFETY: `to_gerror` handed over an owned GError, freed below.
    unsafe {
        assert_eq!((*gerror).domain, camel_folder_error_quark());
        assert_eq!((*gerror).code, CAMEL_FOLDER_ERROR_INVALID_UID as i32);
        glib_sys::g_error_free(gerror);
    }
    // The uid is in the message: a mail folder holds thousands of them, and
    // "invalid uid" without one is a report nobody can act on.
    assert!(error.to_string().contains("E17"), "{error}");
}

/// The crate boundary the distinction has to survive: `jmap-mail-sync` answers
/// a missing message with a variant of its own precisely so that this mapping
/// can exist, and a `From` that flattened it into a client error would put the
/// account back in the service domain without changing a line of the test
/// above.
#[test]
fn a_sync_layers_missing_message_stays_a_missing_message() {
    let error = StoreError::from(SyncError::NoSuchMessage(Id::new("E17")));

    match error {
        StoreError::NoMessage(uid) => assert_eq!(uid, "E17"),
        other => panic!("expected a missing message, got {other}"),
    }
}

/// The same crate boundary again, for the address the account cannot send as.
///
/// Flattened into a client error it would reach Evolution as a server that said
/// no, which is the one thing it is not: the server was never asked. The
/// address survives the boundary because it is the whole of what the user has
/// to act on.
#[test]
fn a_sync_layers_missing_identity_stays_a_missing_identity() {
    let error = StoreError::from(SyncError::NoIdentity("alice@example.com".to_owned()));

    match &error {
        StoreError::NoIdentity(address) => assert_eq!(address, "alice@example.com"),
        other => panic!("expected a missing identity, got {other}"),
    }
    assert!(error.to_string().contains("alice@example.com"), "{error}");
}

/// The one failure that is not Camel's to classify: the user pressed Stop, and
/// every caller in Camel tests for it with `g_error_matches (error, G_IO_ERROR,
/// G_IO_ERROR_CANCELLED)` before deciding anything went wrong at all.
#[test]
fn a_cancelled_connect_is_reported_in_glibs_own_domain() {
    let gerror = StoreError::Client(Error::Cancelled).to_gerror();
    // SAFETY: an owned GError, freed below.
    unsafe {
        assert_eq!((*gerror).domain, gio_sys::g_io_error_quark());
        assert_eq!((*gerror).code, gio_sys::G_IO_ERROR_CANCELLED);
        glib_sys::g_error_free(gerror);
    }
}

// ---------------------------------------------------------------------------
// the connection a store holds between connect and disconnect

fn sync_against(server: &MockServer) -> MailSync {
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    MailSync::new(client, server.account_id())
}

#[test]
fn a_fresh_store_holds_no_connection() {
    let store = JmapStore::detached();
    assert!(!store.is_connected());
}

#[test]
fn a_stored_connection_is_what_the_store_reports() {
    let server = MockServer::builder().start();
    let store = JmapStore::detached();

    store.store_connection(sync_against(&server));
    assert!(store.is_connected());
}

/// `connect_sync` is reached again after Camel decides the connection is gone,
/// and the old one is exactly what is being replaced — refusing would leave the
/// store pointing at a socket nobody is listening on.
#[test]
fn connecting_twice_replaces_the_connection_rather_than_refusing() {
    let server = MockServer::builder().start();
    let store = JmapStore::detached();

    store.store_connection(sync_against(&server));
    store.store_connection(sync_against(&server));
    assert!(store.is_connected());
}

/// Camel asks a store to disconnect on shutdown whether or not it ever
/// connected, so dropping nothing is a normal outcome and not a failure — but
/// the two are told apart, because "there was one" is what a `disconnect_sync`
/// needs in order to know it did anything.
#[test]
fn dropping_the_connection_reports_whether_there_was_one() {
    let server = MockServer::builder().start();
    let store = JmapStore::detached();
    assert!(!store.drop_connection());

    store.store_connection(sync_against(&server));
    assert!(store.drop_connection());
    assert!(!store.is_connected());
    assert!(!store.drop_connection());
}

/// The store's side of reading a message: the folder holds the uid and the
/// store holds the connection, so this is where the two meet — the same split
/// a refresh already goes through.
#[test]
fn a_connected_store_hands_over_the_bytes_of_a_message() {
    let server = MockServer::builder().start();
    let uid = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&server.account_id()).unwrap();
        let mailbox = account.seed_mailbox("Inbox", Some("inbox"));
        account.seed_email(EmailSeed::new(
            mailbox,
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        ))
    };
    let store = JmapStore::detached();
    store.store_connection(sync_against(&server));

    let source = store.message_source(&uid).expect("the message was fetched");
    let source = String::from_utf8(source).expect("a text message");
    assert!(source.contains("Subject: Lunch?"), "{source}");
}

/// A message asked for over a store that has no connection is the disconnected
/// case, not a missing message: Camel drives a store it believes is connected,
/// and reporting the uid as gone would make Evolution drop a row that is fine.
#[test]
fn a_disconnected_store_has_no_message_to_hand_over() {
    let store = JmapStore::detached();

    match store.message_source(&Id::new("E1")) {
        Err(StoreError::Disconnected) => {}
        Err(other) => panic!("expected the store to report being disconnected, got {other}"),
        Ok(_) => panic!("a store with no connection fetched a message"),
    }
}
