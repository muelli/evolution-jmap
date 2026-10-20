// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The collection test, against a real Stalwart instead of the in-process
//! mock.
//!
//! This test makes no write. `functional-collection-client`
//! (`tests/functional/collection-client.c`, the mock client with one addition,
//! see below) waits until the populate of the account writes an address-book
//! child and a calendar child, as in `collection.rs`. A real server has no
//! mock state to seed. Instead, a new `stw seed` Stalwart account has exactly
//! one default address book and one default calendar. In the mock,
//! `collection.rs` makes the same with `seed_address_book`/`seed_calendar`.
//!
//! The method is the same as in the book and calendar tests:
//! `spawn_loopback_proxy` satisfies the loopback-only plaintext rule of
//! `connect_target` in `jmap-backend-core`, and sends the traffic to
//! Stalwart. `JMAP_LIVE_SERVER_REBASE_URLS` rebases the `apiUrl` of the
//! server onto the address of the connection.
//!
//! The addition in `collection-client.c`: in the book and calendar tests, the
//! client stores the password on the leaf source before it opens it. But the
//! collection backend populates when the registry loads the account, before
//! the client connects to the bus. Thus the first attempt has no password,
//! goes to `credentials-required` and waits, and no GUI can answer. The
//! client stores the password on the account source (not on a child, because
//! the `authenticate_sync` of the account reads it). Then it calls
//! `e_source_invoke_authenticate_sync`, so that EDS tries again.
//!
//! ## How to run it
//!
//! Use the same environment as the `live_server_*.rs` tests of the
//! `jmap-*-sync` crates (see `docs/manual-test-live-server.md`): a throwaway
//! `stw seed` account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-collection -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;

use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};

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

/// The keyfile of `collection.rs`, with a real `User=`. This field causes
/// `jmap-backend-core::connect::credentials` to use Basic, not anonymous.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart collection test\n\
         Enabled=true\n\
         \n\
         [Collection]\n\
         BackendName=jmap\n\
         ContactsEnabled=true\n\
         CalendarEnabled=true\n\
         MailEnabled=false\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         User={user}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn evolution_source_registry_fans_the_real_account_out_into_a_book_and_a_calendar() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server collection leg"
        );
        return;
    };
    let client = required_path("JMAP_FUNCTIONAL_COLLECTION_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    const ACCOUNT_UID: &str = "jmap-functional-live-collection";
    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-collection"
    ));
    session.write_source(ACCOUNT_UID, &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    // The session document gives the configured `apiUrl`, not the proxy
    // address. See the comment on this line in `live-stalwart-book.rs`.
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_collection_backend(&module);

    let output = session.run(&client, &[ACCOUNT_UID]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );
    assert_eq!(
        seen.get("account-found"),
        Some(&"1"),
        "the registry never saw the account keyfile at all\n{report}"
    );
    assert_eq!(
        seen.get("children-found"),
        Some(&"2"),
        "the populate/fan-out did not produce both children in time\n{report}"
    );
    assert_eq!(
        seen.get("address-books-found"),
        Some(&"1"),
        "the account did not get an address book child\n{report}"
    );
    assert_eq!(
        seen.get("calendars-found"),
        Some(&"1"),
        "the account did not get a calendar child\n{report}"
    );

    for prefix in ["address-book", "calendar"] {
        assert_eq!(
            seen.get(format!("{prefix}-backend-name").as_str()),
            Some(&"jmap"),
            "the {prefix} child does not name this backend\n{report}"
        );
        assert_eq!(
            seen.get(format!("{prefix}-parent").as_str()),
            Some(&ACCOUNT_UID),
            "the {prefix} child does not belong to the account\n{report}"
        );
        assert_eq!(
            seen.get(format!("{prefix}-enabled").as_str()),
            Some(&"1"),
            "the {prefix} child was created disabled\n{report}"
        );
    }
}
