// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 176(b): the offline-journal fix for `expunge_sync`.
//!
//! A separate file, and so a separate `cargo test` binary and a separate
//! `cmake/Functional.cmake` registration, deliberately not a ninth test in
//! `mail.rs`: that file's own suite does not reliably finish past eight
//! tests in one binary (see `audits/OFFLINE-MATRIX-2026-11-03.md`), a gap
//! still not root-caused. A new file sidesteps it rather than risking it.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_mock::EmailSeed;

fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::MailAccount, port).to_string()
}

/// Marking a message deleted and expunging it while offline must succeed
/// locally, the way a flag change already does, and the deferred removal
/// must reach the server once reconnected, with no explicit synchronize
/// call from the client: the same generic `CamelOfflineStore` reconnect
/// flush `mail.rs`'s `going_back_online_flushes_a_flag_changed_while_
/// offline` already proves exists.
#[test]
fn a_message_expunged_while_offline_reaches_the_server_on_reconnect() {
    let client = required_path("JMAP_FUNCTIONAL_MAIL_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let email_id = {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state.account_mut(&account_id).expect("the default account");
        let inbox = account.seed_mailbox("Inbox", Some("inbox"));
        account.seed_mailbox("Trash", Some("trash"));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Lunch on Tuesday",
            "One o'clock at the usual place.",
            "2026-01-14T09:00:00Z",
        ))
    };

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/mail-offline-expunge"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_camel_provider(&module, &urls);

    let output = session.run(&client, &["jmap-functional", "offline-expunge-roundtrip"]);
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
        seen.get("went-offline"),
        Some(&"1"),
        "the store never reported going offline\n{report}"
    );
    assert_eq!(
        seen.get("went-online"),
        Some(&"1"),
        "the store never reported going back online\n{report}"
    );
    assert_eq!(
        seen.get("offline-expunge-uid"),
        Some(&email_id.as_str()),
        "the client expunged a different message than the one seeded\n{report}"
    );
    assert_eq!(
        seen.get("offline-expunge-succeeded"),
        Some(&"1"),
        "expunging a deleted message while offline must succeed locally, \
         the way a flag change does, leaving the removal for the \
         reconnect flush\n{report}"
    );
    assert_eq!(
        seen.get("inbox-count-after-offline-expunge"),
        Some(&"1"),
        "the row must stay in the folder's own listing while offline: \
         removal happens only after a server write actually confirms it\n{report}"
    );
    assert_eq!(
        seen.get("inbox-count-after-reconnect"),
        Some(&"0"),
        "the deferred expunge never reached the server once the store \
         went back online\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let account = state.account(&account_id).expect("the default account");
    let trash = account
        .mailboxes
        .iter()
        .find(|(_, mailbox)| mailbox.role.as_deref() == Some("trash"))
        .map(|(id, _)| id.clone())
        .expect("the trash this test seeded");
    let mailboxes = account
        .emails
        .get(&email_id)
        .and_then(|email| email.mailbox_ids.clone())
        .unwrap_or_default();
    assert_eq!(
        mailboxes.keys().collect::<Vec<_>>(),
        [&trash],
        "the deferred expunge did not move message {email_id} into the \
         trash on the server\n{report}"
    );
}
