// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 176(b): restart survival for a queued offline append
//! (`notes/OFFLINE-JOURNAL-DESIGN-2026-11-05.md` section 4's last open row).
//!
//! A separate file, and so a separate `cargo test` binary and a separate
//! `cmake/Functional.cmake` registration, the same reason
//! `mail-offline-expunge.rs` gives: `mail.rs`'s own suite does not reliably
//! finish past eight tests in one binary.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_mock::EmailSeed;

fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::MailAccount, port).to_string()
}

/// Runs the client twice against the same session directory, the same way
/// `mail.rs`'s own `restart-prime`/`restart-second` pair simulates a process
/// restart at the Rust-functional level: a second process at the same
/// `XDG_CACHE_HOME`/`XDG_DATA_HOME` sees what an earlier process left on
/// disk. The first run (`offline-append-restart-prime`) connects normally,
/// goes offline and appends a draft, then exits while still offline, with
/// no reconnect. The second (`offline-append-restart-second`) starts with
/// the session already offline, the same as the shell's "Work Offline"
/// state at startup, and reads Drafts from disk alone before going back
/// online: a pending append that lost its row or its bytes on restart would
/// show as an empty folder here, rather than uploading on reconnect.
#[test]
fn a_draft_appended_while_offline_survives_a_restart_and_uploads_on_reconnect() {
    let client = required_path("JMAP_FUNCTIONAL_MAIL_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state.account_mut(&account_id).expect("the default account");
        let inbox = account.seed_mailbox("Inbox", Some("inbox"));
        account.seed_mailbox("Drafts", Some("drafts"));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Lunch on Tuesday",
            "One o'clock at the usual place.",
            "2026-01-14T09:00:00Z",
        ));
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/mail-offline-append-restart"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_camel_provider(&module, &urls);

    let prime_output = session.run(
        &client,
        &["jmap-functional", "offline-append-restart-prime"],
    );
    let prime_stdout = String::from_utf8_lossy(&prime_output.stdout);
    let prime_stderr = String::from_utf8_lossy(&prime_output.stderr);
    let prime_report =
        format!("--- client stdout ---\n{prime_stdout}--- client stderr ---\n{prime_stderr}");
    let prime_seen = observations(&prime_stdout);

    assert!(
        prime_output.status.success(),
        "the priming run failed with {}\n{prime_report}",
        prime_output.status
    );
    assert_eq!(
        prime_seen.get("drafts-count-before"),
        Some(&"0"),
        "Drafts should start empty\n{prime_report}"
    );
    assert_eq!(
        prime_seen.get("went-offline"),
        Some(&"1"),
        "the store never reported going offline\n{prime_report}"
    );
    assert_eq!(
        prime_seen.get("offline-append-restart-appended"),
        Some(&"1"),
        "appending a draft while offline should queue it, not fail\n{prime_report}"
    );
    assert!(
        !prime_seen.contains_key("went-online"),
        "the priming run must exit while still offline, with no reconnect\n{prime_report}"
    );

    let restart_output = session.run(
        &client,
        &["jmap-functional", "offline-append-restart-second"],
    );
    let restart_stdout = String::from_utf8_lossy(&restart_output.stdout);
    let restart_stderr = String::from_utf8_lossy(&restart_output.stderr);
    let restart_report =
        format!("--- client stdout ---\n{restart_stdout}--- client stderr ---\n{restart_stderr}");
    let restart_seen = observations(&restart_stdout);

    assert!(
        restart_output.status.success(),
        "the restarted run failed with {}\n{restart_report}",
        restart_output.status
    );
    assert_eq!(
        restart_seen.get("restart-drafts-count"),
        Some(&"1"),
        "the pending draft must still be listed after a restart with no \
         network reachable: it must not be lost\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("restart-drafts-subjects"),
        Some(&"Draft while offline"),
        "the restarted, still-offline run's draft does not match the one \
         the priming run appended\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("went-online"),
        Some(&"1"),
        "the store never reported going back online\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("drafts-count-after-reconnect"),
        Some(&"1"),
        "the pending draft must still be listed once reconnected\n{restart_report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let account = state.account(&account_id).expect("the default account");
    assert_eq!(
        account.emails.len(),
        2,
        "the mock should hold the seeded inbox message and the uploaded \
         draft that survived the restart\n{restart_report}"
    );

    let drafts = account
        .mailboxes
        .iter()
        .find(|(_, mailbox)| mailbox.role.as_deref() == Some("drafts"))
        .map(|(id, _)| id.clone())
        .expect("the drafts mailbox this test seeded");
    let uploaded = account
        .emails
        .iter()
        .map(|(_, email)| email)
        .find(|email| email.subject.as_deref() == Some("Draft while offline"))
        .expect("the uploaded draft on the server");
    assert_eq!(
        uploaded
            .mailbox_ids
            .as_ref()
            .map(|ids| ids.keys().collect::<Vec<_>>())
            .unwrap_or_default(),
        [&drafts],
        "the restart-survived append did not land in Drafts on the \
         server\n{restart_report}"
    );
}
