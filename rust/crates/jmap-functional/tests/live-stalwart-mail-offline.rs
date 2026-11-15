// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 181's live verification: does a move queued while offline
//! (181(a)) survive a restart and land on a real JMAP server, not only on
//! `jmap-mock`?
//!
//! `rust/crates/jmap-functional/tests/mail-offline-move-restart.rs` answers
//! this against the mock. This file runs the same `restart-prime`/
//! `restart-second` pair against a real Stalwart, through
//! `tests/functional/mail-live-client.c`'s `offline-move-restart-prime`/
//! `offline-move-restart-second` phases, the same way
//! `live-stalwart-mail.rs` runs its phases against a real server instead of
//! the mock.
//!
//! A separate file, and so a separate `cargo test` binary and its own
//! `cmake/Functional.cmake` registration, for the reason
//! `mail-offline-move-restart.rs` gives.
//!
//! ## How to run it
//!
//! Use the same environment as `live-stalwart-mail.rs` (see
//! `docs/manual-test-live-server.md`): a throwaway `stw seed` account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-mail-offline -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;

use jmap_functional::{
    Keyfile, Session, SourceKind, observations, required_path, spawn_loopback_proxy,
};
use jmap_test_support::unique_suffix;

/// `(origin, user, password)`, or `None` to skip. A copy of the function of
/// the same name in `live-stalwart-mail.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// The keyfile of `mail.rs`, with a real `User=`. A copy of the function of
/// the same name in `live-stalwart-mail.rs`.
fn keyfile(port: u16, user: &str) -> String {
    Keyfile::new(SourceKind::MailAccount, port)
        .display_name("JMAP functional live-Stalwart mail offline test")
        .user(user)
        .to_string()
}

/// One observation as a number. Panics with the name of the observation if it
/// is missing or not a number. A copy of the function of the same name in
/// `live-stalwart-mail.rs`.
fn parsed(seen: &std::collections::BTreeMap<&str, &str>, key: &str, report: &str) -> u32 {
    seen.get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("the client reported no parseable {key}\n{report}"))
}

/// Runs the client twice against the same session directory, the live
/// equivalent of `mail-offline-move-restart.rs`'s own pair. The first run
/// (`offline-move-restart-prime`) connects normally, appends a message, goes
/// offline and moves it into Trash: `transfer_messages_to_sync`'s offline
/// queue (181(a)) must queue the move, not fail, even against the real
/// provider and a real `CamelStore`. The second
/// (`offline-move-restart-second`) starts a fresh process, with neither
/// folder reopened, reads Trash from disk, then goes online and
/// synchronizes Inbox directly: the queued move must land on the real
/// server.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_queued_move_made_offline_survives_a_restart_and_lands_on_the_real_server() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server offline leg"
        );
        return;
    };

    let client = required_path("JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-mail-offline"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    // The session document gives the configured `apiUrl`, not the proxy
    // address. See the comment on this line in `live-stalwart-book.rs`.
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_camel_provider(&module, &urls);

    let subject = format!("agent-fnoffline-{:x}", unique_suffix() & 0xffff_ffff_ffff);

    let prime_output = session.run(
        &client,
        &["jmap-functional", &subject, "offline-move-restart-prime"],
    );
    let prime_stdout = String::from_utf8_lossy(&prime_output.stdout);
    let prime_stderr = String::from_utf8_lossy(&prime_output.stderr);
    let prime_report =
        format!("--- client stdout ---\n{prime_stdout}--- client stderr ---\n{prime_stderr}");
    let prime_seen = observations(&prime_stdout);

    assert!(
        prime_output.status.success(),
        "the priming run failed against the real server with {}\n{prime_report}",
        prime_output.status
    );
    assert_eq!(
        prime_seen.get("went-offline"),
        Some(&"1"),
        "the store never reported going offline\n{prime_report}"
    );
    assert_eq!(
        prime_seen.get("offline-move-restart-queued"),
        Some(&"1"),
        "moving a message into Trash while offline should queue it, not fail\n{prime_report}"
    );

    let restart_output = session.run(
        &client,
        &["jmap-functional", &subject, "offline-move-restart-second"],
    );
    let restart_stdout = String::from_utf8_lossy(&restart_output.stdout);
    let restart_stderr = String::from_utf8_lossy(&restart_output.stderr);
    let restart_report =
        format!("--- client stdout ---\n{restart_stdout}--- client stderr ---\n{restart_stderr}");
    let restart_seen = observations(&restart_stdout);

    assert!(
        restart_output.status.success(),
        "the restarted run failed against the real server with {}\n{restart_report}",
        restart_output.status
    );
    assert_eq!(
        restart_seen.get("went-online"),
        Some(&"1"),
        "the restarted process never reported going online\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("inbox-synchronized-after-reopen"),
        Some(&"1"),
        "the restarted process never synchronized Inbox directly\n{restart_report}"
    );

    // The main check: the queued move actually reached the real server, not
    // only the local summary. A second store with no summary database would
    // show this more directly, but `restart-second` already opens a fresh
    // process at a fresh moment; the synchronize call above is what would
    // send the queued filing, so the count after it is the server's answer.
    let trash_before = parsed(
        &restart_seen,
        "restart-trash-count-blind-reconnect",
        &restart_report,
    );
    let trash_after = parsed(
        &restart_seen,
        "restart-trash-count-after-synchronize",
        &restart_report,
    );
    assert_eq!(
        trash_after,
        trash_before + 1,
        "the queued move never reached Trash on the real server\n{restart_report}"
    );
}
