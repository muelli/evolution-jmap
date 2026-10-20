// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opens a JMAP mail account against `jmap-mockd`.
//!
//! Tests the connection the store keeps between `connect_sync` and
//! `disconnect_sync`.
//!
//! This file is the sibling of `jmap-backend-book`'s `connect` test. Most of
//! what follows covers the differences between the two:
//!
//! - Camel classifies a failed connection with a three-valued
//!   `CamelAuthenticationResult`. EDS uses a four-valued
//!   `ESourceAuthenticationResult`.
//! - Camel branches on its own `CAMEL_SERVICE_ERROR` codes. EDS branches on
//!   `E_CLIENT_ERROR` codes.
//!
//! One thing must not differ: which failure means "the password was wrong".
//! `jmap-backend-core` answers that question on both sides. This file only
//! checks the answer.

use eds_sys::{
    CAMEL_AUTHENTICATION_ACCEPTED, CAMEL_AUTHENTICATION_ERROR, CAMEL_AUTHENTICATION_REJECTED,
    CAMEL_FOLDER_ERROR_INVALID, CAMEL_FOLDER_ERROR_INVALID_UID,
    CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE, CAMEL_SERVICE_ERROR_INVALID,
    CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_SERVICE_ERROR_UNAVAILABLE,
    CAMEL_SERVICE_ERROR_URL_INVALID, CAMEL_STORE_ERROR_NO_FOLDER, camel_folder_error_quark,
    camel_service_error_quark, camel_store_error_quark,
};
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_client::{Client, Credentials, Error};
use jmap_mail::connect::{ACCEPTED_AUTHENTICATION, StoreError, open_mail, password_credentials};
use jmap_mail::server::ServerConfig;
use jmap_mail::store::JmapStore;
use jmap_mail_sync::{FolderRole, MailSync, SyncError};
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

/// `MailSync` does not implement `Debug`. Naming the opened account gives a
/// more useful failure message than the type name would.
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

/// A store resolves its account under `urn:ietf:params:jmap:mail`. The address
/// book backend resolves its own account under `:contacts`.
///
/// An account that offers only contacts is not a mail account. Reporting this
/// is better than listing an empty folder tree.
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

/// RFC 8620 §2 allows a server to omit `primaryAccounts` entirely.
///
/// Such a server is still usable when exactly one personal account offers the
/// capability. The collection backend (M6) already relies on this same
/// inference to decide whether to fan out a mail child. A source that backend
/// created must be able to connect, not fail forever with "no primary
/// account".
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
/// user name involved.
///
/// `jmap_mail::oauth2` decides which accounts these are and where the token
/// comes from. This test only proves that a resolved bearer credential
/// survives the trip to the wire, the same way the password does.
/// `jmap-backend-book`'s `an_access_token_is_sent_as_bearer_credentials` proves
/// the same fact for the EDS side.
#[test]
fn an_access_token_is_sent_as_bearer_credentials() {
    let server = MockServer::builder().bearer_token("ya29.a0Af").start();

    let sync = open_mail(&config(&server), Credentials::bearer("ya29.a0Af")).expect("connected");
    assert_eq!(sync.account_id(), &server.account_id());
}

/// Camel re-prompts on `REJECTED` and gives up on `ERROR`. A wrong password
/// must be told apart from a broken server. The EDS backends draw the same
/// distinction, over a different enum.
#[test]
fn a_wrong_password_is_reported_as_rejected_so_camel_re_prompts() {
    let server = MockServer::builder().basic_auth("vera", "hunter2").start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, Some("wrong")));
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_REJECTED);
}

/// `CamelAuthenticationResult` has no `REQUIRED` value. A store has no way to
/// answer "ask the user first".
///
/// A `REJECTED` result produces the prompt, and a 401 from the server produces
/// `REJECTED`. So an account with a user name and no password yet must reach
/// the server rather than refuse in advance. This is the opposite of what the
/// EDS side does.
#[test]
fn a_user_with_no_password_is_refused_by_the_server_rather_than_in_advance() {
    let server = MockServer::builder().basic_auth("vera", "hunter2").start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, None));
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_REJECTED);
}

/// The request reaches the server without the user name attached to an empty
/// password. A server that counts failed attempts must not receive one for an
/// account that has not been asked for its password yet.
#[test]
fn no_password_means_no_credentials_rather_than_an_empty_one() {
    // This server accepts `vera` with an empty password. That makes the
    // refusal below evidence, not a coincidence.
    let server = MockServer::builder().basic_auth("vera", "").start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, None));
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_REJECTED);
}

/// The test above relies on the server answering a missing `Authorization`
/// header with a real 401. `is_wrong_password` needs that 401 to recognise the
/// refusal.
///
/// Real Stalwart does not send a 401 (RFC 8620 §2/§8.1; Finding 12 in
/// `STALWART-RFC-FINDINGS.md`). It answers 200 OK with an anonymous,
/// empty-accounts session instead. `session_via_redirect`'s mock shape
/// reproduces this byte for byte (see that builder's own doc comment).
///
/// Without `StoreError::Unauthenticated`, that 200 resolves no primary account
/// and reports the generic, unprompted `CAMEL_AUTHENTICATION_ERROR`. So the
/// first connection attempt a fresh account makes against Stalwart would
/// dead-end instead of prompting for a password.
#[test]
fn an_anonymous_session_from_a_server_that_never_sends_401_is_still_reported_as_rejected() {
    let server = MockServer::builder()
        .basic_auth("vera", "hunter2")
        .session_via_redirect()
        .start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let error = expect_error(open(&config, None));
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_REJECTED);
}

/// A server can authenticate the session via a redirect without dropping the
/// credential. `session_via_redirect` already pins this ordinary case
/// elsewhere. This case must not be swept into the anonymous case.
///
/// An account whose connection genuinely succeeded must still resolve its
/// account the usual way, not refuse with `Unauthenticated`.
#[test]
fn an_authenticated_session_through_a_redirect_is_not_reported_as_anonymous() {
    let server = MockServer::builder()
        .basic_auth("vera", "hunter2")
        .session_via_redirect()
        .start();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let sync = open(&config, Some("hunter2")).expect("connected");
    assert_eq!(sync.account_id(), &server.account_id());
}

/// A server that is down, or one that refuses the action, is not a password
/// problem. Re-prompting cannot fix either case, and re-prompting forever
/// makes the account unusable.
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
        // Not a wrong password, but reported the same way. See
        // `StoreError::OAuth2`'s own doc comment for why a failed token
        // fetch is `ERROR` here, not `REJECTED`. The EDS side makes the
        // opposite choice for the identical failure.
        StoreError::OAuth2("no token".to_owned()),
    ] {
        assert_eq!(
            error.authentication_result(),
            CAMEL_AUTHENTICATION_ERROR,
            "authentication result for {error}"
        );
    }
}

/// The test above reports the same 401 as `REJECTED` for a plain password.
/// For a bearer token EDS itself supplied, the 401 means something different.
/// See `StoreError::reclassify_oauth2_rejection`'s own doc for why.
///
/// `attempt`'s OAuth 2.0 branch is the one caller, and it needs a live
/// `CamelSession` to reach. This test proves the reclassification itself, on
/// the same `StoreError` variant the OAuth 2.0 path would actually receive
/// from `open_mail`: a real 401 from a real (mock) server, not a hand-built
/// one.
#[test]
fn a_401_on_a_bearer_token_reclassifies_to_the_same_oauth2_failure_a_fetch_would_get() {
    let server = MockServer::builder().bearer_token("good-token").start();

    let error = expect_error(open_mail(
        &config(&server),
        Credentials::bearer("wrong-token"),
    ));
    assert!(
        matches!(error, StoreError::Client(_)),
        "expected a client error, got {error}"
    );
    assert_eq!(error.authentication_result(), CAMEL_AUTHENTICATION_REJECTED);

    let reclassified = error.reclassify_oauth2_rejection();
    assert!(
        matches!(reclassified, StoreError::OAuth2(_)),
        "expected OAuth2, got {reclassified}"
    );
    assert_eq!(
        reclassified.authentication_result(),
        CAMEL_AUTHENTICATION_ERROR
    );
}

/// Reclassification is narrow. A `Client` failure that is not a 401 passes
/// through unchanged. `only_a_401_makes_camel_ask_for_the_password_again`
/// already pins this shape as `ERROR` regardless.
///
/// This keeps a genuinely broken OAuth 2.0 account (an unreachable server, a
/// cancelled request) from silently turning into a different `ERROR` that
/// would read oddly to whoever debugs it.
#[test]
fn only_the_401_shape_is_reclassified() {
    for error in [
        StoreError::Client(Error::Http {
            status: 403,
            problem: None,
        }),
        StoreError::Client(Error::Transport("down".to_owned())),
        StoreError::Client(Error::Cancelled),
    ] {
        let reclassified = error.reclassify_oauth2_rejection();
        assert!(
            matches!(reclassified, StoreError::Client(_)),
            "expected the error left alone, got {reclassified}"
        );
    }
}

#[test]
fn an_unreachable_server_is_an_error_not_a_credentials_problem() {
    let config = ServerConfig {
        // Port 1 is reserved. Nothing listens there.
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

/// The codes are not decoration. `UNAVAILABLE` tells Camel the server is
/// unreachable: the mail-side equivalent of
/// `E_CLIENT_ERROR_REPOSITORY_OFFLINE`. It is the difference between a store
/// that serves its summary cache and one that reports the account as broken.
///
/// `CANT_AUTHENTICATE` and `URL_INVALID` are shown to the user. `URL_INVALID`
/// says "edit the account". `UNAVAILABLE` says "try later".
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
            StoreError::Unauthenticated,
            CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
        ),
        (
            StoreError::Client(Error::Http {
                status: 500,
                problem: None,
            }),
            CAMEL_SERVICE_ERROR_INVALID,
        ),
        // An address the account has no identity for. `INVALID`, not
        // `UNAVAILABLE`: the account works, the server is there, and the send
        // would fail the same way every time. `UNAVAILABLE` asks Evolution to
        // retry, which cannot help here.
        (
            StoreError::NoIdentity("alice@example.com".to_owned()),
            CAMEL_SERVICE_ERROR_INVALID,
        ),
        // The account cannot prove who it is, the same as a wrong password.
        // `CANT_AUTHENTICATE`, not the generic `INVALID`, reports this.
        (
            StoreError::OAuth2("no token".to_owned()),
            CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
        ),
        // Camel's own cue to reconnect and ask again, not to show the
        // account as broken.
        (StoreError::Disconnected, CAMEL_SERVICE_ERROR_NOT_CONNECTED),
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
            // `GError.code` is a plain int. The `CamelServiceError` enum is
            // unsigned: bindgen reads the C enum that way.
            assert_eq!((*gerror).code, expected as i32, "code for {error}");
            assert!(!(*gerror).message.is_null(), "no message for {error}");
            glib_sys::g_error_free(gerror);
        }
    }
}

/// A uid the account no longer holds is the folder's domain, not the
/// service's.
///
/// The same reasoning as [`StoreError::NoFolder`]'s, one level down. Nothing
/// is wrong with the connection. A service error would wrongly report a
/// working account as broken, because one message went away between a
/// listing and a click.
///
/// `CAMEL_FOLDER_ERROR_INVALID_UID` is the code Camel's own providers use for
/// this. Evolution reads it as "that message is gone", not as a reason to
/// take the account offline.
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
    // The uid is in the message. A mail folder holds thousands of messages,
    // and "invalid uid" without a uid is a report nobody can act on.
    assert!(error.to_string().contains("E17"), "{error}");
}

/// The distinction must survive the crate boundary. `jmap-mail-sync` answers a
/// missing message with a variant of its own, precisely so that this mapping
/// can exist. A `From` that flattened it into a client error would put the
/// account back in the service domain, without changing a line of the test
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
/// Flattened into a client error, it would reach Evolution as a server that
/// said no. That is the one thing it is not: the server was never asked.
///
/// The address survives the boundary, because it is the whole fact the user
/// has to act on.
#[test]
fn a_sync_layers_missing_identity_stays_a_missing_identity() {
    let error = StoreError::from(SyncError::NoIdentity("alice@example.com".to_owned()));

    match &error {
        StoreError::NoIdentity(address) => assert_eq!(address, "alice@example.com"),
        other => panic!("expected a missing identity, got {other}"),
    }
    assert!(error.to_string().contains("alice@example.com"), "{error}");
}

/// The one failure that is not Camel's to classify: the user pressed Stop.
/// Every caller in Camel tests for this with `g_error_matches (error,
/// G_IO_ERROR, G_IO_ERROR_CANCELLED)` before deciding anything went wrong.
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

/// `NoQuota` and `NoLocalFile` report "nothing to report" in GLib's own
/// domain. `camel-imapx-folder.c` uses this same code for quotas turned off.
/// A wildcard match would wrongly fall back to the service domain instead.
#[test]
fn no_quota_and_no_local_file_are_reported_in_glibs_own_domain() {
    for error in [StoreError::NoQuota, StoreError::NoLocalFile] {
        let gerror = error.to_gerror();
        // SAFETY: an owned GError, freed below.
        unsafe {
            assert_eq!((*gerror).domain, gio_sys::g_io_error_quark(), "{error}");
            assert_eq!((*gerror).code, gio_sys::G_IO_ERROR_NOT_SUPPORTED, "{error}");
            glib_sys::g_error_free(gerror);
        }
    }
}

/// A folder or role the account does not have is the store's domain, not the
/// service's. Nothing is wrong with the connection. Only one folder is
/// missing.
#[test]
fn a_missing_folder_or_role_is_reported_in_camels_store_domain() {
    for error in [
        StoreError::NoFolder("Archive/2020".to_owned()),
        StoreError::NoRole(FolderRole::Trash),
    ] {
        let gerror = error.to_gerror();
        // SAFETY: an owned GError, freed below.
        unsafe {
            assert_eq!((*gerror).domain, camel_store_error_quark(), "{error}");
            assert_eq!(
                (*gerror).code,
                CAMEL_STORE_ERROR_NO_FOLDER as i32,
                "{error}"
            );
            glib_sys::g_error_free(gerror);
        }
    }
}

/// A message over `maxSizeUpload` is the message's own fault, not the
/// account's. It is reported like a message Camel itself could not write
/// out, never as a service error that would mark a working account broken.
#[test]
fn a_message_over_the_size_limit_is_reported_in_camels_folder_domain() {
    let error = StoreError::Client(Error::TooLarge {
        size: 20_000_000,
        limit: 10_000_000,
    });
    let gerror = error.to_gerror();
    // SAFETY: an owned GError, freed below.
    unsafe {
        assert_eq!((*gerror).domain, camel_folder_error_quark());
        assert_eq!((*gerror).code, CAMEL_FOLDER_ERROR_INVALID as i32);
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

/// `connect_sync` runs again after Camel decides the connection is gone. The
/// old connection is exactly what gets replaced. Refusing would leave the
/// store pointing at a socket nobody listens on.
#[test]
fn connecting_twice_replaces_the_connection_rather_than_refusing() {
    let server = MockServer::builder().start();
    let store = JmapStore::detached();

    store.store_connection(sync_against(&server));
    store.store_connection(sync_against(&server));
    assert!(store.is_connected());
}

/// Camel asks a store to disconnect on shutdown, whether or not it ever
/// connected. Dropping nothing is a normal outcome, not a failure. The two
/// cases are told apart, because `disconnect_sync` needs to know whether it
/// did anything.
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

/// The store's side of reading a message: the folder holds the uid, and the
/// store holds the connection. This is where the two meet, the same split a
/// refresh already goes through.
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

/// A message asked for over a store with no connection is the disconnected
/// case, not a missing message. Camel drives a store it believes is
/// connected. Reporting the uid as gone would make Evolution drop a row that
/// is fine.
#[test]
fn a_disconnected_store_has_no_message_to_hand_over() {
    let store = JmapStore::detached();

    match store.message_source(&Id::new("E1")) {
        Err(StoreError::Disconnected) => {}
        Err(other) => panic!("expected the store to report being disconnected, got {other}"),
        Ok(_) => panic!("a store with no connection fetched a message"),
    }
}
