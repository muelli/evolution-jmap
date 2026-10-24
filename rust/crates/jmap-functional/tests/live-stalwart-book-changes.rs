// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The test of `book-changes.rs` (two connects, one cache), against real
//! Stalwart instead of the in-process mock.
//!
//! `book-changes.rs` writes its server-side change directly into the
//! transaction log of `jmap_mock::state::Store`. A real server has no such
//! backdoor. Thus a second [`jmap_client::Client`], independent of the
//! [`Session`] under test, creates a contact over JMAP between the two
//! connects. `live_server_changes.rs` in `jmap-book-sync` uses the same
//! method to test `BookSync::get_changes`. This test checks one layer
//! higher: the second EDS connect, with the on-disk cache of the first
//! connect, shows a change that the real server made.
//!
//! This test uses the `list` phase of `functional-book-client` without
//! change. It also uses the loopback proxy, the password seed and the URL
//! rebase from `live-stalwart-book.rs`.
//!
//! `book-changes.rs` starts from an empty mock. A real account can keep
//! contacts from earlier runs. Thus the test asserts only that the unique new
//! contact of this run is not in the first list and is in the second list.
//!
//! ## How to run it
//!
//! Use the same environment as `live-stalwart-book.rs` (see
//! `docs/manual-test-live-server.md`):
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-book-changes -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{
    Keyfile, Session, SourceKind, observations, required_path, spawn_loopback_proxy,
};
use jmap_proto::contacts::ContactCard;
use jmap_proto::session::CAPABILITY_CONTACTS;
use jmap_test_support::unique_suffix;

/// `(origin, user, password)`, or `None` to skip. The same as
/// `live_server_params` in `live-stalwart-book.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// The keyfile of `live-stalwart-book.rs`, with a different `DisplayName=`.
fn keyfile(port: u16, user: &str) -> String {
    Keyfile::new(SourceKind::AddressBook, port)
        .display_name("JMAP functional live-Stalwart changes test")
        .user(user)
        .to_string()
}

/// Runs the `list` phase and returns its sorted `contact-<i>` names.
fn list_names(session: &Session, client: &std::path::Path) -> (Vec<String>, String) {
    let output = session.run(client, &["jmap-functional", "list"]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    let seen = observations(&stdout);
    let count: usize = seen
        .get("contacts")
        .unwrap_or_else(|| panic!("no 'contacts' observation\n{report}"))
        .parse()
        .unwrap_or_else(|_| panic!("'contacts' was not a number\n{report}"));
    let names = (0..count)
        .map(|index| {
            seen.get(format!("contact-{index}").as_str())
                .unwrap_or_else(|| panic!("no 'contact-{index}' observation\n{report}"))
                .to_string()
        })
        .collect();
    (names, report)
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_second_connect_pulls_a_real_servers_change_through_get_changes_sync() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server book-changes leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-book-changes"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_address_book_backend(&module);

    let (first_names, first_report) = list_names(&session, &client_binary);

    // A second client: a raw `jmap_client::Client` that connects directly to
    // the real server, not through the loopback proxy or the EDS backend.
    // `rebase_urls_to_origin` is necessary because the session document can
    // name a hostname that this runner cannot reach.
    let raw_client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the raw second client");
    let account_id = raw_client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let address_book_id = raw_client
        .address_books(&account_id)
        .expect("AddressBook/get failed for the raw second client")
        .into_iter()
        .find(|book| book.is_default == Some(true))
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the default address book");

    let new_name = format!(
        "agent-fnbookchanges-{:x}",
        unique_suffix() & 0xffff_ffff_ffff
    );
    assert!(
        !first_names.contains(&new_name),
        "the unique new name collided with one already on the server\n{first_report}"
    );
    let card = ContactCard::simple(
        address_book_id,
        &new_name,
        "agent-fnbookchanges@example.com",
    );
    raw_client
        .contact_create(&account_id, &card)
        .expect("ContactCard/set create failed for the raw second client");

    // A new process and a new private bus, but the same `XDG_CACHE_HOME`.
    // Thus the refresh after the second connect starts from the sync tag
    // that EDS stored on the first connect.
    let (second_names, second_report) = list_names(&session, &client_binary);
    assert!(
        second_names.contains(&new_name),
        "the second connect should see the contact the raw second client \
         created on the real server between the two connects\n{second_report}"
    );
}
