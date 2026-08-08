// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the `CamelService` vfuncs do once the GObject is peeled off.
//!
//! `connect_sync`, `authenticate_sync` and `disconnect_sync` are three vfuncs
//! and one operation: Camel's rule is that the service opens nothing itself but
//! asks its `CamelSession` to authenticate it, and the session calls
//! `authenticate_sync` back — once, or once per password the user types. So
//! everything that actually happens happens in `authenticate_sync`, and what is
//! testable without a `CamelSession` is exactly that: [`authenticate`], which
//! opens the account and installs it on the store, and
//! [`report_authentication`], which turns the outcome into the two answers the
//! vfunc returns — a verdict, and an error that is deliberately absent for one
//! of the three verdicts.

use std::ptr;
use std::sync::Arc;

use eds_sys::{
    CAMEL_AUTHENTICATION_ACCEPTED, CAMEL_AUTHENTICATION_ERROR, CAMEL_AUTHENTICATION_REJECTED,
    CAMEL_SERVICE_ERROR_UNAVAILABLE, CAMEL_STORE_FOLDER_INFO_REFRESH, camel_service_error_quark,
};
use glib_sys::GError;
use jmap_backend_core::source::SourceError;
use jmap_client::transport::CancelFlag;
use jmap_client::{Client, Credentials, Error};
use jmap_mail::connect::StoreError;
use jmap_mail::server::ServerConfig;
use jmap_mail::service::{authenticate, report_authentication};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;
use jmap_proto::mail::role;

/// No flags: the folder listing already in hand, without going to the server.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

fn config(server: &MockServer) -> ServerConfig {
    ServerConfig {
        origin: server.origin().to_owned(),
        user: None,
    }
}

fn open(
    store: &JmapStore,
    config: &ServerConfig,
    password: Option<&str>,
) -> Result<(), StoreError> {
    authenticate(store, config, password, CancelFlag::new())
}

/// The verdict and the error one attempt produces, with the `GError` owned by
/// the caller the way the vfunc's is owned by Camel.
struct Reported {
    result: eds_sys::CamelAuthenticationResult,
    error: *mut GError,
}

impl Reported {
    fn of(outcome: Result<(), StoreError>) -> Self {
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: `error` is a writable, currently-NULL `GError **`, which is
        // what the vfunc is handed.
        let result = unsafe { report_authentication(outcome, &mut error) };
        Self { result, error }
    }
}

impl Drop for Reported {
    fn drop(&mut self) {
        if !self.error.is_null() {
            // SAFETY: the reporting call handed over ownership of it.
            unsafe { glib_sys::g_error_free(self.error) };
        }
    }
}

fn seeded() -> MockServer {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let state = server.state();
    state
        .lock()
        .unwrap()
        .account_mut(&account_id)
        .unwrap()
        .seed_mailbox("Inbox", Some(role::INBOX));
    server
}

#[test]
fn authenticating_installs_the_connection_the_store_serves_from() {
    let server = seeded();
    let store = JmapStore::detached();

    open(&store, &config(&server), None).expect("authenticated");

    assert!(store.is_connected());
    let tree = store.folders(CACHED).expect("listed");
    assert_eq!(tree.iter().count(), 1);
}

#[test]
fn a_successful_attempt_is_accepted_and_sets_no_error() {
    let reported = Reported::of(Ok(()));

    assert_eq!(reported.result, CAMEL_AUTHENTICATION_ACCEPTED);
    assert!(reported.error.is_null(), "an accepted attempt set an error");
}

/// The rule that makes the prompt loop work. `camel_session_authenticate_sync`
/// reads `REJECTED` as "ask the user for another password and call back", and
/// only propagates a `GError` when the verdict is `ERROR`; setting one here
/// would be a failure reported for an attempt that has not failed yet.
#[test]
fn a_rejected_password_is_reported_without_a_gerror_so_camel_asks_again() {
    let server = MockServer::builder().basic_auth("vera", "hunter2").start();
    let store = JmapStore::detached();
    let mut config = config(&server);
    config.user = Some("vera".to_owned());

    let outcome = open(&store, &config, Some("wrong"));
    assert!(
        !store.is_connected(),
        "a refused password left a connection"
    );

    let reported = Reported::of(outcome);
    assert_eq!(reported.result, CAMEL_AUTHENTICATION_REJECTED);
    assert!(reported.error.is_null(), "a rejected attempt set an error");
}

/// The other half of it: an attempt that failed for a reason no password can
/// fix has to say why, or Camel reports a service that failed without a
/// message.
#[test]
fn an_unreachable_server_is_an_error_with_the_gerror_camel_routes_on() {
    let store = JmapStore::detached();
    let config = ServerConfig {
        // Port 1 is reserved and nothing listens there.
        origin: "http://127.0.0.1:1".to_owned(),
        user: None,
    };

    let reported = Reported::of(open(&store, &config, None));

    assert_eq!(reported.result, CAMEL_AUTHENTICATION_ERROR);
    assert!(!reported.error.is_null(), "an error verdict said nothing");
    // SAFETY: a live GError this struct owns.
    unsafe {
        assert_eq!((*reported.error).domain, camel_service_error_quark());
        assert_eq!(
            (*reported.error).code,
            CAMEL_SERVICE_ERROR_UNAVAILABLE as i32
        );
    }
}

/// Not Camel's domain and not ours: the user pressed Stop, and every caller
/// above tests for `G_IO_ERROR_CANCELLED` before deciding anything went wrong.
/// It is still an `ERROR` verdict, because there is no third thing to say.
#[test]
fn a_cancelled_attempt_is_an_error_reported_in_glibs_own_domain() {
    let reported = Reported::of(Err(StoreError::Client(Error::Cancelled)));

    assert_eq!(reported.result, CAMEL_AUTHENTICATION_ERROR);
    // SAFETY: a live GError this struct owns.
    unsafe {
        assert_eq!((*reported.error).domain, gio_sys::g_io_error_quark());
        assert_eq!((*reported.error).code, gio_sys::G_IO_ERROR_CANCELLED);
    }
}

/// An account whose settings do not describe a server never reaches one, so
/// there is nothing for a password to be wrong about — and re-prompting for a
/// misconfigured account is a loop no password ends.
#[test]
fn an_account_with_no_server_fails_without_asking_for_a_password() {
    let store = JmapStore::detached();
    let config = ServerConfig {
        origin: String::new(),
        user: None,
    };

    let outcome = open(&store, &config, None);
    assert!(
        matches!(outcome, Err(StoreError::Client(_))),
        "expected the empty origin to be refused"
    );
    assert_eq!(Reported::of(outcome).result, CAMEL_AUTHENTICATION_ERROR);

    // And the same for the one the settings reader produces itself.
    let reported = Reported::of(Err(StoreError::Config(SourceError::MissingHost)));
    assert_eq!(reported.result, CAMEL_AUTHENTICATION_ERROR);
    assert!(!reported.error.is_null());
}

/// Camel re-authenticates a service it already has a connection for — a
/// password change, a session that lost track. The attempt that fails must not
/// take the working connection with it: the store would be left unable to serve
/// a folder it was serving a moment ago, for a password nobody has typed yet.
#[test]
fn a_failed_attempt_leaves_a_working_connection_alone() {
    let server = seeded();
    let store = JmapStore::detached();
    open(&store, &config(&server), None).expect("authenticated");
    let tree = store.folders(CACHED).expect("listed");

    let unreachable = ServerConfig {
        origin: "http://127.0.0.1:1".to_owned(),
        user: None,
    };
    open(&store, &unreachable, None).expect_err("nothing listens there");

    assert!(store.is_connected());
    assert!(Arc::ptr_eq(&tree, &store.folders(CACHED).expect("listed")));
}

/// A successful one does replace it, listing and all: the account was
/// re-authenticated because something about it changed, and the tree the old
/// connection produced describes a server this one may not be talking to.
#[test]
fn a_second_authentication_replaces_the_connection_and_its_listing() {
    let server = seeded();
    let store = JmapStore::detached();
    open(&store, &config(&server), None).expect("authenticated");
    let tree = store.folders(CACHED).expect("listed");

    open(&store, &config(&server), None).expect("authenticated again");

    assert!(store.is_connected());
    let refreshed = store.folders(CACHED).expect("listed");
    assert!(
        !Arc::ptr_eq(&tree, &refreshed),
        "the reconnected store served the old connection's tree"
    );
    assert_eq!(refreshed.iter().count(), 1);
}

/// The store is what holds the connection, so a `disconnect_sync` is a
/// `drop_connection` and nothing else — including on a store that never
/// connected, which is what Camel asks of every service on shutdown.
#[test]
fn disconnecting_drops_whatever_the_authentication_installed() {
    let server = seeded();
    let store = JmapStore::detached();
    assert!(!store.drop_connection());

    open(&store, &config(&server), None).expect("authenticated");
    assert!(store.drop_connection());
    assert!(!store.is_connected());
    assert!(matches!(
        store.folders(CACHED | CAMEL_STORE_FOLDER_INFO_REFRESH),
        Err(StoreError::Disconnected)
    ));
}

/// The connection the store ends up with is the one that was just opened, not
/// a second one opened behind it: the account id it carries is the server's.
#[test]
fn the_installed_connection_is_the_one_the_attempt_opened() {
    let server = seeded();
    let store = JmapStore::detached();

    open(&store, &config(&server), Some("ignored")).expect("authenticated");

    // A connection made by hand against the same server, to show the store is
    // not simply reporting "connected" for an empty slot.
    let direct = MailSync::new(
        Client::connect(server.origin(), Credentials::none()).expect("connected"),
        server.account_id(),
    );
    assert_eq!(
        store.folders(CACHED).expect("listed").iter().count(),
        direct.folder_tree().expect("listed").1.iter().count()
    );
}
