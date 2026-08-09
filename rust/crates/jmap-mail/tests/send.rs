// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `send_to_sync`: the vfunc a JMAP account's mail leaves through.
//!
//! Every piece under it has been built and tested on its own — [`jmap_mail::envelope`]
//! for the two address lists Camel hands over, [`jmap_mail::mime`] for the bytes
//! the message goes up as, and `MailSync`'s `identity_for`,
//! `outgoing_mailboxes` and `send_message` for the account-side lookups and the
//! import-and-submit itself. What these tests are about is the join: that
//! pressing Send in Evolution reaches all five in the right order, that a
//! refusal costs nothing when it can be known before the upload, and that the
//! one answer Camel takes back besides success or failure —
//! `out_sent_message_saved` — is right.
//!
//! ## The out-parameter is not a detail
//!
//! `out_sent_message_saved` tells Evolution whether the transport has already
//! saved the sent copy, and a caller told `FALSE` saves one of its own into
//! whatever folder the account names. Getting it wrong is not a cosmetic
//! failure in either direction: `FALSE` when the message *is* in Sent gives the
//! user two of every message they send, and `TRUE` when it is not loses the
//! sent copy. It is [`OutgoingMailboxes::saves_sent_copy`] all the way down, and
//! [`an_account_whose_only_outgoing_mailbox_is_sent_has_already_saved_the_copy`]
//! is the case where it differs from "was a destination mailbox needed".
//!
//! ## What is not sent
//!
//! Four refusals reach the wire not at all: an envelope with no sender or no
//! recipients, a transport with no connection, an address the account has no
//! identity for, and an account with nowhere to stage an outgoing message. The
//! tests below check the code Camel gets *and* that the account is untouched —
//! a refusal that had already imported the message would leave a draft behind
//! for a send the user was told did not happen.
//!
//! [`OutgoingMailboxes::saves_sent_copy`]: jmap_mail_sync::OutgoingMailboxes::saves_sent_copy

mod common;

use std::ffi::CStr;
use std::ptr;

use common::{Account, Transport};
use eds_sys::{
    CAMEL_SERVICE_ERROR_INVALID, CAMEL_SERVICE_ERROR_NOT_CONNECTED, CamelAddress, CamelDataWrapper,
    CamelInternetAddress, CamelMimeMessage, CamelTransport, CamelTransportClass,
    camel_data_wrapper_construct_from_data_sync, camel_internet_address_add,
    camel_internet_address_new, camel_mime_message_new, camel_service_error_quark,
    camel_transport_send_to_sync,
};
use glib_sys::{GError, GFALSE, GTRUE, g_clear_error, gboolean, gssize};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::{Client, Credentials};
use jmap_mail::transport::transport_type;
use jmap_mail_sync::{MailSync, MessageSummary};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::Envelope;

/// The RFC 5322 bytes Evolution's composer hands a transport. The addresses in
/// the headers are deliberately *not* the ones the envelope carries in most of
/// the tests below — that is the ordinary case, not a contrived one.
const MESSAGE: &[u8] = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

/// A connected transport on its own session, with an account behind it.
struct Fixture {
    server: MockServer,
    account_id: Id,
    account: Account,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        Self {
            server,
            account_id,
            account: Account::open(),
        }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).expect("connected");
        MailSync::new(client, self.account_id.clone())
    }

    /// The transport Camel would construct, with a connection already on it —
    /// which is what `authenticate_sync` leaves behind.
    fn transport(&self) -> Transport<'_> {
        let transport = Transport::open(&self.account);
        transport.connect(self.sync());
        transport
    }

    fn seed_mailbox(&self, name: &str, role: Option<&str>) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_mailbox(name, role)
    }

    fn seed_identity(&self, name: &str, email: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_identity(name, email)
    }

    /// What the mailbox holds now, as a folder refresh would find it.
    fn listing(&self, mailbox: &Id) -> Vec<MessageSummary> {
        let (_, messages) = self.sync().messages(mailbox).expect("the mailbox lists");
        messages
    }

    /// The envelope of the one submission the server accepted.
    fn only_submission(&self) -> (Id, Id, Envelope) {
        let state = self.server.state();
        let state = state.lock().unwrap();
        let outbox = &state.account(&self.account_id).unwrap().outbox;
        assert_eq!(outbox.len(), 1, "one accepted submission");
        let recorded = outbox.first().expect("the submission just counted");
        (
            recorded.email_id.clone(),
            recorded.identity_id.clone(),
            recorded.envelope.clone(),
        )
    }

    fn outbox_is_empty(&self) -> bool {
        let state = self.server.state();
        let state = state.lock().unwrap();
        state.account(&self.account_id).unwrap().outbox.is_empty()
    }
}

/// The `CamelMimeMessage` the composer would hand over, parsed out of bytes.
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

/// One of the two address lists Camel gives `send_to_sync`.
struct Addresses(*mut CamelInternetAddress);

impl Addresses {
    /// A list of `(display name, addr-spec)` pairs; an empty name is the
    /// bare-address case Evolution's outbox produces.
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

/// What one call to the vfunc answered: whether it succeeded, whether it says
/// the sent copy is saved, and the failure it reported.
struct Outcome {
    sent: bool,
    saved: bool,
    error: *mut GError,
}

impl Outcome {
    /// The `GError`'s domain and code, or `None` where there was no failure.
    fn failure(&self) -> Option<(u32, i32)> {
        // SAFETY: the pointer is NULL or an owned `GError` this value holds.
        unsafe { self.error.as_ref() }.map(|error| (error.domain, error.code))
    }

    fn message(&self) -> Option<String> {
        // SAFETY: as above; the string belongs to the error and outlives this
        // borrow.
        unsafe {
            self.error
                .as_ref()
                .map(|error| CStr::from_ptr(error.message).to_string_lossy().into_owned())
        }
    }
}

impl Drop for Outcome {
    fn drop(&mut self) {
        // SAFETY: NULL or an owned error, which is what `g_clear_error` takes.
        unsafe { g_clear_error(&mut self.error) };
    }
}

/// Sends `message` through `transport` the way `e_mail_session_send_to` does —
/// through the public wrapper, so that Camel's own preconditions are in the
/// path.
fn send(
    transport: &Transport,
    message: &Message,
    from: &Addresses,
    recipients: &Addresses,
) -> Outcome {
    let mut saved: gboolean = GTRUE;
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live transport of ours, a live message, two live address lists,
    // and two out-parameters that are locals; the error starts NULL.
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

/// The service-domain quark every refusal below is reported in.
fn service_domain() -> u32 {
    // SAFETY: no arguments, and the quark registers itself on first use.
    unsafe { camel_service_error_quark() }
}

/// The one row a mailbox holds.
fn only(messages: Vec<MessageSummary>) -> MessageSummary {
    assert_eq!(messages.len(), 1, "one message");
    messages.into_iter().next().expect("the row just counted")
}

// ---------------------------------------------------------------------------
// the slot

/// Camel dispatches `send_to_sync` through the class, and
/// `camel_transport_send_to_sync` is a `g_return_val_if_fail
/// (class->send_to_sync != NULL, FALSE)`: a transport that installed none would
/// be an account that offers to send and answers a GLib critical.
#[test]
fn the_transport_installs_the_send_vfunc() {
    // SAFETY: a live GType of a class that has one; the reference is given back.
    unsafe {
        let class = g_type_class_ref(transport_type()).cast::<CamelTransportClass>();
        assert!(
            (*class).send_to_sync.is_some(),
            "the transport class has no send_to_sync"
        );
        g_type_class_unref(class.cast());
    }
}

// ---------------------------------------------------------------------------
// a message that goes out

#[test]
fn a_message_the_composer_built_is_submitted_and_filed_where_sent_mail_belongs() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));
    let identity = fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[("Bob", "bob@example.com")]),
    );

    assert!(outcome.sent, "the send failed: {:?}", outcome.message());

    // The submission the server would have handed its MTA. A send that reached
    // the account and never reached the submission machinery is a message the
    // user believes went out.
    let (email_id, identity_id, _) = fixture.only_submission();
    assert_eq!(identity_id, identity);

    // And the copy: out of the mailbox it waited in, into the one the account
    // keeps sent mail in, and no longer a draft.
    assert!(fixture.listing(&drafts).is_empty(), "left behind in Drafts");
    let row = only(fixture.listing(&sent));
    assert_eq!(row.uid, email_id);
    assert!(!row.flags.draft, "still a draft after being sent");

    // So Evolution must not save a second copy.
    assert!(
        outcome.saved,
        "the account's own sent copy was not reported"
    );
}

#[test]
fn the_addresses_camel_hands_over_are_the_envelope_that_goes_out() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Drafts", Some("drafts"));
    fixture.seed_mailbox("Sent", Some("sent"));
    fixture.seed_identity("Alice", "alice@lists.example.com");
    let transport = fixture.transport();

    // Neither address is in the message's headers — the `Bcc` case, where the
    // recipient is in the envelope and in no header. A transport that let the
    // server derive the envelope from the headers would deliver to Bob, whom
    // the user did not address, and not to Carol, whom they did.
    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("", "alice@lists.example.com")]),
        &Addresses::of(&[("Carol", "carol@example.net"), ("", "dave@example.org")]),
    );
    assert!(outcome.sent, "the send failed: {:?}", outcome.message());

    let (_, _, envelope) = fixture.only_submission();
    assert_eq!(envelope.mail_from.email, "alice@lists.example.com");
    let rcpt_to: Vec<&str> = envelope
        .rcpt_to
        .iter()
        .map(|address| address.email.as_str())
        .collect();
    assert_eq!(rcpt_to, ["carol@example.net", "dave@example.org"]);
}

/// The identity is looked up by the *envelope* sender rather than by the `From`
/// header, which is what the account is sending as. Here the two disagree on
/// purpose: only the envelope address has an identity, so a transport that read
/// the header would find none and refuse a send that is perfectly ordinary.
#[test]
fn the_identity_is_the_one_the_envelope_sender_names() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Drafts", Some("drafts"));
    fixture.seed_mailbox("Sent", Some("sent"));
    let identity = fixture.seed_identity("Alice at work", "alice@work.example.com");
    let transport = fixture.transport();

    let outcome = send(
        &transport,
        // The message says `From: Alice <alice@example.com>`.
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@work.example.com")]),
        &Addresses::of(&[("Bob", "bob@example.com")]),
    );
    assert!(outcome.sent, "the send failed: {:?}", outcome.message());

    let (_, identity_id, _) = fixture.only_submission();
    assert_eq!(identity_id, identity);
}

/// The case `out_sent_message_saved` exists for and the one where it differs
/// from "did the message have to be moved": the account stages in Sent because
/// it has no Drafts, so nothing is filed anywhere afterwards and the copy is
/// nevertheless saved exactly where the user looks for sent mail.
#[test]
fn an_account_whose_only_outgoing_mailbox_is_sent_has_already_saved_the_copy() {
    let fixture = Fixture::start();
    let sent = fixture.seed_mailbox("Sent", Some("sent"));
    fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[("Bob", "bob@example.com")]),
    );
    assert!(outcome.sent, "the send failed: {:?}", outcome.message());

    let row = only(fixture.listing(&sent));
    assert!(!row.flags.draft, "still a draft after being sent");
    assert!(
        outcome.saved,
        "the copy is in Sent and Evolution was told to save another"
    );
}

/// And the other way: an account with a Drafts and no Sent keeps the message in
/// Drafts, which is not where sent mail belongs, so Evolution is told to save
/// its own copy.
#[test]
fn an_account_with_no_sent_mailbox_leaves_the_copy_to_evolution() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[("Bob", "bob@example.com")]),
    );
    assert!(outcome.sent, "the send failed: {:?}", outcome.message());

    // The message did go out, and the copy stayed where it was staged.
    assert!(!fixture.outbox_is_empty(), "nothing was submitted");
    let row = only(fixture.listing(&drafts));
    assert!(!row.flags.draft, "still a draft after being sent");
    assert!(
        !outcome.saved,
        "the sent copy is in Drafts and Evolution was told not to save one"
    );
}

// ---------------------------------------------------------------------------
// what is refused, and what it costs

#[test]
fn a_message_with_no_recipients_is_refused_before_anything_is_uploaded() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    fixture.seed_mailbox("Sent", Some("sent"));
    fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[]),
    );

    assert!(!outcome.sent, "a message with no recipients was sent");
    // The account is not broken and retrying cannot help, so deliberately not
    // `UNAVAILABLE`, which is what Evolution reads to put an account offline.
    assert_eq!(
        outcome.failure(),
        Some((service_domain(), CAMEL_SERVICE_ERROR_INVALID as i32))
    );
    assert!(fixture.outbox_is_empty());
    assert!(
        fixture.listing(&drafts).is_empty(),
        "a refused send left a draft behind"
    );
}

/// The recipient the user typed a name for and no address. Refusing is the
/// point: the alternative is a shorter `rcptTo`, which is a perfectly valid
/// submission and a message the user believes reached somebody who never got
/// it.
#[test]
fn a_recipient_with_no_address_is_named_in_the_refusal() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Drafts", Some("drafts"));
    fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[("Bob", "bob@example.com"), ("Carol", "")]),
    );

    assert!(
        !outcome.sent,
        "a message with an unusable recipient was sent"
    );
    let message = outcome.message().unwrap_or_default();
    assert!(
        message.contains("Carol"),
        "the refusal does not name the recipient: {message}"
    );
    assert!(fixture.outbox_is_empty());
}

#[test]
fn a_transport_that_is_not_connected_sends_nothing() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Drafts", Some("drafts"));
    fixture.seed_identity("Alice", "alice@example.com");
    // Constructed the way Camel constructs one and never authenticated, which
    // is the state a transport is in until the user presses Send.
    let transport = Transport::open(&fixture.account);

    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[("Bob", "bob@example.com")]),
    );

    assert!(!outcome.sent);
    // `NOT_CONNECTED` rather than a generic failure: it is what makes Camel
    // connect the service and ask again instead of showing the account as
    // broken.
    assert_eq!(
        outcome.failure(),
        Some((service_domain(), CAMEL_SERVICE_ERROR_NOT_CONNECTED as i32))
    );
    assert!(fixture.outbox_is_empty());
}

#[test]
fn an_address_the_account_cannot_send_as_is_refused_before_the_upload() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    fixture.seed_mailbox("Sent", Some("sent"));
    fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Mallory", "mallory@example.net")]),
        &Addresses::of(&[("Bob", "bob@example.com")]),
    );

    assert!(!outcome.sent, "the account sent as an address it has not");
    assert_eq!(
        outcome.failure(),
        Some((service_domain(), CAMEL_SERVICE_ERROR_INVALID as i32))
    );
    let message = outcome.message().unwrap_or_default();
    assert!(
        message.contains("mallory@example.net"),
        "the refusal does not name the address: {message}"
    );
    // Before the upload, which is the one request whose body is the whole
    // message — and, more importantly, before the message is in the account at
    // all.
    assert!(
        fixture.listing(&drafts).is_empty(),
        "a refused send left a draft behind"
    );
    assert!(fixture.outbox_is_empty());
}

#[test]
fn an_account_with_nowhere_to_stage_a_message_is_refused_before_the_upload() {
    let fixture = Fixture::start();
    // An account with folders, none of which is for mail the user writes.
    let inbox = fixture.seed_mailbox("Inbox", Some("inbox"));
    fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let outcome = send(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[("Bob", "bob@example.com")]),
    );

    assert!(!outcome.sent);
    assert_eq!(
        outcome.failure(),
        Some((service_domain(), CAMEL_SERVICE_ERROR_INVALID as i32))
    );
    // Not the Inbox: that is where the *server* delivers, and importing the
    // user's own outgoing mail into it would manufacture arrivals they then
    // have to sort out.
    assert!(fixture.listing(&inbox).is_empty());
    assert!(fixture.outbox_is_empty());
}

// ---------------------------------------------------------------------------
// the out-parameter itself

/// Dispatches the slot itself rather than through
/// `camel_transport_send_to_sync`, with `saved` as whatever the caller's
/// variable happened to hold — which is the whole point: the wrapper clears the
/// out-parameter before it calls the vfunc, so nothing that goes through it can
/// tell a vfunc that writes the parameter from one that leaves it alone.
///
/// `out` decides whether the parameter is passed at all; `saved` is what it
/// starts as when it is.
fn send_through_the_class(
    transport: &Transport,
    message: &Message,
    from: &Addresses,
    recipients: &Addresses,
    out: bool,
    saved: gboolean,
) -> (gboolean, gboolean) {
    let mut saved = saved;
    // SAFETY: a live GType of a class that has one; the vfunc is called with a
    // live transport of ours, a live message, two live address lists, an
    // out-parameter that is a local or NULL, and an error out-parameter that is
    // a local starting NULL. The class reference is given back.
    unsafe {
        let class = g_type_class_ref(transport_type()).cast::<CamelTransportClass>();
        let vfunc = (*class).send_to_sync.expect("the class installs one");
        let mut error: *mut GError = ptr::null_mut();
        let sent = vfunc(
            transport.service.cast::<CamelTransport>(),
            message.0,
            from.as_camel(),
            recipients.as_camel(),
            if out { &mut saved } else { ptr::null_mut() },
            ptr::null_mut(),
            &mut error,
        );
        g_clear_error(&mut error);
        g_type_class_unref(class.cast());
        (sent, saved)
    }
}

/// Camel declares `out_sent_message_saved` and its own transports leave it
/// alone; a caller may pass NULL.
#[test]
fn the_out_parameter_may_be_null() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Sent", Some("sent"));
    fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let (sent, _) = send_through_the_class(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[("Bob", "bob@example.com")]),
        false,
        GFALSE,
    );

    assert_ne!(sent, GFALSE, "a NULL out-parameter failed the send");
    assert!(!fixture.outbox_is_empty(), "nothing was submitted");
}

/// A send that fails leaves the out-parameter saying the copy is *not* saved,
/// whatever it said before.
///
/// `camel_transport_send_to_sync` clears it on the way in, so this can only be
/// checked by dispatching the slot — but it is not a hypothetical: a vfunc that
/// writes the parameter only on success is one whose answer, for every caller
/// that does not clear it, is whatever was on their stack. A stale `TRUE` there
/// is a failed send after which Evolution keeps no copy of the message the user
/// wrote.
#[test]
fn a_failed_send_says_no_copy_was_saved() {
    let fixture = Fixture::start();
    // An account that could send, so that the refusal is about the addresses
    // and the parameter is reached on a path with a real failure on it.
    fixture.seed_mailbox("Sent", Some("sent"));
    fixture.seed_identity("Alice", "alice@example.com");
    let transport = fixture.transport();

    let (sent, saved) = send_through_the_class(
        &transport,
        &Message::parsed(MESSAGE),
        &Addresses::of(&[("Alice", "alice@example.com")]),
        &Addresses::of(&[]),
        true,
        GTRUE,
    );

    assert_eq!(sent, GFALSE, "a message with no recipients was sent");
    assert_eq!(
        saved, GFALSE,
        "a failed send left the caller believing the copy was saved"
    );
    assert!(fixture.outbox_is_empty());
}
