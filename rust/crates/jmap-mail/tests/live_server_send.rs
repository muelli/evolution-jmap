// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapTransport::send_message` against a real JMAP server, through the
//! actual `CamelTransport::send_to_sync` vfunc.
//!
//! `jmap-mail-sync/tests/live_server_send.rs` already proves `MailSync`'s own
//! identity lookup, outgoing-mailbox lookup, staging and submission against
//! real Stalwart, and `jmap-client/tests/live_server.rs` proves
//! `Client::send_email` the same way. Neither drives the GObject join in
//! between: `JmapTransport::send_message`'s per-step `retry_once_after`
//! wrapping, its connection `RwLock`, and the real `send_to_sync` vfunc Camel
//! calls when a user presses Send. `tests/send.rs` proves that join, but only
//! against `jmap-mockd`. This file is its live-server counterpart, using the
//! same `tests/common` GObject fixtures and the same two-account recipe
//! `jmap-mail-sync`'s send test uses.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests — see
//! `docs/manual-test-live-server.md` steps 3 and 3a:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing --test live_server_send -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` or
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` are unset.

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

/// The `CamelMimeMessage` `send_to_sync` is handed, parsed out of bytes —
/// mirrors `tests/send.rs::Message`.
struct Message(*mut CamelMimeMessage);

impl Message {
    fn parsed(source: &[u8]) -> Self {
        // SAFETY: a fresh message is a valid `CamelDataWrapper`, `source` is a
        // live buffer of the length given, and the error out-parameter is a
        // local that starts NULL.
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

/// One of the two address lists Camel gives `send_to_sync` — mirrors
/// `tests/send.rs::Addresses`.
struct Addresses(*mut CamelInternetAddress);

impl Addresses {
    fn of(entries: &[(&str, &str)]) -> Self {
        // SAFETY: a fresh address list, and NUL-terminated strings the setter
        // copies.
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

/// What one call to the vfunc answered — mirrors `tests/send.rs::Outcome`.
struct Outcome {
    sent: bool,
    saved: bool,
    error: *mut GError,
}

impl Outcome {
    fn message(&self) -> Option<String> {
        // SAFETY: the pointer is NULL or an owned `GError` this value holds,
        // and its message string outlives this borrow.
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
/// does — mirrors `tests/send.rs::send`.
fn send(
    transport: &Transport,
    message: &Message,
    from: &Addresses,
    recipients: &Addresses,
) -> Outcome {
    let mut saved: gboolean = GTRUE;
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live transport of ours, a live message, two live address
    // lists, and two out-parameters that are locals; the error starts NULL.
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

/// Sends a message through the real `send_to_sync` vfunc from the write-test
/// account to the recipient account, and polls the recipient's own
/// `MailSync::messages` until it actually lands — proving delivery, not just
/// an accepted submission, the same way the `MailSync`-level live-server send
/// test does.
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

    // Local delivery is not necessarily synchronous with the vfunc returning,
    // so poll rather than assume it has already landed.
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
