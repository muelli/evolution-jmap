// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The SMTP envelope, read out of the two `CamelAddress` arguments a
//! transport's `send_to_sync` is handed.
//!
//! These are the addresses the mail is actually *delivered* to, which is not
//! the same list as the headers say — a `Bcc` recipient is in the envelope and
//! in no header, which is the whole point of one. So every test here builds the
//! arguments the way Evolution does, out of real `CamelInternetAddress`
//! objects, and asserts on what comes out the other side rather than on
//! anything the message says.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    CamelAddress, camel_address_new, camel_internet_address_add, camel_internet_address_new,
};
use gobject_sys::g_object_unref;
use jmap_mail::envelope::{EnvelopeError, read_envelope};
use jmap_proto::mail::EnvelopeAddress;

/// A `CamelInternetAddress` holding `entries`, as the `CamelAddress *` Camel
/// passes. A `None` name is the NULL Camel allows for an address nobody put a
/// display name on.
fn internet(entries: &[(Option<&str>, &str)]) -> *mut CamelAddress {
    // SAFETY: a fresh address object, and every string handed over is
    // NUL-terminated, alive for the call, and copied by Camel.
    unsafe {
        let address = camel_internet_address_new();
        for (name, email) in entries {
            let name = name.map(|name| CString::new(name).expect("a name without a NUL"));
            let email = CString::new(*email).expect("an address without a NUL");
            camel_internet_address_add(
                address,
                name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
                email.as_ptr(),
            );
        }
        address.cast()
    }
}

fn release(address: *mut CamelAddress) {
    // SAFETY: the caller owns the only reference, taken from `internet` or
    // `camel_address_new`.
    unsafe { g_object_unref(address.cast()) };
}

/// The ordinary send: one sender, several recipients, and nothing unusual
/// about any of them.
#[test]
fn the_addresses_camel_was_handed_become_the_envelope() {
    let from = internet(&[(None, "sender@example.com")]);
    let recipients = internet(&[
        (None, "first@example.com"),
        (None, "second@example.net"),
        (None, "third@example.org"),
    ]);

    // SAFETY: two live internet addresses.
    let envelope = unsafe { read_envelope(from, recipients) }.expect("a sendable envelope");

    assert_eq!(
        envelope.mail_from,
        EnvelopeAddress::new("sender@example.com")
    );
    assert_eq!(
        envelope.rcpt_to,
        vec![
            EnvelopeAddress::new("first@example.com"),
            EnvelopeAddress::new("second@example.net"),
            EnvelopeAddress::new("third@example.org"),
        ],
        "the recipients must keep the order and the count Camel listed them in"
    );

    release(from);
    release(recipients);
}

/// The envelope is addresses, not addresses-with-names: RFC 5321's `RCPT TO`
/// takes an addr-spec, and RFC 8621 §7's `EnvelopeAddress` has nowhere to put a
/// display name. The name is in the message's headers, which go up verbatim.
#[test]
fn the_display_name_is_not_part_of_the_envelope() {
    let from = internet(&[(Some("A Sender"), "sender@example.com")]);
    let recipients = internet(&[(Some("Someone, Else"), "else@example.com")]);

    // SAFETY: two live internet addresses.
    let envelope = unsafe { read_envelope(from, recipients) }.expect("a sendable envelope");

    assert_eq!(
        envelope.mail_from,
        EnvelopeAddress::new("sender@example.com")
    );
    assert_eq!(
        envelope.rcpt_to,
        vec![EnvelopeAddress::new("else@example.com")]
    );

    release(from);
    release(recipients);
}

/// A message with nobody to deliver it to is not a message that goes out with
/// an empty envelope — there is no SMTP transaction without a `RCPT TO`, and a
/// submission with an empty `rcptTo` is one the server refuses after the whole
/// message has been uploaded.
#[test]
fn a_message_with_no_recipients_is_refused_before_the_upload() {
    let from = internet(&[(None, "sender@example.com")]);
    let recipients = internet(&[]);

    // SAFETY: two live internet addresses.
    let refused = unsafe { read_envelope(from, recipients) };

    assert_eq!(refused, Err(EnvelopeError::NoRecipients));

    release(from);
    release(recipients);
}

/// And no sender either. The transport does not fall back to the message's
/// `From` header: which identity a message goes out as is the caller's
/// decision, and a transport that read one out of the headers instead would be
/// choosing who the mail is from.
#[test]
fn a_message_with_no_sender_is_refused() {
    let empty = internet(&[]);
    let recipients = internet(&[(None, "first@example.com")]);

    // SAFETY: two live internet addresses.
    assert_eq!(
        unsafe { read_envelope(empty, recipients) },
        Err(EnvelopeError::NoSender)
    );
    // A NULL `from` is the same answer and not a different failure: absent is
    // absent, however Camel spells it.
    // SAFETY: NULL is allowed by `read_envelope`'s contract.
    assert_eq!(
        unsafe { read_envelope(ptr::null_mut(), recipients) },
        Err(EnvelopeError::NoSender)
    );

    release(empty);
    release(recipients);
}

/// A sender Camel listed with a display name and no address is no sender at
/// all — `MAIL FROM:<>` is the null reverse-path a bounce is sent with, not a
/// user's message.
#[test]
fn a_sender_with_no_address_is_no_sender() {
    let from = internet(&[(Some("A Sender"), "")]);
    let recipients = internet(&[(None, "first@example.com")]);

    // SAFETY: two live internet addresses.
    assert_eq!(
        unsafe { read_envelope(from, recipients) },
        Err(EnvelopeError::NoSender)
    );

    release(from);
    release(recipients);
}

/// The same on the other side, and this is the one that must not be silent: a
/// recipient dropped from the envelope is a message the user believes they sent
/// to somebody who never got it.
#[test]
fn a_recipient_with_no_address_is_refused_rather_than_dropped() {
    let from = internet(&[(None, "sender@example.com")]);
    let recipients = internet(&[
        (None, "first@example.com"),
        (Some("Nobody In Particular"), ""),
        (None, "third@example.org"),
    ]);

    // SAFETY: two live internet addresses.
    let refused = unsafe { read_envelope(from, recipients) };

    assert_eq!(
        refused,
        Err(EnvelopeError::UnusableRecipient {
            index: 1,
            name: Some("Nobody In Particular".to_owned()),
        }),
        "the failure must name the recipient it is about, and it must be a failure"
    );

    release(from);
    release(recipients);
}

/// SMTP's reverse-path is one address. Every Camel transport reads entry zero
/// and every caller in Evolution passes exactly one, so this is what the rest
/// of Camel does rather than a policy of ours — but it is worth pinning, since
/// the alternative to taking the first would be refusing a send Evolution
/// cannot produce.
#[test]
fn the_first_of_several_senders_is_the_reverse_path() {
    let from = internet(&[(None, "first@example.com"), (None, "second@example.com")]);
    let recipients = internet(&[(None, "to@example.com")]);

    // SAFETY: two live internet addresses.
    let envelope = unsafe { read_envelope(from, recipients) }.expect("a sendable envelope");

    assert_eq!(
        envelope.mail_from,
        EnvelopeAddress::new("first@example.com")
    );

    release(from);
    release(recipients);
}

/// `send_to_sync` is declared over `CamelAddress`, which has other subclasses;
/// reading one that is not a `CamelInternetAddress` through
/// `camel_internet_address_get` would be undefined behaviour rather than an
/// empty answer, so the type is checked before the emptiness is.
#[test]
fn an_address_that_is_not_an_internet_address_is_refused() {
    // SAFETY: no arguments, and the caller owns the reference.
    let plain = unsafe { camel_address_new() };
    let ordinary = internet(&[(None, "someone@example.com")]);

    // SAFETY: a live `CamelAddress` and a live internet address.
    assert_eq!(
        unsafe { read_envelope(plain, ordinary) },
        Err(EnvelopeError::NotInternet("sender")),
        "an empty address of the wrong type must be reported as the wrong type, \
         not as an absent sender"
    );
    // SAFETY: as above.
    assert_eq!(
        unsafe { read_envelope(ordinary, plain) },
        Err(EnvelopeError::NotInternet("recipients"))
    );

    release(plain);
    release(ordinary);
}

/// Two recipients that are the same address stay two entries. Whether a
/// duplicate `RCPT TO` delivers twice is the server's rule to apply, and a
/// transport that deduplicated would be quietly editing the list the user was
/// shown.
#[test]
fn a_repeated_recipient_is_not_deduplicated() {
    let from = internet(&[(None, "sender@example.com")]);
    let recipients = internet(&[
        (None, "same@example.com"),
        (Some("The Same Person"), "same@example.com"),
    ]);

    // SAFETY: two live internet addresses.
    let envelope = unsafe { read_envelope(from, recipients) }.expect("a sendable envelope");

    assert_eq!(
        envelope.rcpt_to,
        vec![
            EnvelopeAddress::new("same@example.com"),
            EnvelopeAddress::new("same@example.com"),
        ]
    );

    release(from);
    release(recipients);
}

/// A NULL recipient list is the empty one, for the reason a NULL `from` is the
/// empty sender: absent is absent.
#[test]
fn a_null_recipient_list_is_no_recipients() {
    let from = internet(&[(None, "sender@example.com")]);

    // SAFETY: NULL is allowed by `read_envelope`'s contract.
    assert_eq!(
        unsafe { read_envelope(from, ptr::null_mut()) },
        Err(EnvelopeError::NoRecipients)
    );

    release(from);
}

/// Quoted display names with commas, semicolons, brackets, and subaddressed emails
/// in the envelope must be extracted cleanly without leaking display name artifacts into
/// the EnvelopeAddress.
#[test]
fn the_envelope_preserves_special_characters_in_quoted_names_and_subaddresses() {
    let from = internet(&[(Some("Doe, Jane (Work)"), "jane.doe+tag@example.com")]);
    let recipients = internet(&[
        (Some("O'Connor, Liam"), "liam@example.com"),
        (
            Some("\"Special, Name; <Group>\""),
            "special.user@example.org",
        ),
        (Some("Müller, Tobias"), "muelli@example.net"),
    ]);

    // SAFETY: two live internet addresses.
    let envelope = unsafe { read_envelope(from, recipients) }.expect("a sendable envelope");

    assert_eq!(
        envelope.mail_from,
        EnvelopeAddress::new("jane.doe+tag@example.com")
    );
    assert_eq!(
        envelope.rcpt_to,
        vec![
            EnvelopeAddress::new("liam@example.com"),
            EnvelopeAddress::new("special.user@example.org"),
            EnvelopeAddress::new("muelli@example.net"),
        ],
        "subaddresses and exact email strings must be preserved without name leakage"
    );

    release(from);
    release(recipients);
}
