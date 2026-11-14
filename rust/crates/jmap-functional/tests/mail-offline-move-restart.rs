// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 181(a) section 4: does `synchronize_sync` run, on
//! reconnect, for a folder that holds a queued filing but that this
//! process has not reopened since the move?
//! (`notes/OFFLINE-QUEUE-VISIBILITY-DESIGN-2026-11-14.md` section 4.)
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

/// Runs the client twice against the same session directory, the
/// `restart-prime`/`restart-second` shape `mail-offline-append-restart.rs`
/// already uses. The first run (`offline-move-restart-prime`) connects
/// normally, reads Inbox and Trash once, goes offline, moves the inbox
/// message into Trash (queued, not landed: `transfer.rs`'s offline-filing
/// path), then exits while still offline with both folder objects dropped.
/// The second (`offline-move-restart-second`) starts a fresh process, with
/// neither folder reopened, and goes straight online: this is the blind
/// reconnect flush the design note's section 4 asks about. It then opens
/// both folders and synchronizes Inbox directly, to see whether the queued
/// move survives and lands once something does synchronize the folder that
/// holds it.
#[test]
fn a_queued_move_does_not_flush_on_a_blind_reconnect_but_survives_and_lands_once_its_folder_is_synchronized()
 {
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
        "/mail-offline-move-restart"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_camel_provider(&module, &urls);

    let prime_output = session.run(&client, &["jmap-functional", "offline-move-restart-prime"]);
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
        prime_seen.get("offline-move-restart-uid"),
        Some(&email_id.as_str()),
        "the client queued a different message than the one seeded\n{prime_report}"
    );
    assert_eq!(
        prime_seen.get("went-offline"),
        Some(&"1"),
        "the store never reported going offline\n{prime_report}"
    );
    assert_eq!(
        prime_seen.get("offline-move-restart-queued"),
        Some(&"1"),
        "moving a message into Trash while offline should queue the \
         filing, not fail\n{prime_report}"
    );
    assert_eq!(
        prime_seen.get("inbox-count-after-offline-move"),
        Some(&"1"),
        "a queued move leaves its row in the source folder until the \
         filing lands (176(b)'s existing rule)\n{prime_report}"
    );
    assert_eq!(
        prime_seen.get("trash-count-after-offline-move"),
        Some(&"0"),
        "the destination must stay empty until the filing lands\n{prime_report}"
    );
    assert!(
        !prime_seen.contains_key("went-online"),
        "the priming run must exit while still offline, with no reconnect\n{prime_report}"
    );

    let restart_output = session.run(&client, &["jmap-functional", "offline-move-restart-second"]);
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
        restart_seen.get("went-online"),
        Some(&"1"),
        "the store never reported going back online\n{restart_report}"
    );

    // The answer to section 4: a blind reconnect, with neither folder
    // reopened since the queued move, does not flush it. Camel's own
    // reconnect walk only visits folders already live in the store's
    // object bag, which starts empty in a fresh process.
    assert_eq!(
        restart_seen.get("restart-inbox-count-blind-reconnect"),
        Some(&"1"),
        "a blind reconnect must not have flushed the queued move yet: the \
         row should still be in Inbox\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("restart-trash-count-blind-reconnect"),
        Some(&"0"),
        "a blind reconnect must not have flushed the queued move yet: \
         Trash should still be empty\n{restart_report}"
    );

    // The queued move is not lost: once Inbox is reopened and
    // synchronized directly, as Evolution does when a folder is selected
    // or closed, the filing lands.
    assert_eq!(
        restart_seen.get("inbox-synchronized-after-reopen"),
        Some(&"1"),
        "synchronizing the reopened Inbox should succeed\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("restart-inbox-count-after-synchronize"),
        Some(&"0"),
        "the queued move should have left Inbox once its folder was \
         synchronized\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("restart-trash-count-after-synchronize"),
        Some(&"1"),
        "the queued move should have landed in Trash once its folder was \
         synchronized\n{restart_report}"
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
        "the queued move did not reach the server's Trash mailbox once its \
         folder was synchronized\n{restart_report}"
    );
}
