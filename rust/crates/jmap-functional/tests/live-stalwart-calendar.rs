// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Item 80 stage 2 batch 2: the calendar leg, driven against a real Stalwart
//! instead of the in-process mock.
//!
//! The twin of `live-stalwart-book.rs`, and deliberately narrower than
//! `calendar.rs`'s own mock-based leg: `functional-cal-live-client`
//! (`tests/functional/cal-live-client.c`) creates and reads back one plain
//! event only, not `functional-cal-client`'s whole run of an all-day event, a
//! zoned event, a six-occurrence recurring series with three kinds of
//! exception, a THISANDFUTURE split, and a second zoned recurring series — a
//! real server genuinely differing on any one of those would fail the whole
//! client rather than just mismeasure a field, which is a risk item 80
//! stage 2's own plan deferred to a later batch. See the client's own header.
//!
//! The mechanism is unchanged from the address-book leg: `spawn_loopback_proxy`
//! satisfies `jmap-backend-core::connect_target`'s plaintext-stays-loopback
//! rule honestly while the traffic actually continues to Stalwart;
//! `JMAP_FUNCTIONAL_STORE_PASSWORD` seeds the password a real server needs and
//! the mock never checks; `JMAP_LIVE_SERVER_REBASE_URLS` rebases the server's
//! stated `apiUrl` onto the address actually connected through.
//!
//! ## Running it
//!
//! Same environment as the `jmap-*-sync` crates' own `live_server_*.rs`
//! tests -- see `docs/manual-test-live-server.md`: a throwaway `stw seed`
//! account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};

const LOCATION: &str = "Room 42";
const CATEGORIES: &str = "offsite,planning";
const TRANSP: &str = "TRANSPARENT";
const PRIORITY: &str = "1";
const CLASS: &str = "CONFIDENTIAL";
const ALARM_TRIGGER: &str = "-PT15M";

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never mistakes a previous run's event for this
/// one's. Mirrors every `jmap-*-sync` live-server test's own copy.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

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

/// `calendar.rs`'s own keyfile, with a real `User=` added -- the one field
/// that turns `jmap-backend-core::connect::credentials`'s anonymous branch
/// into the Basic one.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart calendar test\n\
         Enabled=true\n\
         \n\
         [Calendar]\n\
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
fn evolution_opens_the_calendar_and_a_write_reaches_the_real_server() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server calendar leg"
        );
        return;
    };
    let client = required_path("JMAP_FUNCTIONAL_CAL_LIVE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-calendar"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    // Stalwart's session document states its own configured `apiUrl`, not the
    // loopback proxy address it was actually asked through -- see
    // `live-stalwart-book.rs`'s own comment on this line for the mechanism.
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let summary = format!("agent-fncal-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(&client, &["jmap-functional", &summary]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Checked before the exit status, same reason `live-stalwart-book.rs`
    // does: a read-only calendar turns every later failure into "Permission
    // denied", a message about the write that is really about the connect.
    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the calendar, with {}\n{report}",
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
        "EDS opened the real-server calendar read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    let added = seen
        .get("added")
        .unwrap_or_else(|| panic!("the client reported no added event\n{report}"));
    assert!(
        !added.is_empty(),
        "EDS added an event with no UID\n{report}"
    );

    assert_eq!(
        seen.get("read-back-summary"),
        Some(&summary.as_str()),
        "the event EDS handed back is not the one that went in\n{report}"
    );
    assert_eq!(
        seen.get("read-back-location"),
        Some(&LOCATION),
        "the event EDS handed back lost or moved its location\n{report}"
    );
    assert_eq!(
        seen.get("read-back-categories"),
        Some(&CATEGORIES),
        "the event EDS handed back lost or split its categories\n{report}"
    );
    assert_eq!(
        seen.get("read-back-transp"),
        Some(&TRANSP),
        "the event EDS handed back lost whether it blocks the time it occupies\n{report}"
    );
    assert_eq!(
        seen.get("read-back-priority"),
        Some(&PRIORITY),
        "the event EDS handed back lost its priority\n{report}"
    );
    assert_eq!(
        seen.get("read-back-class"),
        Some(&CLASS),
        "the event EDS handed back lost or mistranslated its privacy\n{report}"
    );
    assert_eq!(
        seen.get("read-back-alarm-trigger"),
        Some(&ALARM_TRIGGER),
        "the event EDS handed back lost its reminder\n{report}"
    );
    // Not `== "1"`: unlike the mock, which `calendar.rs` starts fresh every
    // run, a real account can carry events an earlier run of this same test
    // left behind. Presence, not the exact count, is the claim -- same
    // reasoning as `live-stalwart-book.rs`'s own `contacts-after` check.
    let events_after = seen
        .get("events-after")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("the client reported no parseable events-after\n{report}"));
    assert!(
        events_after >= 1,
        "the added event is not in the calendar it was added to\n{report}"
    );
}
