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
//!
//! The fourth vfunc has nothing to do with connecting at all: `get_name` is how
//! every message Camel writes about this account refers to it, and it is
//! testable both ways round — as [`describe`], which is the whole of the
//! decision, and through `camel_service_get_name` on a real store, which is the
//! only thing that proves the slot was filled in.

mod common;

use std::ffi::CStr;
use std::ptr;
use std::sync::Arc;

use common::Account;
use eds_sys::{
    CAMEL_AUTHENTICATION_ACCEPTED, CAMEL_AUTHENTICATION_ERROR, CAMEL_AUTHENTICATION_REJECTED,
    CAMEL_SERVICE_ERROR_INVALID, CAMEL_SERVICE_ERROR_UNAVAILABLE, CAMEL_STORE_FOLDER_INFO_REFRESH,
    CamelNetworkSettings, CamelService, camel_network_settings_set_host,
    camel_network_settings_set_port, camel_network_settings_set_user, camel_service_error_quark,
    camel_service_get_name, camel_service_ref_settings,
};
use glib_sys::{GError, GFALSE, GTRUE, g_error_free, g_error_new_literal};
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_client::{Client, Credentials, Error};
use jmap_mail::connect::{StoreError, password_credentials};
use jmap_mail::server::ServerConfig;
use jmap_mail::service::{
    FirstAttempt, authenticate, describe, report_authentication, resolve_oauth2_attempt,
};
use jmap_mail::store::JmapStore;
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;
use jmap_proto::mail::role;

/// No flags: the folder listing already in hand, without going to the server.
const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

fn config(server: &MockServer) -> ServerConfig {
    ServerConfig {
        target: ConnectTarget::Origin(server.origin().to_owned()),
        user: None,
    }
}

fn open(
    store: &JmapStore,
    config: &ServerConfig,
    password: Option<&str>,
) -> Result<(), StoreError> {
    authenticate(
        store,
        config,
        password_credentials(config.user.as_deref(), password),
    )
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
        target: ConnectTarget::Origin("http://127.0.0.1:1".to_owned()),
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
        target: ConnectTarget::Origin(String::new()),
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
        target: ConnectTarget::Origin("http://127.0.0.1:1".to_owned()),
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

// ---------------------------------------------------------------------------
// the OAuth 2.0 first attempt's `GError` handoff (`connect_sync`, item 21
// area (4))
//
// `resolve_oauth2_attempt` is what `connect_sync` calls after
// `camel_service_authenticate_sync`'s OAuth 2.0 first attempt answers, and it
// is the one place that attempt's private `attempt_error` changes ownership.
// A wrong answer here is either a leak (an error nobody ever frees) or a
// double free (an error freed here *and* later by whoever the vfunc's own
// `error` out-parameter belongs to) — neither is something the existing
// `Reported` tests above exercise, since those drive `report_authentication`
// on the *session's* verdict, never this earlier, OAuth-only attempt.

/// A `GError` this test owns, standing in for what
/// `camel_service_authenticate_sync` would have written into `attempt_error`.
fn synthetic_error() -> *mut GError {
    let message = std::ffi::CString::new("synthetic").unwrap();
    // SAFETY: a literal, NUL-terminated message; the domain/code are
    // arbitrary, since nothing under test reads them.
    unsafe {
        g_error_new_literal(
            camel_service_error_quark(),
            CAMEL_SERVICE_ERROR_INVALID as i32,
            message.as_ptr(),
        )
    }
}

/// `ACCEPTED` is the one verdict Camel's own contract guarantees carries no
/// error, so there is nothing to move — the outcome is a final answer and the
/// caller's `error` out-parameter is left exactly as it was.
#[test]
fn an_accepted_attempt_is_done_and_touches_no_error() {
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: `attempt_error` is NULL, matching `ACCEPTED`'s own contract;
    // `error` is a writable, currently-NULL `GError **`.
    let outcome = unsafe {
        resolve_oauth2_attempt(CAMEL_AUTHENTICATION_ACCEPTED, ptr::null_mut(), &mut error)
    };

    assert_eq!(outcome, FirstAttempt::Done(GTRUE));
    assert!(
        error.is_null(),
        "an accepted attempt touched the caller's error"
    );
}

/// The path a real bug here would show up as a message the user never sees:
/// an `ERROR` verdict's `GError` must reach the caller's own out-parameter,
/// not merely disappear (which is what "no leak" alone would also permit,
/// via a stray free).
#[test]
fn an_error_first_attempt_moves_its_gerror_to_the_callers_out_parameter() {
    let attempt_error = synthetic_error();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: `attempt_error` is an owned `GError` this call may move;
    // `error` is a writable, currently-NULL `GError **`.
    let outcome =
        unsafe { resolve_oauth2_attempt(CAMEL_AUTHENTICATION_ERROR, attempt_error, &mut error) };

    assert_eq!(outcome, FirstAttempt::Done(GFALSE));
    assert_eq!(
        error, attempt_error,
        "the caller's out-parameter did not receive the attempt's own GError"
    );
    // The caller is now the owner, exactly as `report_authentication`'s own
    // callers are — freeing it here is this test discharging that, not the
    // function under test.
    // SAFETY: ownership was handed to `error` above, and nothing else holds
    // a reference to it.
    unsafe { g_error_free(error) };
}

/// A caller that passed no `error` (Camel's contract allows it — the vfunc's
/// own `error` may itself be NULL) does not want to *know* about a failure,
/// but the `GError` still must not leak. Freeing it here, rather than moving
/// it nowhere, is the whole of what this test pins: a regression that instead
/// frees it a second time later (double free) or leaves both this call and a
/// later one thinking they own it aborts the process outright, which is a
/// stronger net than any assertion this test could write.
#[test]
fn an_error_first_attempt_with_no_out_parameter_frees_its_gerror_rather_than_leaking_it() {
    let attempt_error = synthetic_error();

    // SAFETY: `attempt_error` is an owned `GError`; a NULL `error` is exactly
    // the "caller wants no error" case this function's contract allows.
    let outcome = unsafe {
        resolve_oauth2_attempt(CAMEL_AUTHENTICATION_ERROR, attempt_error, ptr::null_mut())
    };

    assert_eq!(outcome, FirstAttempt::Done(GFALSE));
}

/// `REJECTED` means the server refused the token, not that the connection
/// failed — the session's own interactive loop below is where that recovers,
/// so the private attempt's `GError` is not this verdict's to report and must
/// be freed, and the caller's own `error` slot must stay exactly as it was
/// (still NULL) for that loop to write into.
#[test]
fn a_rejected_attempt_frees_its_gerror_and_leaves_the_callers_error_untouched() {
    let attempt_error = synthetic_error();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as the `ERROR` tests above.
    let outcome =
        unsafe { resolve_oauth2_attempt(CAMEL_AUTHENTICATION_REJECTED, attempt_error, &mut error) };

    assert_eq!(outcome, FirstAttempt::Refused);
    assert!(
        error.is_null(),
        "a rejected attempt wrote into the caller's error, stealing the session loop's slot"
    );
}

/// The same free-not-leak obligation as the `ERROR` case, for the verdict
/// whose own `GError` is never supposed to reach anyone.
#[test]
fn a_rejected_attempt_with_no_out_parameter_still_frees_its_gerror() {
    let attempt_error = synthetic_error();

    // SAFETY: as above, with the "caller wants no error" NULL out-parameter.
    let outcome = unsafe {
        resolve_oauth2_attempt(
            CAMEL_AUTHENTICATION_REJECTED,
            attempt_error,
            ptr::null_mut(),
        )
    };

    assert_eq!(outcome, FirstAttempt::Refused);
}

// ---------------------------------------------------------------------------
// what Camel calls this account

/// Writes a server onto the account's settings the way EDS configures one: by
/// property, on the settings object the service made for itself.
fn configure(account: &Account, host: &str, port: u16, user: Option<&str>) {
    let host = std::ffi::CString::new(host).expect("a host with no NUL in it");
    let user = user.map(|user| std::ffi::CString::new(user).expect("a user with no NUL in it"));
    // SAFETY: a live `CamelService`, whose settings are a `CamelJmapSettings`
    // and therefore a `CamelNetworkSettings`; the strings are copied by the
    // setters and the reference is given back.
    unsafe {
        let settings = camel_service_ref_settings(account.store.cast::<CamelService>());
        assert!(!settings.is_null(), "the store has no settings");
        let network = settings.cast::<CamelNetworkSettings>();
        camel_network_settings_set_host(network, host.as_ptr());
        camel_network_settings_set_port(network, port);
        camel_network_settings_set_user(
            network,
            user.as_ref().map_or(ptr::null(), |user| user.as_ptr()),
        );
        gobject_sys::g_object_unref(settings.cast());
    }
}

/// What `camel_service_get_name` answers, as a Rust string. NULL — which is
/// what Camel returns for a class that filled none of the slot in — comes back
/// as `None` rather than as a panic, so the test that cares says so itself.
fn name_of(account: &Account, brief: bool) -> Option<String> {
    // SAFETY: a live `CamelService`; the string returned is a GLib allocation
    // this call owns and frees.
    unsafe {
        let name = camel_service_get_name(
            account.store.cast::<CamelService>(),
            if brief {
                glib_sys::GTRUE
            } else {
                glib_sys::GFALSE
            },
        );
        if name.is_null() {
            return None;
        }
        let owned = CStr::from_ptr(name).to_string_lossy().into_owned();
        glib_sys::g_free(name.cast());
        Some(owned)
    }
}

/// The brief form is the one Camel puts in a folder tree and in the middle of
/// its own sentences, so it is the server and nothing else.
#[test]
fn the_brief_name_of_an_account_is_the_server_it_reaches() {
    assert_eq!(
        describe(Some("jmap.example.com"), 0, Some("ada"), true),
        "JMAP server jmap.example.com"
    );
}

/// The long form is documented as "complete and mostly unambiguous", and the
/// thing that most often tells two accounts on one server apart is which user
/// they are.
#[test]
fn the_full_name_of_an_account_names_the_user_as_well_as_the_server() {
    assert_eq!(
        describe(Some("jmap.example.com"), 0, Some("ada"), false),
        "JMAP service for ada on jmap.example.com"
    );
}

/// An account with no user name is not an error — the credential may be a
/// bearer token — so it is named by its server alone rather than by an empty
/// slot in a sentence.
#[test]
fn an_account_with_no_user_is_named_by_its_server_alone() {
    assert_eq!(
        describe(Some("jmap.example.com"), 0, None, false),
        "JMAP service on jmap.example.com"
    );
}

/// JMAP is HTTP, so two accounts differing only in port are ordinary — a local
/// server and a test one. The port goes in the form whose job is to be
/// unambiguous, and stays out of the one whose job is to be short.
#[test]
fn a_port_the_account_names_belongs_to_the_unambiguous_form() {
    assert_eq!(
        describe(Some("127.0.0.1"), 8080, None, false),
        "JMAP service on 127.0.0.1:8080"
    );
    assert_eq!(
        describe(Some("127.0.0.1"), 8080, None, true),
        "JMAP server 127.0.0.1"
    );
}

/// A newly created account has no server yet and Camel still asks what to call
/// it. "JMAP server " with the host left off would be a sentence about a server
/// that is not there.
#[test]
fn an_account_with_no_server_is_named_without_one() {
    assert_eq!(describe(None, 0, Some("ada"), true), "JMAP account");
    assert_eq!(describe(None, 0, Some("ada"), false), "JMAP account");
}

/// The slot itself. `camel_service_get_name` is `g_return_val_if_fail
/// (class->get_name != NULL, NULL)`, so a class that overrides nothing answers
/// NULL and logs a critical every time Camel mentions the account — which is
/// what this asserts is no longer true.
#[test]
fn camel_asks_the_store_what_the_account_is_called_and_is_answered() {
    let account = Account::open();
    configure(&account, "jmap.example.com", 0, Some("ada"));

    assert_eq!(
        name_of(&account, true).as_deref(),
        Some("JMAP server jmap.example.com")
    );
    assert_eq!(
        name_of(&account, false).as_deref(),
        Some("JMAP service for ada on jmap.example.com")
    );
}

/// The name is read off the settings each time it is asked for, not frozen at
/// construction: an account whose server the user edits is one Camel goes on
/// naming after the edit.
#[test]
fn the_name_follows_the_server_the_account_is_reconfigured_to() {
    let account = Account::open();
    assert_eq!(name_of(&account, true).as_deref(), Some("JMAP account"));

    configure(&account, "jmap.example.com", 8080, None);
    assert_eq!(
        name_of(&account, false).as_deref(),
        Some("JMAP service on jmap.example.com:8080")
    );
}

/// The name is a human's spelling of the host rather than the wire's: nothing
/// connects with it, and an account configured in an internationalised domain
/// name should be described in the one its owner typed.
#[test]
fn an_internationalised_host_is_named_as_the_account_spells_it() {
    let account = Account::open();
    configure(&account, "jmap.bücher.example", 0, None);

    assert_eq!(
        name_of(&account, true).as_deref(),
        Some("JMAP server jmap.bücher.example")
    );
}
