// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Builds the SMTP envelope from the addresses Camel gives a transport.
//!
//! `send_to_sync` gets two `CamelAddress` objects next to the message: the
//! `MAIL FROM` and the `RCPT TO` list. RFC 8621 §7 puts the same pair in the
//! `envelope` property of an `EmailSubmission`.
//!
//! The envelope is always sent. A server that derives it from the headers
//! cannot see `Bcc` recipients, so it does not deliver to them or it shows
//! them to all recipients. [`Outgoing::envelope`] carries the list to the
//! request.
//!
//! [`Outgoing::envelope`]: jmap_mail_sync::Outgoing::envelope
//!
//! Display names are dropped, because RFC 8621 §7's `EnvelopeAddress` holds
//! only an addr-spec. The uploaded headers keep the names. The list is not
//! deduplicated, reordered, filtered or completed from the headers.
//!
//! An address with no addr-spec stops the send. If the code dropped it, the
//! user would think the message went to a recipient who never got it. This
//! check occurs before the upload, so it costs no request.

use std::fmt;

use eds_sys::{
    CAMEL_SERVICE_ERROR_INVALID, CamelAddress, CamelInternetAddress, camel_address_length,
    camel_internet_address_get, camel_internet_address_get_type, camel_service_error_quark,
};
use glib_sys::{GError, GFALSE, g_error_new_literal, gchar};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::marshal::{checked_borrow_ptr_or, read_string};
use jmap_proto::mail::{Envelope, EnvelopeAddress};

/// A pair of addresses that does not describe a message that can be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    /// An argument is a `CamelAddress` that is not a `CamelInternetAddress`.
    /// The field is `"sender"` or `"recipients"`.
    ///
    /// Evolution always passes internet addresses, but the vfunc signature
    /// accepts any `CamelAddress`. Reading another subclass through
    /// `camel_internet_address_get` is undefined behaviour.
    NotInternet(&'static str),
    /// There is no address for `MAIL FROM`: the argument is NULL or empty, or
    /// its first entry has no addr-spec.
    ///
    /// `MAIL FROM:<>` is the null reverse-path of a bounce, not of a user's
    /// message.
    NoSender,
    /// The recipients argument is NULL or empty. An unusable entry is
    /// [`Self::UnusableRecipient`].
    NoRecipients,
    /// One recipient has no addr-spec. The error holds its position in the
    /// Camel list and its display name, if it has one.
    UnusableRecipient { index: usize, name: Option<String> },
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInternet(which) => {
                // The callers of `internet()` pass an untranslated literal.
                // The translation occurs here, so a new caller cannot forget it.
                let which = match *which {
                    "sender" => translate(c"sender"),
                    "recipients" => translate(c"recipients"),
                    other => other.to_owned(),
                };
                f.write_str(&translate_with(
                    // TRANSLATORS: %1$s is "sender" or "recipients", already
                    // translated.
                    c"the %1$s are not internet addresses",
                    &[which.as_str()],
                ))
            }
            Self::NoSender => f.write_str(&translate(c"the message has no sender address")),
            Self::NoRecipients => f.write_str(&translate(c"the message has no recipients")),
            Self::UnusableRecipient { index, name } => match name {
                Some(name) => f.write_str(&translate_with(
                    // TRANSLATORS: %1$s is the recipient's display name.
                    c"no address for the recipient \"%1$s\"",
                    &[name.as_str()],
                )),
                None => f.write_str(&translate_with(
                    // TRANSLATORS: %1$s is the recipient's 1-based position in
                    // the list. This text shows when the recipient has no
                    // display name.
                    c"no address for recipient %1$s",
                    &[(index + 1).to_string().as_str()],
                )),
            },
        }
    }
}

impl std::error::Error for EnvelopeError {}

impl EnvelopeError {
    /// Allocates a `GError` for this refusal. The caller owns it, as with
    /// [`StoreError::to_gerror`].
    ///
    /// The code is `CAMEL_SERVICE_ERROR_INVALID`, because a `CamelTransport` is a
    /// `CamelService`. It is not `CAMEL_SERVICE_ERROR_UNAVAILABLE`, because
    /// Evolution then puts the account offline, and the account has no fault.
    ///
    /// [`StoreError::to_gerror`]: crate::connect::StoreError::to_gerror
    pub fn to_gerror(&self) -> *mut GError {
        let message = cstring_lossy(&self.to_string());

        // SAFETY: a live quark, a code from the enum of that domain, and a
        // NUL-terminated message that the call copies.
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

    // Use entry zero, as all Camel transports do. The SMTP reverse-path is one
    // address, and Evolution always passes exactly one.
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
        // A failed read below the reported length means that the list changed.
        // Refuse it as an entry with no address, not with a shorter envelope.
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

/// Casts `address` to a `CamelInternetAddress`. A NULL gives `None`, which the
/// callers treat as an empty list.
///
/// # Safety
///
/// `address` must be NULL or point at a live `CamelAddress`.
unsafe fn internet(
    address: *mut CamelAddress,
    which: &'static str,
) -> Result<Option<*mut CamelInternetAddress>, EnvelopeError> {
    // SAFETY: the contract above gives a live GObject instance, and the type
    // registers itself on first use.
    unsafe {
        checked_borrow_ptr_or(
            address,
            camel_internet_address_get_type(),
            EnvelopeError::NotInternet(which),
        )
    }
}

/// The number of addresses in the list. A NULL list holds none.
///
/// # Safety
///
/// `address` must be NULL or point at a live `CamelInternetAddress`.
unsafe fn length(address: Option<*mut CamelInternetAddress>) -> usize {
    let Some(address) = address else {
        return 0;
    };
    // SAFETY: the contract above. `CamelInternetAddress` derives from
    // `CamelAddress`, which the accessor takes. Camel returns a negative length
    // only for an argument that fails its type check, and `internet` checked it.
    unsafe { camel_address_length(address.cast()) }.max(0) as usize
}

/// One entry of the list: its display name and its addr-spec. Each is `None`
/// when Camel has no value. The result is `None` for an index out of range.
///
/// [`read_string`] makes an empty string `None`. Thus an empty addr-spec cannot
/// put `RCPT TO:<>` on the wire.
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
    // returned strings belong to the address object and outlive the call.
    // `read_string` copies them.
    unsafe {
        if camel_internet_address_get(address, index, &mut name, &mut email) == GFALSE {
            return None;
        }
        Some((read_string(name), read_string(email)))
    }
}
