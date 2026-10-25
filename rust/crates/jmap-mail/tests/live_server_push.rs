// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Confirms, against a real JMAP server, a point that `jmap_mail::push`'s own
//! module doc only assumed: which `StateChange` types actually fire for a new
//! message and for a folder-list change.
//!
//! `jmap-mail/tests/push.rs` already covers every book-keeping behavior on
//! `JmapStore`'s side of a subscription (start, stop, header refresh), against
//! `jmap-mock` over a real socket. `jmap_mail::push::actions_for`'s own unit
//! tests cover the pure `types` -> [`jmap_mail::push::Work`] decision. Neither
//! has ever seen a real server's own `StateChange` payloads. `dispatch` needs
//! a live `JmapStore` GObject, which this test environment cannot construct
//! (the same limitation that `tests/push.rs`'s own doc comment records). So
//! this file drives [`jmap_backend_core::push::PushRefresh`] by hand, the same
//! way `tests/push.rs::subscription` does against `jmap-mock`, but against the
//! real server's own `eventSourceUrl`. This exercises the exact matching logic
//! that `dispatch` relies on (`jmap_backend_core::push`'s `concerns`), with a
//! real payload shape.
//!
//! ## A real finding: Stalwart never sends `EmailDelivery`
//!
//! `jmap_mail::push::PUSHED_TYPES` is `["Email", "EmailDelivery"]`. Its own
//! doc comment says that `jmap-mock` tracks only `Email` of the two, and that
//! a real server needs both. Before this file, no one had checked that
//! assumption against a real server.
//!
//! This file checks it with a raw `Email/import` against Stalwart, while an
//! unfiltered (`types=*`) `eventSourceUrl` stream is attached. The pushed
//! `StateChange` names `Thread`, `Mailbox`, and `Email`, never
//! `EmailDelivery`.
//!
//! So on Stalwart, `EmailDelivery` is not merely redundant with `Email`; it is
//! dead weight. Subscribing to it changes nothing that this crate has ever
//! seen fire. Nothing needs fixing here: `dispatch` already treats the two
//! types as either/or, so it does not depend on `EmailDelivery` ever
//! arriving. `PUSHED_TYPES` keeps asking for both, because a future or
//! different server may use `EmailDelivery`, and asking costs nothing.
//!
//! This is logged as `RFC-SUSPECT` in the night log. Whether never emitting
//! the RFC 8621 §5 delivery-only pseudo-type is within spec is for the
//! maintainer to judge.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests; see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! The test is skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER` or
//! `_PASSWORD` are unset.

use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use jmap_backend_core::push::PushRefresh;
use jmap_client::Client;
use jmap_client::eventsource::expand_url;
use jmap_mail_sync::{Keywords, MailSync};
use jmap_proto::mail::{Mailbox, role};
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_test_support::{connect_for_write, unique_suffix};

/// Subscribes to `types` for `client`'s own account, in the exact shape that
/// `JmapStore::start_push` builds (see
/// `jmap_backend_core::push::start_for_with`). Returns the subscription
/// together with a receiver of every matched-types list that the pump
/// observes.
fn subscribe(
    client: &Client,
    account_id: &jmap_proto::Id,
    types: &[&str],
) -> (PushRefresh, Receiver<Vec<String>>) {
    let (tx, rx) = channel();
    let url = expand_url(client.session().event_source_url.trim(), types, false, 0);
    let headers = client
        .authorization_header()
        .map(|value| vec![("Authorization".to_owned(), value)])
        .unwrap_or_default();
    let push = PushRefresh::start(
        url,
        headers,
        account_id.clone(),
        types.iter().map(|name| (*name).to_owned()).collect(),
        move |matched: &[String]| {
            let _ = tx.send(matched.to_vec());
        },
    );
    (push, rx)
}

/// Waits for a match containing `wanted`. Tolerates unrelated matches, because
/// the account may have other traffic in a shared test domain.
fn wait_for(rx: &Receiver<Vec<String>>, wanted: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match rx.recv_timeout(remaining) {
            Ok(matched) if matched.iter().any(|name| name == wanted) => return true,
            Ok(_) => continue,
            Err(RecvTimeoutError::Timeout) => return false,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

/// A new message imported into the Inbox pushes a `StateChange` naming
/// `Email`. This test confirms that against a real server for the first time.
/// It subscribes to both `Email` and `EmailDelivery`
/// (`jmap_mail::push::PUSHED_TYPES` exactly), but requires only `Email`. Per
/// this file's module doc, a raw probe against Stalwart showed that
/// `EmailDelivery` never fires there.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn importing_a_message_pushes_an_email_state_change() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let inbox_id = client
        .mailbox_get(&account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::INBOX))
        .expect("the write-test account needs an Inbox")
        .id
        .expect("the server named the Inbox");

    let (_push, rx) = subscribe(&client, &account_id, &["Email", "EmailDelivery"]);

    let sync = MailSync::new(client, account_id);
    let subject = format!("agent-mailpush-{}", unique_suffix());
    let message = format!(
        "From: agent-mailpush@example.invalid\r\n\
         To: agent-mailpush@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         It arrived to check which StateChange types a real server sends.\r\n"
    );
    sync.import_message(&inbox_id, message.into_bytes(), &Keywords::default(), None)
        .expect("Email/import failed against the real server");

    assert!(
        wait_for(&rx, "Email", Duration::from_secs(15)),
        "the real server never pushed an Email StateChange after Email/import"
    );
}

/// Creating a mailbox pushes a `StateChange` naming `Mailbox`. This is the
/// type that `jmap_mail::push::FOLDER_LIST_TYPES` watches, to decide whether
/// the folder list of Camel changed.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn creating_a_mailbox_pushes_a_mailbox_state_change() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let (_push, rx) = subscribe(&client, &account_id, &["Mailbox"]);

    let name = format!("agent-mailpush-folder-{}", unique_suffix());
    client
        .mailbox_create(&account_id, &Mailbox::new(name))
        .expect("Mailbox/set create failed against the real server");

    assert!(
        wait_for(&rx, "Mailbox", Duration::from_secs(15)),
        "the real server never pushed a Mailbox StateChange after Mailbox/set create"
    );
}
