// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests what a setup refuses to commit. The last test shows why the refusal is
// necessary: the reader of the collection backend accepts each account that
// `check` accepts.
//
// The two are separate functions over the same fields, and such pairs can
// drift apart. `check` runs in Evolution before the setup writes data.
// `server_of` runs in the registry after the setup writes all data. If the
// first accepts what the second rejects, the setup commits an account where
// each operation fails. The user then sees an error dialog from a process that
// they did not start, not a message at the entry where they typed the mistake.

use jmap_backend_collection::collection_source::server_of;
use jmap_backend_core::source::{ConnectTarget, OwnedSource, SourceError};
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;
use jmap_config::account::{Account, apply};
use jmap_config::complete::{Incomplete, check};

/// The start account of each case: complete, and the same as in the manual
/// test recipe.
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
struct TestSource(OwnedSource);

impl TestSource {
    fn written(account: &Account) -> Self {
        let source = OwnedSource::new("jmap-account");
        // SAFETY: a live source.
        unsafe { apply(source.as_ptr(), account) };
        Self(source)
    }

    /// The server as the collection backend reads it back.
    fn server(&self) -> Result<ConnectTarget, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0.as_ptr()) }.map(|server| server.target)
    }
}

#[test]
fn a_complete_account_is_complete() {
    assert_eq!(check(&account()), Ok(()));
}

#[test]
fn the_server_entry_left_blank_is_the_missing_one() {
    // Not `InvalidHost("")`: an empty entry is a question that the user has
    // not answered yet. A keyfile with `Host=` reads back as no host.
    let mut account = account();
    account.connection.host = String::new();
    assert_eq!(
        check(&account),
        Err(Incomplete::Server(SourceError::MissingHost))
    );
}

#[test]
fn a_server_typed_as_a_url_is_not_a_host() {
    // Users frequently type this mistake in the server entry, because all
    // other places name a JMAP server with a URL.
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
    // Deliberately not a rule: `credentials()` makes an anonymous connection
    // when there is no user. The mock server and a local development instance
    // use this connection. If a setup required a user name, it would refuse
    // the account that this project uses for development.
    let mut account = account();
    account.connection.user = None;
    assert_eq!(check(&account), Ok(()));
}

#[test]
fn an_account_that_offers_nothing_yet_is_complete() {
    // Also deliberately not a rule. The three parts are switches over sources
    // that the setup writes in all cases (`mail::apply` gives the details).
    // Thus the user can enable a part later. The commit is not incorrect.
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
    // The join, and the reason that this file links the collection backend.
    // Each server that `check` evaluates is committed and read back as the
    // registry reads it.
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
