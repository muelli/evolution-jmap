// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapTransport::send_message` against a real JMAP server, through the
//! actual `CamelTransport::send_to_sync` vfunc.
//!
//! `jmap-mail-sync/tests/live_server_send.rs` already proves `MailSync`'s own
//! identity lookup, outgoing-mailbox lookup, staging, and submission against
//! real Stalwart. `jmap-client/tests/live_server.rs` proves
//! `Client::send_email` the same way. Neither test drives the GObject join:
//! the per-step `retry_once_after` wrapping of `JmapTransport::send_message`,
//! its connection `RwLock`, and the real `send_to_sync` vfunc that Camel
//! calls when a user presses Send. `tests/send.rs` proves that join, but only
//! against `jmap-mockd`. This file is the live-server counterpart. It uses
//! the same `tests/common` GObject fixtures and the same two-account recipe
//! as `jmap-mail-sync`'s send test.
//!
//! ## Running it
//!
//! This test needs the same environment as the other live-server tests. See
//! `docs/manual-test-live-server.md` steps 3 and 3a:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing --test live_server_send -- --ignored
//! ```
//!
//! The test is skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`,
//! `_PASSWORD`, `JMAP_LIVE_SERVER_RECIPIENT_USER`, or `_PASSWORD` is unset.

mod common;

use std::env;
use std::ptr;
use std::time::Duration;

use common::{Account, Transport};
use eds_sys::{
    CamelAddress, CamelDataWrapper, CamelInternetAddress, CamelMimeMessage, CamelTransport,
    camel_data_wrapper_construct_from_data_sync, camel_internet_address_add,
    camel_internet_address_new, camel_mime_message_new, camel_transport_send_to_sync,
};
use glib_sys::{GError, GFALSE, GTRUE, g_clear_error, gboolean, gssize};
use gobject_sys::g_object_unref;
use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderRole, MailSync};

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-mail-sync/tests/live_server_send.rs::connect_for_write`
/// exactly.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the write-test account"),
    )
}

/// Mirrors `jmap-mail-sync/tests/live_server_send.rs::connect_recipient`
/// exactly.
fn connect_recipient() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the recipient account"),
    )
}

/// The `CamelMimeMessage` that `send_to_sync` receives, parsed from bytes.
/// Mirrors `tests/send.rs::Message`.
struct Message(*mut CamelMimeMessage);

impl Message {
    fn parsed(source: &[u8]) -> Self {
        // SAFETY: a fresh message is a valid `CamelDataWrapper`. `source` is a
        // live buffer with the given length. The error out-parameter is a
        // local variable that starts as NULL.
        unsafe {
            let message = camel_mime_message_new();
            let mut error: *mut GError = ptr::null_mut();
            let parsed = camel_data_wrapper_construct_from_data_sync(
                message.cast::<CamelDataWrapper>(),
                source.as_ptr().cast(),
                source.len() as gssize,
                ptr::null_mut(),
                &mut error,
            );
            assert_ne!(parsed, GFALSE, "the fixture message would not parse");
            Self(message)
        }
    }
}

impl Drop for Message {
    fn drop(&mut self) {
        // SAFETY: the one reference, taken at construction.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// One of the two address lists that Camel gives `send_to_sync`. Mirrors
/// `tests/send.rs::Addresses`.
struct Addresses(*mut CamelInternetAddress);

impl Addresses {
    fn of(entries: &[(&str, &str)]) -> Self {
        // SAFETY: a fresh address list. The setter copies NUL-terminated
        // strings.
        unsafe {
            let list = camel_internet_address_new();
            for (name, email) in entries {
                let name = std::ffi::CString::new(*name).expect("a name with no NUL in it");
                let email = std::ffi::CString::new(*email).expect("an address with no NUL in it");
                camel_internet_address_add(list, name.as_ptr(), email.as_ptr());
            }
            Self(list)
        }
    }

    fn as_camel(&self) -> *mut CamelAddress {
        self.0.cast()
    }
}

impl Drop for Addresses {
    fn drop(&mut self) {
        // SAFETY: the one reference, taken at construction.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The result of one call to the vfunc. Mirrors `tests/send.rs::Outcome`.
struct Outcome {
    sent: bool,
    saved: bool,
    error: *mut GError,
}

impl Outcome {
    fn message(&self) -> Option<String> {
        // SAFETY: the pointer is NULL, or an owned `GError` that this value
        // holds. Its message string outlives this borrow.
        unsafe {
            self.error.as_ref().map(|error| {
                std::ffi::CStr::from_ptr(error.message)
                    .to_string_lossy()
                    .into_owned()
            })
        }
    }
}

impl Drop for Outcome {
    fn drop(&mut self) {
        // SAFETY: NULL or an owned error, which is what `g_clear_error` takes.
        unsafe { g_clear_error(&mut self.error) };
    }
}

/// Sends `message` through `transport` the way `e_mail_session_send_to`
/// does. Mirrors `tests/send.rs::send`.
fn send(
    transport: &Transport,
    message: &Message,
    from: &Addresses,
    recipients: &Addresses,
) -> Outcome {
    let mut saved: gboolean = GTRUE;
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live transport, a live message, and two live address lists.
    // Both out-parameters are locals, and the error starts as NULL.
    let sent = unsafe {
        camel_transport_send_to_sync(
            transport.service.cast::<CamelTransport>(),
            message.0,
            from.as_camel(),
            recipients.as_camel(),
            &mut saved,
            ptr::null_mut(),
            &mut error,
        )
    };
    Outcome {
        sent: sent != GFALSE,
        saved: saved != GFALSE,
        error,
    }
}

/// Sends a message through the real `send_to_sync` vfunc, from the
/// write-test account to the recipient account. Polls the recipient's own
/// `MailSync::messages` until the message lands. This proves delivery, not
/// just an accepted submission, the same way the `MailSync`-level
/// live-server send test does.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_message_sent_through_the_real_vfunc_is_delivered_by_the_real_server() {
    let Some(sender_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the send test");
        return;
    };
    let Some(recipient_client) = connect_recipient() else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the send test");
        return;
    };

    let sender_email = env::var("JMAP_LIVE_SERVER_WRITE_USER").unwrap();
    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();

    let sender_account_id = sender_client
        .primary_account(jmap_proto::session::CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let sync = MailSync::new(sender_client, sender_account_id);

    let account = Account::open();
    let transport = Transport::open(&account);
    transport.connect(sync);

    let subject = format!("agent-transport-send-{}", unique_suffix());
    let message = format!(
        "From: {sender_email}\r\n\
         To: {recipient_email}\r\n\
         Subject: {subject}\r\n\
         Message-ID: <{subject}@agent-livewrite.net>\r\n\
         Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
         \r\n\
         Sent via JmapTransport::send_message against a real server.\r\n"
    );

    let outcome = send(
        &transport,
        &Message::parsed(message.as_bytes()),
        &Addresses::of(&[("", sender_email.as_str())]),
        &Addresses::of(&[("", recipient_email.as_str())]),
    );
    assert!(outcome.sent, "the send failed: {:?}", outcome.message());

    let recipient_account_id = recipient_client
        .primary_account(jmap_proto::session::CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");
    let recipient_sync = MailSync::new(recipient_client, recipient_account_id);
    let (_, tree) = recipient_sync
        .folder_tree()
        .expect("listing the recipient's folder tree failed");
    let inbox_id = tree
        .role(FolderRole::Inbox)
        .expect("the recipient account needs an Inbox")
        .id
        .clone();

    // Local delivery does not necessarily happen before the vfunc returns, so
    // poll rather than assume the message has already landed.
    let mut delivered = None;
    for _ in 0..20 {
        let (_, messages) = recipient_sync
            .messages(&inbox_id)
            .expect("listing the recipient's Inbox failed");
        if let Some(row) = messages
            .into_iter()
            .find(|row| row.subject.as_deref() == Some(subject.as_str()))
        {
            delivered = Some(row);
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let delivered = delivered.unwrap_or_else(|| {
        panic!(
            "the message sent through the real send_to_sync vfunc never showed up in the \
             recipient's Inbox after 20s of polling"
        )
    });

    assert_eq!(delivered.subject.as_deref(), Some(subject.as_str()));
    assert!(
        outcome.saved,
        "the write-test account's own sent copy was not reported as saved"
    );
}

/// `jmap-mail-sync/tests/live_server_send.rs`'s
/// `an_envelope_recipient_absent_from_every_header_is_still_delivered` proves
/// that real Stalwart honours `envelope.rcptTo` over headers, but only by
/// giving `MailSync::send_message` a hand-built `Envelope` directly.
/// `tests/send.rs`'s
/// `the_addresses_camel_hands_over_are_the_envelope_that_goes_out` proves
/// that `src/envelope.rs`'s `read_envelope` builds that same divergence from
/// a real `CamelAddress` recipients list, but only against the mock. Neither
/// proves the join: a real `CamelAddress` list, with a Bcc-shaped recipient
/// absent from every header, translated by `read_envelope`, and delivered by
/// a real server through the actual `send_to_sync` vfunc. This test proves
/// that join. The message's only header-named party is the sender. The
/// recipient's address appears only in the `recipients` argument.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_recipient_absent_from_every_header_is_still_delivered_through_the_real_vfunc() {
    let Some(sender_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the send test");
        return;
    };
    let Some(recipient_client) = connect_recipient() else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the send test");
        return;
    };

    let sender_email = env::var("JMAP_LIVE_SERVER_WRITE_USER").unwrap();
    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();

    let sender_account_id = sender_client
        .primary_account(jmap_proto::session::CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let sync = MailSync::new(sender_client, sender_account_id);

    let account = Account::open();
    let transport = Transport::open(&account);
    transport.connect(sync);

    let subject = format!("agent-transport-bcc-{}", unique_suffix());
    // `To` names the sender, not the recipient. The recipient's address is
    // nowhere in the source bytes, the same way a real Bcc works.
    let message = format!(
        "From: {sender_email}\r\n\
         To: {sender_email}\r\n\
         Subject: {subject}\r\n\
         Message-ID: <{subject}@agent-livewrite.net>\r\n\
         Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
         \r\n\
         Sent via JmapTransport::send_message, with no header naming the recipient.\r\n"
    );

    let outcome = send(
        &transport,
        &Message::parsed(message.as_bytes()),
        &Addresses::of(&[("", sender_email.as_str())]),
        &Addresses::of(&[("", recipient_email.as_str())]),
    );
    assert!(outcome.sent, "the send failed: {:?}", outcome.message());

    let recipient_account_id = recipient_client
        .primary_account(jmap_proto::session::CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");
    let recipient_sync = MailSync::new(recipient_client, recipient_account_id);
    let (_, tree) = recipient_sync
        .folder_tree()
        .expect("listing the recipient's folder tree failed");
    let inbox_id = tree
        .role(FolderRole::Inbox)
        .expect("the recipient account needs an Inbox")
        .id
        .clone();

    // Local delivery does not necessarily happen before the vfunc returns, so
    // poll rather than assume the message has already landed.
    let mut delivered = None;
    for _ in 0..20 {
        let (_, messages) = recipient_sync
            .messages(&inbox_id)
            .expect("listing the recipient's Inbox failed");
        if let Some(row) = messages
            .into_iter()
            .find(|row| row.subject.as_deref() == Some(subject.as_str()))
        {
            delivered = Some(row);
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let delivered = delivered.unwrap_or_else(|| {
        panic!(
            "the message sent through the real send_to_sync vfunc (recipient in the envelope \
             only) never showed up in that recipient's Inbox after 20s of polling — the real \
             server appears to deliver from headers, not the envelope"
        )
    });

    assert_eq!(delivered.subject.as_deref(), Some(subject.as_str()));
    assert!(
        delivered.to.iter().all(|to| to.email != recipient_email),
        "the delivered message's own To header names the envelope-only recipient \
         ({recipient_email}); it should name only the sender, the same way a real Bcc \
         recipient's address never appears in the headers"
    );
}
