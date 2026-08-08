// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelMessageInfo`: one summary row, as the object Camel keeps it in.
//!
//! `jmap-mail-sync`'s [`MessageSummary`] is the same row read off an
//! `Email/get`, and most of what happens here is copying it across. Three
//! columns are not a copy, and they are what this module is for:
//!
//! - **The flags word.** Camel keeps a message's state as bits in one `guint32`
//!   where JMAP keeps it as a set of keywords. The sync layer has already
//!   sorted the keywords into the ones Camel has a bit for and the ones it does
//!   not; turning the first into the word is [`flags_word`], and the second
//!   become Camel's user flags, which is what Evolution shows as labels.
//! - **The address columns.** JMAP sends `from`, `to` and `cc` as structured
//!   name/address pairs; Camel's summary stores one display string per header.
//!   Formatting that string is `CamelInternetAddress`'s job — RFC 5322's
//!   quoting rules live there — so [`address_list`] builds one and asks it.
//! - **The threading columns.** Camel does not store a `Message-ID`; it stores
//!   64 bits of an MD5 over it, per `CamelSummaryMessageID`, and an array of
//!   the same for the ancestors. There is no public function to compute one,
//!   so [`message_id_digest`] computes it — and it computes exactly what
//!   Camel's own `camel_message_info_new_from_headers` would have, because
//!   those two paths meet: a message Camel parses locally lands in the same
//!   summary as a row built here, and digests that disagreed would thread one
//!   conversation as two. `tests/message_info.rs` pins that against Camel
//!   itself rather than against a constant.
//!
//! The row is built detached, with no summary behind it. That is not a
//! placeholder for the summary this folder will grow: `camel_message_info_new`
//! consults the summary only to learn which message-info type to instantiate,
//! and a summary that declares none — which is the case here — gets
//! `CamelMessageInfoBase`, the same class NULL produces.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CAMEL_MESSAGE_ANSWERED, CAMEL_MESSAGE_ATTACHMENTS, CAMEL_MESSAGE_DRAFT, CAMEL_MESSAGE_FLAGGED,
    CAMEL_MESSAGE_FORWARDED, CAMEL_MESSAGE_JUNK, CAMEL_MESSAGE_NOTJUNK, CAMEL_MESSAGE_SEEN,
    CamelAddress, CamelMessageFlags, CamelMessageInfo, camel_address_format,
    camel_internet_address_add, camel_internet_address_new,
    camel_message_info_freeze_notifications, camel_message_info_new, camel_message_info_set_cc,
    camel_message_info_set_date_received, camel_message_info_set_date_sent,
    camel_message_info_set_flags, camel_message_info_set_from, camel_message_info_set_message_id,
    camel_message_info_set_preview, camel_message_info_set_size, camel_message_info_set_subject,
    camel_message_info_set_to, camel_message_info_set_uid, camel_message_info_take_references,
    camel_message_info_take_user_flags, camel_message_info_thaw_notifications,
    camel_named_flags_insert, camel_named_flags_new,
};
use glib_sys::{
    G_CHECKSUM_MD5, GArray, GFALSE, g_array_append_vals, g_array_sized_new, g_checksum_free,
    g_checksum_get_digest, g_checksum_new, g_checksum_update, g_free,
};
use gobject_sys::g_object_unref;
use jmap_mail_sync::{MessageFlags, MessageSummary};
use jmap_proto::mail::EmailAddress;

use crate::folder_info::c_string;

/// The summary row for one message, owned by the caller.
///
/// Never NULL in practice — `camel_message_info_new` is a `g_object_new` on a
/// concrete type — but the pointer is returned as Camel's own functions return
/// it rather than unwrapped into a reference, because its only caller hands it
/// straight back to a summary.
pub fn new_message_info(message: &MessageSummary) -> *mut CamelMessageInfo {
    // SAFETY: NULL is a summary that declares no message-info type, which is
    // what this provider's will be; the row comes out a `CamelMessageInfoBase`.
    let info = unsafe { camel_message_info_new(ptr::null_mut()) };
    if info.is_null() {
        return info;
    }

    // Every setter below emits a property notification and marks the row
    // dirty. Camel's own builders freeze first for that reason: a row filled
    // column by column while a summary watched would be a dozen changes to a
    // message that has not been listed yet.
    //
    // SAFETY: `info` is a fresh info this function owns; every string passed
    // is NUL-terminated and copied by the setter, and the references array is
    // handed over to `take_references` along with its ownership.
    unsafe {
        camel_message_info_freeze_notifications(info);

        camel_message_info_set_uid(info, c_string(message.uid.as_str()).as_ptr());
        update_message_info(info, message);

        set_string(
            info,
            camel_message_info_set_subject,
            message.subject.as_deref(),
        );
        set_string(
            info,
            camel_message_info_set_from,
            address_list(&message.from).as_deref(),
        );
        set_string(
            info,
            camel_message_info_set_to,
            address_list(&message.to).as_deref(),
        );
        set_string(
            info,
            camel_message_info_set_cc,
            address_list(&message.cc).as_deref(),
        );
        set_string(
            info,
            camel_message_info_set_preview,
            message.preview.as_deref(),
        );

        camel_message_info_set_size(info, message.size);
        camel_message_info_set_date_received(info, message.received_at.unwrap_or(0));
        camel_message_info_set_date_sent(info, message.sent_at.unwrap_or(0));

        if let Some(id) = &message.message_id {
            camel_message_info_set_message_id(info, message_id_digest(id));
        }
        let ancestors = references_array(&message.references);
        if !ancestors.is_null() {
            camel_message_info_take_references(info, ancestors);
        }

        camel_message_info_thaw_notifications(info);
    }

    info
}

/// Writes the columns a later listing of the same message may have changed.
///
/// Every property of a JMAP `Email` is immutable except `keywords` and
/// `mailboxIds` (RFC 8621 §4.1) — a message does not get a new subject, a new
/// sender or a new `Message-ID` — and the mailbox it is in is not a column of
/// the row, it is which folder the row is in. So a refresh is these two
/// columns and nothing else, which is both what the server can honestly be
/// said to have changed and what [`crate::summary`] rewrites when it meets a
/// row that is already there. Everything else is written once, by
/// [`new_message_info`], which calls this for the rest of the row.
///
/// # Safety
///
/// `info` must be a live message info.
pub unsafe fn update_message_info(info: *mut CamelMessageInfo, message: &MessageSummary) {
    // SAFETY: the caller guarantees the info; the mask keeps the setter off the
    // bits this provider is not the authority on, and the flag set below is
    // handed over along with its ownership.
    unsafe {
        camel_message_info_freeze_notifications(info);
        camel_message_info_set_flags(info, FLAGS_FROM_JMAP, flags_word(&message.flags));
        set_user_flags(info, &message.tags);
        camel_message_info_thaw_notifications(info);
    }
}

/// Replaces the row's user flags — Evolution's labels — with the ones the
/// listing carries.
///
/// Wholesale rather than name by name, because the keyword set is the whole
/// truth about a message's labels: a keyword the server has stopped sending is
/// one that was taken off somewhere else, and Camel is told about it only by
/// its absence. Handing over an empty set is therefore right rather than
/// merely harmless — unlike the text columns, where an empty value and an
/// absent one are different things in the summary database, user flags are
/// stored as one joined string with no way to spell "absent".
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn set_user_flags(info: *mut CamelMessageInfo, tags: &[String]) {
    // SAFETY: the set is built here and handed to `take_user_flags`, which
    // consumes it whether or not it turned out to be a change; every name is a
    // NUL-terminated string the set copies.
    unsafe {
        let flags = camel_named_flags_new();
        for tag in tags {
            camel_named_flags_insert(flags, c_string(tag).as_ptr());
        }
        camel_message_info_take_user_flags(info, flags);
    }
}

/// Sets one text column, or leaves it unset when the message did not carry it.
///
/// The distinction is not cosmetic: Camel writes the summary to a database and
/// reads it back, where an empty string is a header that was present and blank
/// rather than one that was absent.
///
/// # Safety
///
/// `info` must be a live message info, and `setter` one of Camel's own column
/// setters for it.
unsafe fn set_string(
    info: *mut CamelMessageInfo,
    setter: unsafe extern "C" fn(
        *mut CamelMessageInfo,
        *const glib_sys::gchar,
    ) -> glib_sys::gboolean,
    value: Option<&str>,
) {
    let Some(value) = value else { return };
    // SAFETY: the copy lives across the call, and the setter copies the string.
    unsafe { setter(info, c_string(value).as_ptr()) };
}

/// The bits of the flags word this provider is the authority on.
///
/// Passed as the mask to `camel_message_info_set_flags` so that a row being
/// refreshed keeps everything else Camel put in the word — `DELETED` and
/// `FOLDER_FLAGGED` are local marks the user made, and clearing them because
/// the server said nothing about them would undo a deletion the user is
/// waiting to have expunged.
const FLAGS_FROM_JMAP: CamelMessageFlags = CAMEL_MESSAGE_SEEN
    | CAMEL_MESSAGE_ANSWERED
    | CAMEL_MESSAGE_FLAGGED
    | CAMEL_MESSAGE_DRAFT
    | CAMEL_MESSAGE_FORWARDED
    | CAMEL_MESSAGE_JUNK
    | CAMEL_MESSAGE_NOTJUNK
    | CAMEL_MESSAGE_ATTACHMENTS;

/// The keywords Camel has a bit for, as that word.
///
/// One arm per field of [`MessageFlags`], which is one arm per bit the sync
/// layer can honestly speak to. Everything else in the word is Camel's:
/// `DELETED` because JMAP has no deleted keyword, `SECURE` and `ANSWERED_ALL`
/// because they are conclusions drawn from a message that has been fetched.
fn flags_word(flags: &MessageFlags) -> CamelMessageFlags {
    let mut word = 0;
    for (set, bit) in [
        (flags.seen, CAMEL_MESSAGE_SEEN),
        (flags.answered, CAMEL_MESSAGE_ANSWERED),
        (flags.flagged, CAMEL_MESSAGE_FLAGGED),
        (flags.draft, CAMEL_MESSAGE_DRAFT),
        (flags.forwarded, CAMEL_MESSAGE_FORWARDED),
        (flags.junk, CAMEL_MESSAGE_JUNK),
        (flags.not_junk, CAMEL_MESSAGE_NOTJUNK),
        (flags.attachments, CAMEL_MESSAGE_ATTACHMENTS),
    ] {
        if set {
            word |= bit;
        }
    }
    word
}

/// One address header, in the single string Camel's summary keeps it as.
///
/// Built through `CamelInternetAddress` rather than by joining the parts here:
/// a display name may hold a comma, a quote or a backslash, and the rules for
/// which of those have to be quoted are RFC 5322's, already implemented once in
/// Camel. `None` for a header the message did not carry, which is not the same
/// as one Camel would store as empty.
fn address_list(addresses: &[EmailAddress]) -> Option<String> {
    if addresses.is_empty() {
        return None;
    }

    // SAFETY: the address object is constructed, filled and unreffed here and
    // reaches nothing else; every string handed over is NUL-terminated and
    // copied, and `camel_address_format` returns a `g_malloc`ed string this
    // function frees.
    unsafe {
        let address = camel_internet_address_new();
        for entry in addresses {
            let name = entry.name.as_deref().map(c_string);
            let email = c_string(&entry.email);
            camel_internet_address_add(
                address,
                name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
                email.as_ptr(),
            );
        }

        let formatted = camel_address_format(address.cast::<CamelAddress>());
        g_object_unref(address.cast());
        if formatted.is_null() {
            return None;
        }
        let text = CStr::from_ptr(formatted).to_string_lossy().into_owned();
        g_free(formatted.cast());
        Some(text)
    }
}

/// The 64 bits Camel stores instead of a `Message-ID`.
///
/// `CamelSummaryMessageID` is a union over a `guint64` and eight bytes, and
/// Camel fills the bytes from the front of an MD5 over the header value with
/// the angle brackets already off — which is precisely what JMAP's `messageId`
/// property is. Reading the union back as the integer is therefore a
/// native-endian load of those eight bytes, and `from_ne_bytes` is that load
/// written down.
///
/// MD5 is not a security decision here and is not ours to make: it is the
/// digest already in every Camel summary on disk, and the only thing this value
/// is ever compared against is another one of them.
fn message_id_digest(message_id: &str) -> u64 {
    let mut digest = [0u8; 16];
    let mut length = digest.len();

    // SAFETY: the checksum is created, updated and freed here; the buffer is
    // ours and `length` is its true size, which is what `g_checksum_get_digest`
    // requires (it aborts on a buffer too small for the algorithm).
    unsafe {
        let checksum = g_checksum_new(G_CHECKSUM_MD5);
        g_checksum_update(checksum, message_id.as_ptr(), message_id.len() as isize);
        g_checksum_get_digest(checksum, digest.as_mut_ptr(), &mut length);
        g_checksum_free(checksum);
    }

    let (head, _) = digest.split_at(size_of::<u64>());
    u64::from_ne_bytes(
        head.try_into()
            .expect("eight bytes of a sixteen-byte digest"),
    )
}

/// The ancestors, as the `GArray` of digests Camel threads on — or NULL for a
/// message with none, because an empty array is one the threader walks for
/// nothing on every rebuild.
///
/// Reversed on the way in. `references` arrives oldest first, with the
/// `In-Reply-To` parent last, which is the order the headers put it in;
/// Camel's own builder stores the chain the other way round, nearest ancestor
/// first, and its threader walks from the front taking the first ancestor the
/// folder actually holds. Filled in header order, every reply in a long thread
/// would hang off the root of its conversation instead of off its parent.
///
/// The array is returned with its ownership, for `take_references`.
fn references_array(references: &[String]) -> *mut GArray {
    if references.is_empty() {
        return ptr::null_mut();
    }

    // SAFETY: the array is allocated here with `guint64` elements, which is
    // what `camel_message_info_take_references` documents them to be, and each
    // append copies eight bytes out of a local.
    unsafe {
        let array = g_array_sized_new(
            GFALSE,
            GFALSE,
            size_of::<u64>() as u32,
            references.len() as u32,
        );
        for reference in references.iter().rev() {
            let digest = message_id_digest(reference);
            g_array_append_vals(array, ptr::from_ref(&digest).cast(), 1);
        }
        array
    }
}
