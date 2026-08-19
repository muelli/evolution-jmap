// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// What a setup refuses to commit — and, in the last test, the reason the
// refusal is worth having: an account this accepts is an account the collection
// backend's own reader accepts.
//
// The two are separate functions over the same fields, and that is exactly the
// kind of pair that drifts. `check` runs in Evolution before anything is
// written, `server_of` runs in the registry after everything is; if the first
// accepts what the second rejects, the setup has committed an account whose
// every operation fails, and the user is told about it in an error dialog from
// a process they did not start rather than in the entry they typed the mistake
// into.

use std::ffi::CString;
use std::ptr;

use eds_sys::{ESource, e_source_new_with_uid};
use gobject_sys::g_object_unref;
use jmap_backend_collection::collection_source::server_of;
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;
use jmap_config::account::{Account, apply};
use jmap_config::complete::{Incomplete, check};

/// The account every case below starts from: complete, and the one the manual
/// test recipe describes.
fn account() -> Account {
    Account {
        identity: "vera@example.com".to_owned(),
        connection: Connection {
            host: "jmap.example.com".to_owned(),
            port: Some(8443),
            user: Some("vera".to_owned()),
            auth_method: None,
            secure: true,
        },
        parts: Parts {
            mail: true,
            contacts: true,
            calendars: true,
        },
        oauth2_registered: false,
    }
}

/// An `ESource` in the state a setup commits into, as in `tests/account.rs`.
struct TestSource(*mut ESource);

impl TestSource {
    fn written(account: &Account) -> Self {
        let uid = CString::new("jmap-account").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        // SAFETY: a live source.
        unsafe { apply(source, account) };
        Self(source)
    }

    /// The server as the collection backend reads it back.
    fn server(&self) -> Result<ConnectTarget, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0) }.map(|server| server.target)
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this owns the reference `e_source_new_with_uid` returned.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn a_complete_account_is_complete() {
    assert_eq!(check(&account()), Ok(()));
}

#[test]
fn the_server_entry_left_blank_is_the_missing_one() {
    // Not `InvalidHost("")`: an empty entry is a question the user has not
    // answered yet, and a keyfile with `Host=` in it reads back as no host at
    // all.
    let mut account = account();
    account.connection.host = String::new();
    assert_eq!(
        check(&account),
        Err(Incomplete::Server(SourceError::MissingHost))
    );
}

#[test]
fn a_server_typed_as_a_url_is_not_a_host() {
    // The mistake a server entry actually collects, because every other place
    // a JMAP server is named is a URL.
    for host in [
        "https://jmap.example.com",
        "jmap.example.com/jmap",
        "jmap.example.com:8443",
    ] {
        let mut account = account();
        account.connection.host = host.to_owned();
        assert_eq!(
            check(&account),
            Err(Incomplete::Server(SourceError::InvalidHost(
                host.to_owned()
            ))),
            "{host} was accepted as a host name"
        );
    }
}

#[test]
fn plaintext_to_a_server_that_is_not_this_machine_is_refused() {
    let mut account = account();
    account.connection.secure = false;
    assert_eq!(
        check(&account),
        Err(Incomplete::Server(SourceError::InsecureTransport(
            "jmap.example.com".to_owned()
        )))
    );
}

#[test]
fn plaintext_to_this_machine_is_the_mock_server() {
    let mut account = account();
    account.connection.host = "localhost".to_owned();
    account.connection.secure = false;
    assert_eq!(check(&account), Ok(()));
}

#[test]
fn an_account_with_no_identity_is_incomplete() {
    for identity in ["", "   "] {
        let mut account = account();
        account.identity = identity.to_owned();
        assert_eq!(
            check(&account),
            Err(Incomplete::MissingIdentity),
            "{identity:?} was accepted as an identity"
        );
    }
}

#[test]
fn an_identity_that_is_not_an_address_is_incomplete() {
    // `[Mail Identity] Address` is the `From:` of everything the account sends.
    for identity in [
        "vera",
        "vera@",
        "@example.com",
        "ve ra@example.com",
        "a@b@c",
    ] {
        let mut account = account();
        account.identity = identity.to_owned();
        assert_eq!(
            check(&account),
            Err(Incomplete::InvalidIdentity(identity.to_owned())),
            "{identity:?} was accepted as an address"
        );
    }
}

#[test]
fn an_account_nobody_logs_in_to_is_complete() {
    // Deliberately not a rule: `credentials()` turns an absent user into an
    // anonymous connection, which is what the mock server and a local
    // development instance are reached by. A setup that insisted on a user name
    // would refuse to commit the account this project is developed against.
    let mut account = account();
    account.connection.user = None;
    assert_eq!(check(&account), Ok(()));
}

#[test]
fn an_account_that_offers_nothing_yet_is_complete() {
    // Also deliberately not a rule. The three parts are switches over sources
    // that are written either way — `mail::apply` says so at length — so an
    // account with none of them on is one the user can turn something on in
    // later, not one that was committed wrong.
    let mut account = account();
    account.parts = Parts {
        mail: false,
        contacts: false,
        calendars: false,
    };
    assert_eq!(check(&account), Ok(()));
}

#[test]
fn the_check_and_the_registry_agree_about_the_server() {
    // The join, and the reason this file links the collection backend: every
    // server the check has an opinion about, committed and read back the way
    // the registry will read it.
    for (host, port, secure) in [
        ("jmap.example.com", Some(8443), true),
        ("jmap.example.com", None, true),
        ("", Some(8443), true),
        ("https://jmap.example.com", None, true),
        ("jmap.example.com", None, false),
        ("localhost", Some(8080), false),
        ("127.0.0.1", None, false),
        ("::1", None, false),
    ] {
        let mut account = account();
        account.connection.host = host.to_owned();
        account.connection.port = port;
        account.connection.secure = secure;

        let checked = check(&account);
        let read = TestSource::written(&account).server();
        match (checked, read) {
            (Ok(()), Ok(_)) => {}
            (Err(Incomplete::Server(refused)), Err(read)) => assert_eq!(
                refused, read,
                "{host:?} was refused for one reason and unreadable for another"
            ),
            (checked, read) => {
                panic!("{host:?}: the setup says {checked:?} and the registry says {read:?}")
            }
        }
    }
}
