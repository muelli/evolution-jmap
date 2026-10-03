// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Item 80 stage 2: `address-book.rs`'s own single-write leg, driven against
//! a real Stalwart instead of the in-process mock.
//!
//! Reuses `functional-book-client`'s `write` phase unchanged — the same
//! program, built once and staged for both legs — so that a difference in
//! what EDS hands the client back is a difference in what the *server* did
//! with the write, not a difference in how two client programs asked for it.
//!
//! `jmap-backend-core::connect_target` refuses plaintext to anything but a
//! loopback host, which a `.source` keyfile's `Host=`/`Port=` pair cannot
//! name a real, non-loopback Stalwart around; `spawn_loopback_proxy` is the
//! workaround (see its own doc comment), and `JMAP_FUNCTIONAL_STORE_PASSWORD`
//! (`book-client.c`) is the other missing piece — a real server needs a
//! password this crate's other legs never do, and nothing here has a GUI to
//! be prompted through.
//!
//! ## Running it
//!
//! Same environment as the `jmap-*-sync` crates' own `live_server_*.rs`
//! tests -- see `docs/manual-test-live-server.md`: a throwaway `stw seed`
//! account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-book -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

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
/// The same 1x1 PNG `address-book.rs` uses.
const PHOTO_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never mistakes a previous run's contact for this
/// one's. Mirrors every `jmap-*-sync` live-server test's own copy.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)`, or `None` to skip -- mirrors every
/// `jmap-*-sync` live-server test's own `connect_for_write`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// `address-book.rs`'s own keyfile, with a real `User=` added: the one field
/// that turns `jmap-backend-core::connect::credentials`'s anonymous branch
/// into the Basic one.
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
    // Stalwart's session document states its own configured `apiUrl`
    // (`mail.example.internal`, unreachable from here), not the loopback
    // proxy address it was actually asked through -- the same divergence
    // every other live-server test in this workspace already works around.
    // `jmap-backend-core::rebase` ORs this env var into its own
    // `[JMAP Rebase]` keyfile extension, so it reaches the real backend's
    // connect path too, not just `jmap_client`'s own harness.
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

    // Checked before the exit status, same reason `address-book.rs` does.
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
    // Not `Some(&IM_HANDLE)`, unlike every other field here: a vCard
    // `X-JABBER` line can only ever state a bare `user`, never a `uri`
    // (`jmap-book-sync`'s own `patch.rs` module doc), and real Stalwart
    // silently discards an `onlineServices` entry that has `user` but no
    // `uri` -- confirmed independently of this test, with raw curl against a
    // freshly seeded account, in the harness log's 2026-10-02
    // `diff_online_services` entry (RFC-SUSPECT, RFC 9553 section 2.3.2).
    // Asserted as the known-dropped shape rather than left unchecked, so a
    // future fix on either side (Stalwart stops dropping it, or this crate
    // starts synthesizing a `uri`) turns this red instead of silently
    // passing on behaviour nobody re-examined.
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
    // Not `== "1"`: unlike the mock, which `address-book.rs` starts fresh
    // every run, a real account can carry contacts an earlier run of this
    // same test left behind. Presence, not the exact count, is the claim.
    let contacts_after = seen
        .get("contacts-after")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("the client reported no parseable contacts-after\n{report}"));
    assert!(
        contacts_after >= 1,
        "the added contact is not in the book it was added to\n{report}"
    );
}
