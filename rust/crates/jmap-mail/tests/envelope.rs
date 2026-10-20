// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The SMTP envelope, read from the two `CamelAddress` arguments that a
//! transport's `send_to_sync` receives.
//!
//! These are the addresses the mail is actually *delivered* to, not the same
//! list the headers show. A `Bcc` recipient is in the envelope and in no
//! header: that is the whole point of a `Bcc`. Every test here builds the
//! arguments the way Evolution does, from real `CamelInternetAddress` objects,
//! and asserts on what the envelope contains, not on anything the message
//! itself says.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    CAMEL_SERVICE_ERROR_INVALID, CamelAddress, camel_address_new, camel_internet_address_add,
    camel_internet_address_new, camel_service_error_quark,
};
use gobject_sys::g_object_unref;
use jmap_mail::envelope::{EnvelopeError, read_envelope};
use jmap_proto::mail::EnvelopeAddress;

/// Builds a `CamelInternetAddress` holding `entries`, as the `CamelAddress *`
/// pointer Camel passes. A `None` name becomes the NULL Camel allows for an
/// address with no display name.
fn internet(entries: &[(Option<&str>, &str)]) -> *mut CamelAddress {
    // SAFETY: the address object is fresh. Every string passed in is
    // NUL-terminated, stays alive for the call, and Camel copies it.
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
    // from `camel_address_new`.
    unsafe { g_object_unref(address.cast()) };
}

/// An ordinary send: one sender, several recipients, nothing unusual about
/// any of them.
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

/// The envelope holds addresses, not addresses with names. RFC 5321's `RCPT
/// TO` takes an addr-spec, and RFC 8621 §7's `EnvelopeAddress` has no field for
/// a display name. The display name stays in the message headers, sent
/// unchanged.
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

/// A message with no recipients does not go out with an empty envelope. SMTP
/// has no transaction without a `RCPT TO`. A submission with an empty
/// `rcptTo` is one the server refuses only after it receives the whole
/// message.
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

/// No sender either. The transport does not fall back to the message's `From`
/// header. Which identity a message goes out as is the caller's decision, not
/// the transport's. A transport that reads the sender from the headers would
/// be choosing who the mail is from.
#[test]
fn a_message_with_no_sender_is_refused() {
    let empty = internet(&[]);
    let recipients = internet(&[(None, "first@example.com")]);

    // SAFETY: two live internet addresses.
    assert_eq!(
        unsafe { read_envelope(empty, recipients) },
        Err(EnvelopeError::NoSender)
    );
    // A NULL `from` gives the same answer, not a different failure: absent is
    // absent, however Camel represents it.
    // SAFETY: NULL is allowed by `read_envelope`'s contract.
    assert_eq!(
        unsafe { read_envelope(ptr::null_mut(), recipients) },
        Err(EnvelopeError::NoSender)
    );

    release(empty);
    release(recipients);
}

/// A sender with a display name but no address is no sender at all.
/// `MAIL FROM:<>` is the null reverse-path used for a bounce, not for a user's
/// message.
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

/// The same rule applies to a recipient, and the failure must not be silent.
/// A recipient dropped from the envelope lets the user believe they sent the
/// message to someone who never received it.
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

/// SMTP's reverse-path is one address. Every Camel transport reads entry zero,
/// and every caller in Evolution passes exactly one sender. This behavior
/// comes from Camel, not from a policy here, but the test still pins it: the
/// alternative to taking the first sender is refusing a send that Evolution
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

/// `send_to_sync` is declared over `CamelAddress`, which has other subclasses.
/// Reading one that is not a `CamelInternetAddress` through
/// `camel_internet_address_get` causes undefined behavior, not an empty
/// answer. So the code checks the type before it checks for emptiness.
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

/// Two recipients with the same address stay two entries. Whether a duplicate
/// `RCPT TO` delivers twice is the server's rule to apply. A transport that
/// deduplicated the list would quietly edit what the user was shown.
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

/// A NULL recipient list means no recipients, for the same reason a NULL
/// `from` means no sender: absent is absent.
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

/// Quoted display names can contain commas, semicolons, and brackets. Email
/// addresses can carry a subaddress tag. Extraction must not leak any display
/// name artifact into the `EnvelopeAddress`.
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

/// Every error variant's displayed sentence names the actual problem. This
/// includes the 1-based position that `UnusableRecipient` reports for an entry
/// with no display name. Camel's `index` field is 0-based, and the sentence
/// must convert it, not repeat it unchanged or apply the wrong arithmetic.
#[test]
fn the_display_text_names_the_failure() {
    assert!(
        EnvelopeError::NotInternet("sender")
            .to_string()
            .contains("sender")
    );
    assert!(
        EnvelopeError::NotInternet("recipients")
            .to_string()
            .contains("recipients")
    );
    assert!(!EnvelopeError::NoSender.to_string().is_empty());
    assert!(!EnvelopeError::NoRecipients.to_string().is_empty());
    assert!(
        EnvelopeError::UnusableRecipient {
            index: 0,
            name: Some("Nobody In Particular".to_owned()),
        }
        .to_string()
        .contains("Nobody In Particular")
    );

    // A nameless entry at Camel's index 2 must be reported as recipient 3 (the
    // 1-based position): `index + 1`, not `index * 1` (which also gives 2) or
    // any other formula that happens to agree at index 0 or 1.
    let message = EnvelopeError::UnusableRecipient {
        index: 2,
        name: None,
    }
    .to_string();
    assert!(
        message.contains('3'),
        "expected the 1-based position 3 in {message:?}"
    );
}

/// `to_gerror` must build a real `GError` in the transport's own domain, not
/// return a null pointer or an empty struct.
#[test]
fn to_gerror_builds_a_real_camel_service_error() {
    let error = EnvelopeError::NoSender;
    let gerror = error.to_gerror();
    assert!(!gerror.is_null());

    // SAFETY: `to_gerror` returns an owned `GError`, freed below.
    unsafe {
        assert_eq!((*gerror).domain, camel_service_error_quark());
        assert_eq!((*gerror).code, CAMEL_SERVICE_ERROR_INVALID as i32);
        assert!(!(*gerror).message.is_null());
        glib_sys::g_error_free(gerror);
    }
}
