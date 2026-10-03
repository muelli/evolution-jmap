// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Item 80 stage 2 batch 3: the collection leg, driven against a real
//! Stalwart instead of the in-process mock.
//!
//! Unlike the book and calendar legs, this one makes no write at all:
//! `functional-collection-client` (`tests/functional/collection-client.c`,
//! unchanged from the mock leg bar one addition -- see below) only waits for
//! the account's populate/fan-out to write an address-book and a calendar
//! child, the same question `collection.rs` asks of the mock. Against a real
//! server there is no mock state to seed directly; what this test relies on
//! instead is that a freshly `stw seed`-ed Stalwart account already carries
//! exactly one default address book and one default calendar (confirmed by
//! hand with a raw `AddressBook/get`/`Calendar/get` against a throwaway
//! account before writing this test), the same shape
//! `collection.rs`'s own `seed_address_book`/`seed_calendar` calls build in
//! the mock.
//!
//! The mechanism is unchanged from the other two legs:
//! `spawn_loopback_proxy` satisfies `jmap-backend-core::connect_target`'s
//! plaintext-stays-loopback rule honestly while the traffic actually
//! continues to Stalwart; `JMAP_LIVE_SERVER_REBASE_URLS` rebases the
//! server's stated `apiUrl` onto the address actually connected through.
//!
//! What `collection-client.c` adds beyond the other two legs' password seed:
//! unlike the book and calendar legs, where the write-phase client seeds the
//! password against the leaf source it is about to open *before* opening it,
//! the collection backend's populate runs the moment the registry loads the
//! account -- before this client has even connected to the bus -- so the
//! first attempt always finds no password, schedules
//! `credentials-required`, and goes quiet with no GUI here to answer it.
//! The client therefore stores the password against the *account* source
//! (not a child; it is the account's own `authenticate_sync` that resolves
//! it) and then calls `e_source_invoke_authenticate_sync`, the ordinary way
//! a client answers that signal, to make EDS retry now that a password
//! exists.
//!
//! ## Running it
//!
//! Same environment as the `jmap-*-sync` crates' own `live_server_*.rs`
//! tests -- see `docs/manual-test-live-server.md`: a throwaway `stw seed`
//! account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-collection -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};

/// `(origin, user, password)`, or `None` to skip -- mirrors
/// `live-stalwart-book.rs`'s own `live_server_params`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// `collection.rs`'s own keyfile, with a real `User=` added -- the one field
/// that turns `jmap-backend-core::connect::credentials`'s anonymous branch
/// into the Basic one.
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
    // Stalwart's session document states its own configured `apiUrl`, not the
    // loopback proxy address it was actually asked through -- see
    // `live-stalwart-book.rs`'s own comment on this line for the mechanism.
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
