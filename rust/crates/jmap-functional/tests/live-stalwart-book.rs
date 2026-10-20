// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The single-write test of `address-book.rs`, against a real Stalwart
//! instead of the in-process mock.
//!
//! This test uses the `write` phase of `functional-book-client` without
//! change. Both tests use the same program. Thus a different result from EDS
//! comes from the server, not from a different client program.
//!
//! `connect_target` in `jmap-backend-core` refuses plaintext to a host that
//! is not loopback. Thus `spawn_loopback_proxy` gives a loopback address for
//! the real Stalwart (see its doc comment).
//! `JMAP_FUNCTIONAL_STORE_PASSWORD` (`book-client.c`) supplies the password,
//! because a real server needs one and no GUI can ask for it.
//!
//! ## How to run it
//!
//! Use the same environment as the `live_server_*.rs` tests of the
//! `jmap-*-sync` crates (see `docs/manual-test-live-server.md`): a throwaway
//! `stw seed` account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-book -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;

use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};

const EMAIL: &str = "dana@example.com";
const NICKNAME: &str = "Vee, the tall one";
const ORG: &str = "Acme Ltd";
const ORG_UNIT: &str = "Research";
const TITLE: &str = "Research Scientist";
const ROLE: &str = "Project Lead";
const STREET: &str = "Hauptstrasse 1";
const LOCALITY: &str = "Berlin";
const POSTCODE: &str = "10115";
const COUNTRY: &str = "Germany";
const ADDRESS_LABEL: &str = "Hauptstrasse 1\n10115 Berlin\nGermany";
const NOTE: &str = "met at FOSDEM; owes me a beer, apparently";
const HOMEPAGE: &str = "https://dana.example/profile?tags=x-files,ufo";
const CALENDAR_URI: &str = "https://dana.example/cal/dana.ics";
const FREEBUSY_URI: &str = "https://dana.example/fb/dana.ifb";
const SPOUSE: &str = "Fox Mulder";
const CATEGORY_ONE: &str = "Friends";
const CATEGORY_TWO: &str = "beer, in Berlin";
const CATEGORY_SEPARATOR: &str = "|";
const BIRTHDAY: &str = "1964-03-27";
/// The same 1x1 PNG as in `address-book.rs`.
const PHOTO_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

/// A value that is unique to this process. Thus a new run on the same
/// throwaway account does not read a contact from an earlier run.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)`, or `None` to skip. This is the same as
/// `connect_for_write` in the live-server tests of the other crates.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// The keyfile of `address-book.rs`, with a real `User=`. This field causes
/// `jmap-backend-core::connect::credentials` to use Basic, not anonymous.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart test\n\
         Enabled=true\n\
         \n\
         [Address Book]\n\
         BackendName=jmap\n\
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
fn evolution_opens_the_book_and_a_write_reaches_the_real_server() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server address-book leg"
        );
        return;
    };
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/live-stalwart-book"));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    // The session document of Stalwart gives its configured `apiUrl`
    // (`mail.example.internal`, which this host cannot reach), not the
    // address of the loopback proxy. `jmap-backend-core::rebase` ORs this
    // variable into its `[JMAP Rebase]` keyfile extension, thus the connect
    // path of the real backend also rewrites the URLs.
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_address_book_backend(&module);

    let full_name = format!("agent-fnbook-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(
        &client,
        &["jmap-functional", "write", &full_name, PHOTO_BASE64],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check this before the exit status, for the same reason as in
    // `address-book.rs`.
    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the book, with {}\n{report}",
            output.status
        )
    });
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected against the real server\n{report}"
    );
    assert_eq!(
        readonly, "0",
        "EDS opened the real-server book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    let added = seen
        .get("added")
        .unwrap_or_else(|| panic!("the client reported no added contact\n{report}"));
    assert!(
        !added.is_empty(),
        "EDS added a contact with no UID\n{report}"
    );

    assert_eq!(
        seen.get("read-back-full-name"),
        Some(&full_name.as_str()),
        "the contact EDS handed back is not the one that went in\n{report}"
    );
    assert_eq!(
        seen.get("read-back-email"),
        Some(&EMAIL),
        "the contact EDS handed back lost its email address\n{report}"
    );
    assert_eq!(
        seen.get("read-back-org"),
        Some(&ORG),
        "the contact EDS handed back lost its employer\n{report}"
    );
    assert_eq!(
        seen.get("read-back-org-unit"),
        Some(&ORG_UNIT),
        "the contact EDS handed back lost its department\n{report}"
    );
    assert_eq!(
        seen.get("read-back-nickname"),
        Some(&NICKNAME),
        "the contact EDS handed back lost or split its nickname\n{report}"
    );
    assert_eq!(
        seen.get("read-back-title"),
        Some(&TITLE),
        "the contact EDS handed back lost its job title\n{report}"
    );
    assert_eq!(
        seen.get("read-back-role"),
        Some(&ROLE),
        "the contact EDS handed back lost its role\n{report}"
    );
    assert_eq!(
        seen.get("read-back-note"),
        Some(&NOTE),
        "the contact EDS handed back lost or mangled its note\n{report}"
    );
    assert_eq!(
        seen.get("read-back-homepage"),
        Some(&HOMEPAGE),
        "the contact EDS handed back lost or mangled its home page\n{report}"
    );
    assert_eq!(
        seen.get("read-back-calendar-uri"),
        Some(&CALENDAR_URI),
        "the contact EDS handed back lost or moved its calendar address\n{report}"
    );
    assert_eq!(
        seen.get("read-back-freebusy-uri"),
        Some(&FREEBUSY_URI),
        "the contact EDS handed back lost or moved its free/busy address\n{report}"
    );
    assert_eq!(
        seen.get("read-back-spouse"),
        Some(&SPOUSE),
        "the contact EDS handed back lost or respelled its spouse\n{report}"
    );
    assert_eq!(
        seen.get("read-back-birthday"),
        Some(&BIRTHDAY),
        "the contact EDS handed back lost or moved its birthday\n{report}"
    );
    // Not the `IM_HANDLE` of `address-book.rs`. A vCard `X-JABBER` line
    // gives only a `user`, never a `uri` (module doc of `patch.rs` in
    // `jmap-book-sync`). Real Stalwart discards an `onlineServices` entry
    // that has a `user` but no `uri` (RFC-SUSPECT, RFC 9553 section 2.3.2).
    // The test asserts the dropped value. Thus a fix on the server or in
    // this crate causes the test to fail, and somebody must examine it again.
    assert_eq!(
        seen.get("read-back-jabber"),
        Some(&"(null)"),
        "the Jabber handle survived where it was expected to be dropped -- \
         has real Stalwart's RFC-SUSPECT `user`-without-`uri` behaviour \
         changed?\n{report}"
    );
    let categories = [CATEGORY_ONE, CATEGORY_TWO].join(CATEGORY_SEPARATOR);
    assert_eq!(
        seen.get("read-back-categories"),
        Some(&categories.as_str()),
        "the contact EDS handed back lost or split its categories\n{report}"
    );
    for (field, expected) in [
        ("read-back-street", STREET),
        ("read-back-locality", LOCALITY),
        ("read-back-code", POSTCODE),
        ("read-back-country", COUNTRY),
    ] {
        assert_eq!(
            seen.get(field),
            Some(&expected),
            "the contact EDS handed back lost or misplaced its {field}\n{report}"
        );
    }
    assert_eq!(
        seen.get("read-back-photo-type"),
        Some(&"uri"),
        "EDS did not cache the picture the way a meta backend caches one\n{report}"
    );
    let cached_photo = seen
        .get("read-back-photo-uri")
        .unwrap_or_else(|| panic!("the client reported no picture\n{report}"));
    assert!(
        cached_photo.starts_with("file://"),
        "EDS pointed the picture somewhere other than at its own cache: \
         {cached_photo}\n{report}"
    );
    assert!(
        cached_photo.ends_with(".png"),
        "EDS did not file the picture as the kind of image it is: \
         {cached_photo}\n{report}"
    );
    assert_eq!(
        seen.get("read-back-photo-file-base64"),
        Some(&PHOTO_BASE64),
        "the picture EDS cached is not the one that went in\n{report}"
    );
    let escaped_label = ADDRESS_LABEL.replace('\n', "\\n");
    assert_eq!(
        seen.get("read-back-address-label"),
        Some(&escaped_label.as_str()),
        "the contact EDS handed back lost or mangled its address label\n{report}"
    );
    // Not `== "1"`. `address-book.rs` starts a new mock for each run, but a
    // real account can keep contacts from an earlier run of this test. Thus
    // the test asserts that the contact is present, not the exact count.
    let contacts_after = seen
        .get("contacts-after")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("the client reported no parseable contacts-after\n{report}"));
    assert!(
        contacts_after >= 1,
        "the added contact is not in the book it was added to\n{report}"
    );
}
