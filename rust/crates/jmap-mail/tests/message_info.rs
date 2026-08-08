// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelMessageInfo`: one summary row, as the object Camel keeps it in.
//!
//! `jmap-mail-sync`'s [`MessageSummary`] is the same row in Rust, read off an
//! `Email/get` — and the two are not a field-for-field copy of each other.
//! Three of Camel's columns are not values the server sent but things computed
//! from them: the flags word, which is a bitfield over the keywords; the
//! address columns, which Camel stores as one formatted string per header; and
//! the two threading columns, which Camel stores as 64-bit digests rather than
//! as the `Message-ID` text they are derived from.
//!
//! The last of those is the reason half of this file is written against an
//! oracle. Camel builds message infos itself — `camel_message_info_new_from_
//! headers` is the path a locally-parsed message takes — and a provider whose
//! digests disagreed with that path would thread the same conversation into two
//! when the two ever met. So the test does not assert a digest this crate chose:
//! it hands Camel the headers the JMAP properties came from and asserts that
//! what Camel computed is what we computed.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    CAMEL_MESSAGE_ANSWERED, CAMEL_MESSAGE_ATTACHMENTS, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DRAFT,
    CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FORWARDED, CAMEL_MESSAGE_JUNK, CAMEL_MESSAGE_NOTJUNK,
    CAMEL_MESSAGE_SEEN, CamelMessageInfo, camel_message_info_get_cc,
    camel_message_info_get_date_received, camel_message_info_get_date_sent,
    camel_message_info_get_flags, camel_message_info_get_from, camel_message_info_get_message_id,
    camel_message_info_get_preview, camel_message_info_get_references, camel_message_info_get_size,
    camel_message_info_get_subject, camel_message_info_get_to, camel_message_info_get_uid,
    camel_message_info_get_user_flag, camel_message_info_new_from_headers,
    camel_name_value_array_append, camel_name_value_array_free, camel_name_value_array_new,
};
use glib_sys::{GFALSE, gchar};
use gobject_sys::g_object_unref;
use jmap_mail::message_info::new_message_info;
use jmap_mail_sync::{MessageFlags, MessageSummary};
use jmap_proto::Id;
use jmap_proto::mail::EmailAddress;

/// A row with nothing in it but the one thing a row cannot be without.
fn row(uid: &str) -> MessageSummary {
    MessageSummary {
        uid: Id::new(uid),
        blob_id: None,
        thread_id: None,
        flags: MessageFlags::default(),
        tags: Vec::new(),
        size: 0,
        received_at: None,
        sent_at: None,
        subject: None,
        from: Vec::new(),
        to: Vec::new(),
        cc: Vec::new(),
        message_id: None,
        references: Vec::new(),
        preview: None,
    }
}

/// The message info for one row, owned by the caller.
fn info_of(message: &MessageSummary) -> *mut CamelMessageInfo {
    let info = new_message_info(message);
    assert!(!info.is_null(), "no message info for {}", message.uid);
    info
}

/// Reads a summary column back as Camel stores it: a borrowed C string, or
/// `None` where Camel kept nothing at all. The difference matters — an empty
/// `Cc` column and an absent one are the same to a reader and not to the
/// database Camel writes the summary to.
///
/// # Safety
///
/// `text` must be NULL or a NUL-terminated string that outlives the call.
unsafe fn column(text: *const gchar) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the accessors return a string owned by the info, which the
    // caller keeps alive across this.
    Some(
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// What Camel itself makes of the headers a JMAP `Email` was derived from.
///
/// The oracle for everything below that is a computation rather than a copy.
/// `NULL` for the summary is what `camel_message_info_new` falls back on when
/// a summary declares no message-info type of its own, which is the case this
/// provider is in — the row comes out a `CamelMessageInfoBase` either way.
fn camel_info_from(headers: &[(&str, &str)]) -> *mut CamelMessageInfo {
    // SAFETY: the array is freshly allocated and owned here; `append` copies
    // both strings, and every string passed in is NUL-terminated.
    unsafe {
        let array = camel_name_value_array_new();
        for (name, value) in headers {
            let name = CString::new(*name).expect("a header name with no NUL in it");
            let value = CString::new(*value).expect("a header value with no NUL in it");
            camel_name_value_array_append(array, name.as_ptr(), value.as_ptr());
        }
        let info = camel_message_info_new_from_headers(ptr::null_mut(), array);
        camel_name_value_array_free(array);
        assert!(!info.is_null(), "Camel built no message info");
        info
    }
}

/// The 64-bit ancestors Camel threads on, in the order it stored them.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn references(info: *mut CamelMessageInfo) -> Vec<u64> {
    // SAFETY: the array is owned by the info and read-only here; its elements
    // are `guint64` by `camel_message_info_take_references`'s contract.
    unsafe {
        let array = camel_message_info_get_references(info);
        if array.is_null() {
            return Vec::new();
        }
        let len = (*array).len as usize;
        std::slice::from_raw_parts((*array).data.cast::<u64>(), len).to_vec()
    }
}

/// The columns that are a copy rather than a computation: the uid Camel keys
/// the row by, the three plain values, and the two dates. The uid is the JMAP
/// email id verbatim — a server-assigned immutable identifier is what Camel
/// wants a uid to be, so unlike the folder path there is nothing to invent.
#[test]
fn a_row_is_the_camel_view_of_one_email() {
    let mut message = row("Em0001");
    message.subject = Some("Q3 plans".to_owned());
    message.size = 4096;
    message.received_at = Some(1_700_000_100);
    message.sent_at = Some(1_700_000_000);
    message.preview = Some("As discussed,".to_owned());
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(
            column(camel_message_info_get_uid(info)).as_deref(),
            Some("Em0001")
        );
        assert_eq!(
            column(camel_message_info_get_subject(info)).as_deref(),
            Some("Q3 plans")
        );
        assert_eq!(camel_message_info_get_size(info), 4096);
        assert_eq!(camel_message_info_get_date_received(info), 1_700_000_100);
        assert_eq!(camel_message_info_get_date_sent(info), 1_700_000_000);
        assert_eq!(
            column(camel_message_info_get_preview(info)).as_deref(),
            Some("As discussed,")
        );
        g_object_unref(info.cast());
    }
}

/// The two dates are different columns and are read from different JMAP
/// properties: `receivedAt` is the server's clock and what a mailbox is sorted
/// by, `sentAt` is the `Date` header and therefore the sender's. Reading one
/// out of the other is invisible on almost every message and wrong on the ones
/// that matter, so the row that pins it has them far apart.
#[test]
fn the_senders_clock_and_the_servers_are_not_the_same_column() {
    let mut message = row("Em0002");
    message.sent_at = Some(1_000_000_000);
    message.received_at = Some(1_700_000_000);
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(camel_message_info_get_date_sent(info), 1_000_000_000);
        assert_eq!(camel_message_info_get_date_received(info), 1_700_000_000);
        g_object_unref(info.cast());
    }
}

/// Every keyword Camel has a bit for, in the one word it keeps them in.
#[test]
fn the_keywords_camel_has_a_bit_for_become_the_flags_word() {
    let mut message = row("Em0003");
    message.flags = MessageFlags {
        seen: true,
        answered: true,
        flagged: true,
        draft: true,
        forwarded: true,
        junk: true,
        not_junk: true,
        attachments: true,
    };
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        let flags = camel_message_info_get_flags(info);
        for (bit, name) in [
            (CAMEL_MESSAGE_SEEN, "seen"),
            (CAMEL_MESSAGE_ANSWERED, "answered"),
            (CAMEL_MESSAGE_FLAGGED, "flagged"),
            (CAMEL_MESSAGE_DRAFT, "draft"),
            (CAMEL_MESSAGE_FORWARDED, "forwarded"),
            (CAMEL_MESSAGE_JUNK, "junk"),
            (CAMEL_MESSAGE_NOTJUNK, "not junk"),
            (CAMEL_MESSAGE_ATTACHMENTS, "attachments"),
        ] {
            assert_ne!(flags & bit, 0, "{name} did not reach the flags word");
        }
        g_object_unref(info.cast());
    }
}

/// A message with no keywords set has none of those bits — the mapping is not
/// allowed to be a constant that happens to satisfy the test above. Nor may it
/// invent `DELETED`: JMAP has no deleted keyword, and a row that arrived marked
/// deleted would be one Evolution hides and the next expunge takes off the
/// server.
#[test]
fn a_message_with_no_keywords_carries_no_bits() {
    let info = info_of(&row("Em0004"));

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        let flags = camel_message_info_get_flags(info);
        assert_eq!(
            flags & (CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_JUNK | CAMEL_MESSAGE_ATTACHMENTS),
            0
        );
        assert_eq!(flags & CAMEL_MESSAGE_DELETED, 0, "JMAP cannot say this");
        g_object_unref(info.cast());
    }
}

/// Keywords Camel has no bit for are Evolution's labels, and they go in
/// verbatim — leading `$` included. A flag change sends the keyword back to the
/// server, so a label renamed on the way in would not be the same keyword on
/// the way out.
#[test]
fn the_keywords_camel_has_no_bit_for_become_user_flags() {
    let mut message = row("Em0005");
    message.flags = MessageFlags {
        seen: true,
        ..MessageFlags::default()
    };
    message.tags = vec!["$label1".to_owned(), "todo".to_owned()];
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns, and each name is a
    // NUL-terminated literal.
    unsafe {
        assert_ne!(
            camel_message_info_get_user_flag(info, c"$label1".as_ptr()),
            GFALSE
        );
        assert_ne!(
            camel_message_info_get_user_flag(info, c"todo".as_ptr()),
            GFALSE
        );
        assert_eq!(
            camel_message_info_get_user_flag(info, c"$seen".as_ptr()),
            GFALSE,
            "a keyword the flags word covers is not also a label"
        );
        g_object_unref(info.cast());
    }
}

/// The address columns. JMAP hands the parts over — a name and an address per
/// entry — and Camel stores one string per header, so the formatting happens
/// here; `CamelInternetAddress` is where RFC 5322's quoting rules already live,
/// and the oracle is that same formatter reading the header a mail client would
/// have sent.
#[test]
fn an_address_list_is_formatted_the_way_camel_formats_one() {
    let mut message = row("Em0006");
    message.from = vec![EmailAddress::new(Some("Ann Example"), "ann@example.com")];
    message.to = vec![
        EmailAddress::new(Some("Bo Example"), "bo@example.com"),
        EmailAddress::new(None, "cy@example.com"),
    ];
    message.cc = vec![EmailAddress::new(Some("Di, Example"), "di@example.com")];
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(
            column(camel_message_info_get_from(info)).as_deref(),
            Some("Ann Example <ann@example.com>")
        );
        assert_eq!(
            column(camel_message_info_get_to(info)).as_deref(),
            Some("Bo Example <bo@example.com>, cy@example.com")
        );
        assert_eq!(
            column(camel_message_info_get_cc(info)).as_deref(),
            Some("\"Di, Example\" <di@example.com>"),
            "a comma in a display name is quoted, not a second address"
        );
        g_object_unref(info.cast());
    }
}

/// A header the message did not carry leaves the column unset rather than
/// empty. Camel writes the summary to a database and reads it back; an empty
/// string there is a `Cc:` that was present and blank.
#[test]
fn a_header_the_message_did_not_carry_leaves_no_column() {
    let info = info_of(&row("Em0007"));

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(column(camel_message_info_get_from(info)), None);
        assert_eq!(column(camel_message_info_get_to(info)), None);
        assert_eq!(column(camel_message_info_get_cc(info)), None);
        assert_eq!(column(camel_message_info_get_subject(info)), None);
        assert_eq!(column(camel_message_info_get_preview(info)), None);
        g_object_unref(info.cast());
    }
}

/// The digest, against Camel's own. Camel stores a message id as 64 bits of an
/// MD5 it computes over the `Message-ID` value with the angle brackets off,
/// which is exactly what JMAP's `messageId` property already is — so the two
/// have to agree, and nothing but Camel can say what agreeing means.
#[test]
fn the_message_id_is_the_digest_camel_computes_from_the_header() {
    let mut message = row("Em0008");
    message.message_id = Some("a1@example.com".to_owned());
    let info = info_of(&message);
    let oracle = camel_info_from(&[("Message-ID", "<a1@example.com>")]);

    // SAFETY: both infos are live and owned here.
    unsafe {
        let digest = camel_message_info_get_message_id(info);
        assert_ne!(digest, 0, "a message with an id threads on something");
        assert_eq!(digest, camel_message_info_get_message_id(oracle));
        g_object_unref(info.cast());
        g_object_unref(oracle.cast());
    }
}

/// The ancestors, against Camel's own — including their order, which is not the
/// order the header lists them in. `camel_message_info_new_from_headers` stores
/// the chain nearest-ancestor first, with the `In-Reply-To` parent at the head,
/// and Camel's threader walks it from the front looking for an ancestor it
/// already has. A provider that filled the array in header order would thread
/// every reply onto the root of its conversation instead of onto its parent.
#[test]
fn the_ancestors_are_stored_in_the_order_camel_threads_on() {
    let mut message = row("Em0009");
    message.message_id = Some("a1@example.com".to_owned());
    // What `jmap-mail-sync` produces for the headers below: the chain oldest
    // first, with the `In-Reply-To` parent appended.
    message.references = vec![
        "r1@example.com".to_owned(),
        "r2@example.com".to_owned(),
        "r3@example.com".to_owned(),
    ];
    let info = info_of(&message);
    let oracle = camel_info_from(&[
        ("Message-ID", "<a1@example.com>"),
        ("References", "<r1@example.com> <r2@example.com>"),
        ("In-Reply-To", "<r3@example.com>"),
    ]);

    // SAFETY: both infos are live and owned here.
    unsafe {
        let ours = references(info);
        assert_eq!(ours.len(), 3, "one entry per ancestor");
        assert_eq!(ours, references(oracle));
        g_object_unref(info.cast());
        g_object_unref(oracle.cast());
    }
}

/// A message with no `Message-ID` and no ancestors. Camel leaves the digest
/// zero and the array absent, and so must this: a row with an empty references
/// array is one the threader walks for nothing on every rebuild, and a zero
/// digest is how Camel says "this message threads on nothing".
#[test]
fn a_message_that_threads_on_nothing_stores_nothing() {
    let info = info_of(&row("Em0010"));
    let oracle = camel_info_from(&[]);

    // SAFETY: both infos are live and owned here.
    unsafe {
        assert_eq!(camel_message_info_get_message_id(info), 0);
        assert_eq!(
            camel_message_info_get_message_id(info),
            camel_message_info_get_message_id(oracle)
        );
        assert!(
            camel_message_info_get_references(info).is_null(),
            "no ancestors is no array, not an empty one"
        );
        assert!(camel_message_info_get_references(oracle).is_null());
        g_object_unref(info.cast());
        g_object_unref(oracle.cast());
    }
}

/// A JMAP string is a JSON string, so any of these can carry a NUL that RFC
/// 5322 forbids. Handing the bytes to Camel would cut the value there — a
/// subject line that ends where the sender hid a NUL — so it is rewritten
/// rather than obeyed, exactly as the folder names are.
#[test]
fn a_value_with_a_nul_in_it_does_not_truncate_the_row() {
    let mut message = row("Em0011");
    message.subject = Some("Invoice\0 attached".to_owned());
    message.tags = vec!["to\0do".to_owned()];
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(
            column(camel_message_info_get_subject(info)).as_deref(),
            Some("Invoice\u{fffd} attached")
        );
        assert_ne!(
            camel_message_info_get_user_flag(info, c"to\u{fffd}do".as_ptr()),
            GFALSE
        );
        g_object_unref(info.cast());
    }
}
