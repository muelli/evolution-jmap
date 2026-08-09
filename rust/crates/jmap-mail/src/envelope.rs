// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The SMTP envelope, out of the addresses Camel hands a transport.
//!
//! `CamelTransportClass`'s `send_to_sync` is given the message and, separately,
//! two `CamelAddress` objects: who it is from and who it goes to. Those two are
//! the SMTP envelope — the `MAIL FROM` and the `RCPT TO`s of the transaction —
//! and RFC 8621 §7 has an `EmailSubmission` carry exactly the same pair as its
//! `envelope` property. This module is that translation, and nothing else: no
//! request, no connection, no GObject of ours.
//!
//! ## Why the envelope is carried at all
//!
//! RFC 8621 §7 lets the submission omit `envelope` and have the server derive
//! one from the message's own headers. That is right for a message the client
//! composed out of JMAP properties and wrong for one it was handed, because the
//! two lists genuinely differ: a `Bcc` recipient is a recipient with no header,
//! and a server deriving the envelope from headers would either not deliver to
//! them or — if the client left the `Bcc` header in — announce them to
//! everybody else. Camel gives the transport the recipients as their own
//! argument precisely because they are their own thing, and
//! [`Outgoing::envelope`] carries them the whole way down.
//!
//! [`Outgoing::envelope`]: jmap_mail_sync::Outgoing::envelope
//!
//! ## What is dropped, and what is not
//!
//! The display names are dropped, because the envelope has nowhere to put one:
//! RFC 5321's `RCPT TO` takes an addr-spec and RFC 8621 §7's `EnvelopeAddress`
//! has one field. The names are in the message's headers, which are uploaded
//! verbatim.
//!
//! Nothing else is. The list is not deduplicated — whether a repeated
//! `RCPT TO` delivers twice is the server's rule, and a transport that edited
//! the list would be quietly changing who the user addressed — and it is not
//! reordered, filtered or completed from the headers.
//!
//! ## Refusing, rather than sending less
//!
//! Every failure here is a refusal to send at all, and that is the point: the
//! alternative to refusing an address that has no addr-spec is dropping it, and
//! a dropped recipient is a message the user believes they sent to somebody who
//! never received it. Nothing below this point can notice the difference, since
//! a submission with a shorter `rcptTo` is a perfectly valid submission.
//!
//! The refusals are also free. They happen before the message is uploaded,
//! which is the one request whose body is the whole message.

use std::fmt;

use eds_sys::{
    CAMEL_SERVICE_ERROR_INVALID, CamelAddress, CamelInternetAddress, camel_address_length,
    camel_internet_address_get, camel_internet_address_get_type, camel_service_error_quark,
};
use glib_sys::{GError, GFALSE, g_error_new_literal, gchar};
use gobject_sys::g_type_check_instance_is_a;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::read_string;
use jmap_proto::mail::{Envelope, EnvelopeAddress};

/// A pair of addresses that does not describe a message that can be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    /// One of the two arguments is a `CamelAddress` that is not a
    /// `CamelInternetAddress`. The field names which — `"sender"` or
    /// `"recipients"` — since that is the whole of what a reader can do about
    /// it.
    ///
    /// Camel's own transports cast without checking, and in practice every
    /// caller in Evolution passes internet addresses. It is checked anyway
    /// because the alternative to a refusal is not a wrong answer: reading a
    /// `CamelAddress` of some other subclass through
    /// `camel_internet_address_get` is undefined behaviour, and the vfunc's
    /// signature is what lets a caller pass one.
    NotInternet(&'static str),
    /// There is no address to put in `MAIL FROM`.
    ///
    /// Which covers a NULL argument, an empty one, and one whose first entry
    /// carries a display name and no addr-spec — `MAIL FROM:<>` is the null
    /// reverse-path a bounce is sent with, not a user's message.
    NoSender,
    /// There is nobody to deliver to. A NULL argument or an empty one; an
    /// unusable *entry* is [`Self::UnusableRecipient`], because that one can
    /// say which.
    NoRecipients,
    /// One recipient has no addr-spec, named by the position Camel listed it at
    /// and by its display name if it has one.
    UnusableRecipient { index: usize, name: Option<String> },
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInternet(which) => {
                write!(f, "the {which} are not internet addresses")
            }
            Self::NoSender => f.write_str("the message has no sender address"),
            Self::NoRecipients => f.write_str("the message has no recipients"),
            Self::UnusableRecipient { index, name } => match name {
                Some(name) => write!(f, "no address for the recipient \"{name}\""),
                None => write!(f, "no address for recipient {}", index + 1),
            },
        }
    }
}

impl std::error::Error for EnvelopeError {}

impl EnvelopeError {
    /// Allocates a `GError` describing this refusal. Ownership passes to the
    /// caller, exactly as with [`StoreError::to_gerror`].
    ///
    /// `CAMEL_SERVICE_ERROR_INVALID`, in the transport's own domain — a
    /// `CamelTransport` is a `CamelService`. Deliberately not
    /// `CAMEL_SERVICE_ERROR_UNAVAILABLE`, which is what Evolution reads to put
    /// an account offline: nothing is wrong with the account or the connection,
    /// and this send would fail the same way against a working server. What the
    /// user needs is the sentence, which says which address is missing.
    ///
    /// [`StoreError::to_gerror`]: crate::connect::StoreError::to_gerror
    pub fn to_gerror(&self) -> *mut GError {
        let message = cstring_lossy(&self.to_string());

        // SAFETY: a live quark, a code from that domain's own enum, and a
        // NUL-terminated message the call copies.
        unsafe {
            g_error_new_literal(
                camel_service_error_quark(),
                CAMEL_SERVICE_ERROR_INVALID as i32,
                message.as_ptr(),
            )
        }
    }
}

/// The envelope `send_to_sync`'s two address arguments describe.
///
/// # Safety
///
/// `from` and `recipients` must each be NULL or point at a live `CamelAddress`.
/// They may be the same object, and neither is modified or unreffed.
pub unsafe fn read_envelope(
    from: *mut CamelAddress,
    recipients: *mut CamelAddress,
) -> Result<Envelope, EnvelopeError> {
    // SAFETY: the contract above.
    let sender = unsafe { internet(from, "sender") }?;
    // SAFETY: as above.
    let recipients = unsafe { internet(recipients, "recipients") }?;

    // Entry zero, which is what every Camel transport reads: SMTP's
    // reverse-path is one address, and every caller in Evolution passes exactly
    // one. Refusing a list of two would be refusing a send Evolution cannot
    // produce, in the name of a rule SMTP applies to the transaction rather
    // than to the client.
    //
    // SAFETY: `sender` is NULL or a live internet address, by `internet`.
    let mail_from = unsafe { entry(sender, 0) }
        .and_then(|(_, email)| email)
        .ok_or(EnvelopeError::NoSender)?;

    // SAFETY: as above; `camel_address_length` is declared on the parent.
    let count = unsafe { length(recipients) };
    if count == 0 {
        return Err(EnvelopeError::NoRecipients);
    }

    let mut rcpt_to = Vec::with_capacity(count);
    for index in 0..count {
        // A read that fails inside the length Camel just reported is a list
        // that changed under us, which is not a case to paper over with a
        // shorter envelope: it lands in the same refusal an entry with no
        // address does.
        //
        // SAFETY: as above, and `index` is below the reported length.
        let (name, email) = unsafe { entry(recipients, index as i32) }.unwrap_or((None, None));
        let Some(email) = email else {
            return Err(EnvelopeError::UnusableRecipient { index, name });
        };
        rcpt_to.push(EnvelopeAddress::new(email));
    }

    Ok(Envelope {
        mail_from: EnvelopeAddress::new(mail_from),
        rcpt_to,
    })
}

/// `address` as the internet address it claims to be, or `None` for a NULL —
/// which every caller here treats as the empty list, because absent is absent
/// however Camel spells it.
///
/// # Safety
///
/// `address` must be NULL or point at a live `CamelAddress`.
unsafe fn internet(
    address: *mut CamelAddress,
    which: &'static str,
) -> Result<Option<*mut CamelInternetAddress>, EnvelopeError> {
    if address.is_null() {
        return Ok(None);
    }
    // SAFETY: the contract above — a live GObject instance, and a type that
    // registers itself on first use.
    let is_internet = unsafe {
        g_type_check_instance_is_a(address.cast(), camel_internet_address_get_type()) != GFALSE
    };
    match is_internet {
        true => Ok(Some(address.cast())),
        false => Err(EnvelopeError::NotInternet(which)),
    }
}

/// How many addresses the list holds; a NULL list holds none.
///
/// # Safety
///
/// `address` must be NULL or point at a live `CamelInternetAddress`.
unsafe fn length(address: Option<*mut CamelInternetAddress>) -> usize {
    let Some(address) = address else {
        return 0;
    };
    // SAFETY: the contract above; `CamelInternetAddress` derives from
    // `CamelAddress`, which is what the accessor takes. A negative length is
    // what Camel returns for an argument that failed its own type check, which
    // `internet` has already ruled out.
    unsafe { camel_address_length(address.cast()) }.max(0) as usize
}

/// One entry of the list: its display name and its addr-spec, each `None` when
/// Camel has nothing there. `None` for an index the list does not have.
///
/// Both strings go through [`read_string`], so an empty one is `None` — which
/// is the answer that matters: a display name with no address behind it is not
/// an address, and treating `""` as one would put `RCPT TO:<>` on the wire.
///
/// # Safety
///
/// `address` must be NULL or point at a live `CamelInternetAddress`.
unsafe fn entry(
    address: Option<*mut CamelInternetAddress>,
    index: i32,
) -> Option<(Option<String>, Option<String>)> {
    let address = address?;

    let mut name: *const gchar = std::ptr::null();
    let mut email: *const gchar = std::ptr::null();
    // SAFETY: the contract above, and two out-parameters that are locals. The
    // strings that come back belong to the address object and outlive the call,
    // which is what `read_string` needs; it copies them.
    unsafe {
        if camel_internet_address_get(address, index, &mut name, &mut email) == GFALSE {
            return None;
        }
        Some((read_string(name), read_string(email)))
    }
}
