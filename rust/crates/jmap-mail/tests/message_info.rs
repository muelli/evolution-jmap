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
    CAMEL_MESSAGE_SEEN, CamelMIRecord, CamelMessageInfo, camel_message_info_get_cc,
    camel_message_info_get_date_received, camel_message_info_get_date_sent,
    camel_message_info_get_flags, camel_message_info_get_folder_flagged,
    camel_message_info_get_from, camel_message_info_get_message_id, camel_message_info_get_preview,
    camel_message_info_get_references, camel_message_info_get_size, camel_message_info_get_subject,
    camel_message_info_get_to, camel_message_info_get_uid, camel_message_info_get_user_flag,
    camel_message_info_load, camel_message_info_new_from_headers, camel_message_info_save,
    camel_message_info_set_flags, camel_name_value_array_append, camel_name_value_array_free,
    camel_name_value_array_new,
};
use glib_sys::{GFALSE, GTRUE, g_string_free, g_string_new, gchar};
use gobject_sys::{g_object_unref, g_type_check_instance_is_a};
use jmap_mail::message_info::{
    message_info_type, new_message_info, row_keywords, server_keywords, update_message_info,
};
use jmap_mail_sync::{Keywords, MessageFlags, MessageSummary};
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

/// The keywords the row remembers the *server* holding, which is a fourth
/// column and the only one that is neither a copy nor a computation: it is what
/// this provider knows and Camel has no field for.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn remembered(info: *mut CamelMessageInfo) -> Keywords {
    // SAFETY: the accessor reads the row's own copy of the set and clones it.
    unsafe { server_keywords(info) }.expect("a row of this provider's own kind")
}

/// A row is not a `CamelMessageInfoBase` any more, and the reason is one
/// column: the keywords the last listing found. Without them a flag change has
/// no *before* to be the difference from — the row's own flags word is the
/// after, mutated in place by the user — so every write would have to be a
/// whole-set replacement over keywords no client here has ever seen.
#[test]
fn a_row_is_the_provider_s_own_kind_of_row() {
    let info = info_of(&row("Em0020"));

    // SAFETY: `info` is a live message info this test owns, and a GObject.
    unsafe {
        assert_ne!(
            g_type_check_instance_is_a(info.cast(), message_info_type()),
            GFALSE,
            "the row is not the type the summary declares"
        );
        g_object_unref(info.cast());
    }
}

#[test]
fn a_row_remembers_the_keywords_the_listing_carried() {
    let mut message = row("Em0021");
    message.flags.seen = true;
    message.tags = vec!["Work".to_owned()];
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(
            remembered(info),
            Keywords::new(&message.flags, &message.tags)
        );
        g_object_unref(info.cast());
    }
}

/// The whole point of the column. The user marking a row read writes Camel's
/// flags word and nothing else, so the row now claims `$seen` and the column
/// still says the server has not been told — which is what makes the one
/// keyword to send a difference rather than a guess.
#[test]
fn a_flag_the_user_changed_does_not_change_what_the_row_remembers() {
    let mut message = row("Em0022");
    message.tags = vec!["Work".to_owned()];
    let info = info_of(&message);
    let listed = Keywords::new(&message.flags, &message.tags);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        camel_message_info_set_flags(info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

        assert_ne!(camel_message_info_get_flags(info) & CAMEL_MESSAGE_SEEN, 0);
        assert_eq!(
            remembered(info),
            listed,
            "a local mark rewrote what the server was last seen holding"
        );
        g_object_unref(info.cast());
    }
}

/// The row's other end of the same difference, read back out of the two columns
/// the user's click lands in. A row nobody has touched has to claim exactly what
/// it remembers, or every synchronisation would write every message back.
#[test]
fn an_untouched_row_claims_the_keywords_it_remembers() {
    let mut message = row("Em0031");
    message.flags.seen = true;
    message.flags.flagged = true;
    message.tags = vec!["Work".to_owned(), "home/todo".to_owned()];
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(row_keywords(info), remembered(info));
        g_object_unref(info.cast());
    }
}

/// And once the user has clicked, it claims the click. `attachments` is the one
/// bit of the flags word that must not travel: `hasAttachment` is a property RFC
/// 8621 §4.1.1 has the server compute, so sending it back would put a label on
/// the message every other client would then show.
#[test]
fn a_row_claims_the_flag_the_user_just_set() {
    let mut message = row("Em0032");
    message.flags.attachments = true;
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        camel_message_info_set_flags(info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

        let claimed = row_keywords(info);
        assert_eq!(claimed.iter().collect::<Vec<&str>>(), vec!["$seen"]);
        g_object_unref(info.cast());
    }
}

/// A row built out of a listing carries no change of the user's — it did not
/// exist a moment ago — so it must not arrive on the folder's work list. Camel's
/// column setters put it there, which is what `new_message_info` undoes: left
/// alone, every message of every mailbox would be written back to the server it
/// was just listed from.
#[test]
fn a_row_a_listing_built_is_not_waiting_for_the_server() {
    let mut message = row("Em0033");
    message.flags.seen = true;
    message.subject = Some("Lunch?".to_owned());
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(camel_message_info_get_folder_flagged(info), GFALSE);
        g_object_unref(info.cast());
    }
}

/// And a refresh that meets a row the user has changed and not yet saved leaves
/// it waiting: taking the row off the work list would lose the change in silence
/// rather than retry it. What the listing does to the *flags* of such a row is
/// the group of tests below.
#[test]
fn a_listing_does_not_take_an_unsaved_change_off_the_work_list() {
    let info = info_of(&row("Em0034"));
    let mut listed = row("Em0034");
    listed.flags.seen = true;

    // SAFETY: `info` is a live message info this test owns, updated from a row
    // with the same uid.
    unsafe {
        camel_message_info_set_flags(info, CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FLAGGED);
        assert_ne!(
            camel_message_info_get_folder_flagged(info),
            GFALSE,
            "Camel did not queue the row the user changed"
        );

        update_message_info(info, &listed);

        assert_ne!(
            camel_message_info_get_folder_flagged(info),
            GFALSE,
            "the listing dropped the user's unsaved change"
        );
        g_object_unref(info.cast());
    }
}

/// The race the work list on its own does not settle. Evolution's refresh timer
/// goes off between the user marking a message read and the folder being
/// synchronised, and the listing it brings back is one the server made before
/// the click: it says unread. Written whole, it would undo the click on screen
/// *and* leave the row claiming what the server already holds — so the diff the
/// next synchronisation makes would be empty and the change would never be sent.
/// The row stays queued either way; what is lost is what it has to say.
#[test]
fn a_listing_does_not_undo_a_flag_the_user_has_not_saved_yet() {
    let info = info_of(&row("Em0035"));
    // The server has not heard about the click, so its listing is the row as it
    // was before it.
    let listed = row("Em0035");

    // SAFETY: `info` is a live message info this test owns, updated from a row
    // with the same uid.
    unsafe {
        camel_message_info_set_flags(info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

        update_message_info(info, &listed);

        assert_ne!(
            camel_message_info_get_flags(info) & CAMEL_MESSAGE_SEEN,
            0,
            "the listing undid the user's unsaved change"
        );
        // And the row remembers the *listing*, not what it claims: the two
        // together are what makes the next synchronisation send `$seen`.
        assert_eq!(remembered(info), Keywords::new(&listed.flags, &listed.tags));
        g_object_unref(info.cast());
    }
}

/// The other half of the same rule, and why the row cannot simply refuse a
/// listing while it is queued: what another client did in the meantime is news,
/// and the user's outstanding change says nothing about it. The listing is
/// applied and the change replayed on top.
#[test]
fn a_listing_still_brings_a_queued_row_what_the_server_changed() {
    let info = info_of(&row("Em0036"));
    let mut listed = row("Em0036");
    listed.flags.seen = true;
    listed.tags = vec!["Urgent".to_owned()];

    // SAFETY: `info` is a live message info this test owns, updated from a row
    // with the same uid.
    unsafe {
        camel_message_info_set_flags(info, CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FLAGGED);

        update_message_info(info, &listed);

        let flags = camel_message_info_get_flags(info);
        assert_ne!(
            flags & CAMEL_MESSAGE_FLAGGED,
            0,
            "the user's unsaved change was overwritten"
        );
        assert_ne!(
            flags & CAMEL_MESSAGE_SEEN,
            0,
            "the server's change was lost"
        );
        assert_ne!(
            camel_message_info_get_user_flag(info, c"Urgent".as_ptr()),
            GFALSE,
            "the label another client added never arrived"
        );
        g_object_unref(info.cast());
    }
}

/// A keyword the server *stopped* holding comes off a queued row too — the row
/// keeps only what it is itself waiting to say. Both directions matter: a rule
/// that only ever added would leave a label another client removed on screen
/// until the user next touched the message.
#[test]
fn a_listing_takes_a_keyword_off_a_queued_row_when_the_server_did() {
    let mut message = row("Em0037");
    message.tags = vec!["Work".to_owned()];
    let info = info_of(&message);
    // The same message after another client took the label off.
    let listed = row("Em0037");

    // SAFETY: `info` is a live message info this test owns, updated from a row
    // with the same uid.
    unsafe {
        camel_message_info_set_flags(info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

        update_message_info(info, &listed);

        assert_eq!(
            camel_message_info_get_user_flag(info, c"Work".as_ptr()),
            GFALSE,
            "the label the server dropped is still on the row"
        );
        assert_ne!(
            camel_message_info_get_flags(info) & CAMEL_MESSAGE_SEEN,
            0,
            "the user's unsaved change went with it"
        );
        g_object_unref(info.cast());
    }
}

/// And a row with nothing outstanding takes the listing whole, which is the
/// ordinary case and the only thing that ever brings a row that has drifted back
/// to what the server says. The replay above is for the row that is *waiting* to
/// speak; a row that is not has nothing to add to the server's answer.
#[test]
fn a_listing_that_dropped_a_flag_takes_it_off_an_untouched_row() {
    let mut message = row("Em0038");
    message.flags.seen = true;
    message.tags = vec!["Work".to_owned()];
    let info = info_of(&message);
    let listed = row("Em0038");

    // SAFETY: `info` is a live message info this test owns, updated from a row
    // with the same uid.
    unsafe {
        update_message_info(info, &listed);

        assert_eq!(
            camel_message_info_get_flags(info) & CAMEL_MESSAGE_SEEN,
            0,
            "a row nobody touched kept a flag the server had dropped"
        );
        assert_eq!(
            camel_message_info_get_user_flag(info, c"Work".as_ptr()),
            GFALSE
        );
        g_object_unref(info.cast());
    }
}

/// The bit of the flags word that is not a keyword. `hasAttachment` is the
/// server's own conclusion about the message, so it comes off the listing on
/// every path — a queued row would otherwise lose it, since the set the replay
/// works on cannot carry it.
#[test]
fn a_queued_row_still_takes_the_servers_word_on_an_attachment() {
    let info = info_of(&row("Em0039"));
    let mut listed = row("Em0039");
    listed.flags.attachments = true;

    // SAFETY: `info` is a live message info this test owns, updated from a row
    // with the same uid.
    unsafe {
        camel_message_info_set_flags(info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);

        update_message_info(info, &listed);

        assert_ne!(
            camel_message_info_get_flags(info) & CAMEL_MESSAGE_ATTACHMENTS,
            0,
            "the row lost the server's word on its attachments"
        );
        g_object_unref(info.cast());
    }
}

/// A refresh is where the set is renewed: the listing that reports a keyword
/// another client added is the listing after which that keyword is no longer
/// something this folder would remove.
#[test]
fn a_later_listing_replaces_the_keywords_the_row_remembers() {
    let mut first = row("Em0023");
    first.tags = vec!["Work".to_owned()];
    let mut second = row("Em0023");
    second.tags = vec!["Later".to_owned()];
    let info = info_of(&first);

    // SAFETY: `info` is a live message info this test owns, built from a row
    // with the same uid as the one it is updated from.
    unsafe {
        update_message_info(info, &second);

        assert_eq!(remembered(info), Keywords::new(&second.flags, &second.tags));
        g_object_unref(info.cast());
    }
}

/// The set is written into the one field of the summary database Camel reserves
/// for what a provider knows and it does not, and read back out of it in the
/// same order. Asserted through Camel's own `load`/`save` entry points rather
/// than against the encoding, because the encoding is Camel's: a keyword with a
/// space in it is what says so, since a format that joined the names would come
/// back as two keywords.
#[test]
fn the_keywords_a_row_remembers_survive_a_trip_through_the_summary_database() {
    let mut message = row("Em0025");
    message.flags.seen = true;
    message.tags = vec!["Read later".to_owned(), "9-lives".to_owned()];
    let info = info_of(&message);
    let restored = info_of(&row("Em0025"));

    // SAFETY: both infos are live and owned here. The record is zeroed, which
    // is the state Camel hands `save` one in, and every string `save` leaves in
    // it is read back before it goes out of scope.
    unsafe {
        let mut record: CamelMIRecord = std::mem::zeroed();
        let bdata = g_string_new(ptr::null());
        assert_ne!(
            camel_message_info_save(info, ptr::addr_of_mut!(record), bdata),
            GFALSE,
            "the row would not save"
        );

        record.bdata = (*bdata).str;
        let mut cursor = record.bdata;
        assert_ne!(
            camel_message_info_load(restored, ptr::addr_of!(record), ptr::addr_of_mut!(cursor)),
            GFALSE,
            "the row would not load"
        );

        assert_eq!(
            remembered(restored),
            Keywords::new(&message.flags, &message.tags)
        );
        g_string_free(bdata, GTRUE);
        g_object_unref(restored.cast());
        g_object_unref(info.cast());
    }
}

/// A row whose stored data says nothing about keywords — one written by a
/// summary from before this column existed — is a row that remembers nothing,
/// not one that fails to load. The empty set is also the safe answer: the
/// difference from it only ever *adds* keywords, so a folder that lost the
/// column removes none.
#[test]
fn a_row_stored_without_the_column_loads_as_a_row_that_remembers_nothing() {
    let mut message = row("Em0026");
    message.tags = vec!["Work".to_owned()];
    let info = info_of(&message);

    // SAFETY: as above, with a record whose bdata is the empty string a summary
    // that wrote no provider data leaves behind.
    unsafe {
        let mut record: CamelMIRecord = std::mem::zeroed();
        let bdata = g_string_new(c"".as_ptr());
        record.uid = camel_message_info_get_uid(info);
        record.bdata = (*bdata).str;
        let mut cursor = record.bdata;

        assert_ne!(
            camel_message_info_load(info, ptr::addr_of!(record), ptr::addr_of_mut!(cursor)),
            GFALSE,
            "a row with no provider data would not load"
        );

        assert!(remembered(info).is_empty());
        g_string_free(bdata, GTRUE);
        g_object_unref(info.cast());
    }
}

/// Verifying that headers containing multiple occurrences (e.g. repeated `Received` headers)
/// and custom `X-` headers are safely processed into `CamelMessageInfo` via `CamelNameValueArray`.
#[test]
fn the_name_value_array_handles_duplicate_and_special_headers() {
    let headers = &[
        ("From", "sender@example.com"),
        ("To", "receiver@example.com"),
        ("Subject", "Multi-hop message"),
        ("Received", "from mail1.example.org by mx.example.com"),
        ("Received", "from client.local by mail1.example.org"),
        ("X-Evolution-Custom", "true"),
        ("X-JMAP-State", "sync-v1"),
        ("Message-ID", "<msg-special-01@example.com>"),
    ];

    let info = camel_info_from(headers);
    unsafe {
        assert_eq!(
            column(camel_message_info_get_subject(info)).as_deref(),
            Some("Multi-hop message")
        );
        assert_eq!(
            column(camel_message_info_get_from(info)).as_deref(),
            Some("sender@example.com")
        );
        assert_eq!(
            column(camel_message_info_get_to(info)).as_deref(),
            Some("receiver@example.com")
        );
        let digest = camel_message_info_get_message_id(info);
        assert_ne!(digest, 0);

        g_object_unref(info.cast());
    }
}

/// Verifying that multi-level reply chains and references maintain proper nearest-ancestor
/// ordering and non-zero 64-bit digests in `CamelMessageInfo`.
#[test]
fn multiple_references_and_long_in_reply_to_chains_maintain_ancestry_ordering() {
    let mut message = row("Em0099");
    message.message_id = Some("reply-03@example.com".to_owned());
    message.references = vec![
        "root-00@example.com".to_owned(),
        "parent-01@example.com".to_owned(),
        "immediate-02@example.com".to_owned(),
    ];
    let info = info_of(&message);
    let oracle = camel_info_from(&[
        ("Message-ID", "<reply-03@example.com>"),
        (
            "References",
            "<root-00@example.com> <parent-01@example.com>",
        ),
        ("In-Reply-To", "<immediate-02@example.com>"),
    ]);

    unsafe {
        let refs = references(info);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs, references(oracle));
        assert_ne!(camel_message_info_get_message_id(info), 0);

        g_object_unref(info.cast());
        g_object_unref(oracle.cast());
    }
}

/// The `$junk` keyword becomes `CAMEL_MESSAGE_JUNK` in the flags word, and
/// `$notjunk` becomes `CAMEL_MESSAGE_NOTJUNK`.  This verifies the bit-level
/// correspondence between our keyword→flag mapping and the CamelMessageInfo
/// flags that Camel (and, in turn, a `CamelJunkFilter` implementation) reads.
///
/// The positioning (`CAMEL_JUNK_STATUS_MESSAGE_IS_JUNK = 2`,
/// `CAMEL_JUNK_STATUS_MESSAGE_IS_NOT_JUNK = 3`) is an EDS invariant checked in
/// `eds-sys/tests/camel.rs::camel_junk_filter_interface_and_status_in_eds`.
#[test]
fn junk_and_not_junk_keywords_map_to_camel_message_flags() {
    // Message with only $junk set (server believes it is spam).
    let junk_only = MessageSummary {
        flags: MessageFlags {
            junk: true,
            not_junk: false,
            ..MessageFlags::default()
        },
        ..row("Em-junk-01")
    };
    let info = info_of(&junk_only);
    unsafe {
        let flags = camel_message_info_get_flags(info);
        assert_ne!(
            flags & CAMEL_MESSAGE_JUNK,
            0,
            "CAMEL_MESSAGE_JUNK must be set"
        );
        assert_eq!(
            flags & CAMEL_MESSAGE_NOTJUNK,
            0,
            "CAMEL_MESSAGE_NOTJUNK must be clear when only $junk is set"
        );
        g_object_unref(info.cast());
    }

    // Message with only $notjunk set (user manually cleared spam verdict).
    let not_junk_only = MessageSummary {
        flags: MessageFlags {
            junk: false,
            not_junk: true,
            ..MessageFlags::default()
        },
        ..row("Em-junk-02")
    };
    let info2 = info_of(&not_junk_only);
    unsafe {
        let flags = camel_message_info_get_flags(info2);
        assert_eq!(
            flags & CAMEL_MESSAGE_JUNK,
            0,
            "CAMEL_MESSAGE_JUNK must be clear when only $notjunk is set"
        );
        assert_ne!(
            flags & CAMEL_MESSAGE_NOTJUNK,
            0,
            "CAMEL_MESSAGE_NOTJUNK must be set"
        );
        g_object_unref(info2.cast());
    }
}

/// A flag-word update round-trip: set `CAMEL_MESSAGE_JUNK` on an info created
/// from a non-junk row, then read back the current keywords through
/// `row_keywords` and confirm `$junk` appears.  This tests that
/// `camel_message_info_set_flags` + `row_keywords` stay consistent when the
/// local Camel state changes independently of the server (e.g. a
/// `CamelJunkFilter` reclassified the message after sync).
#[test]
fn junk_flag_update_round_trips_through_row_keywords() {
    let clean = row("Em-junk-03");
    let info = info_of(&clean);
    unsafe {
        // Initially no junk bit.
        let before = camel_message_info_get_flags(info);
        assert_eq!(before & CAMEL_MESSAGE_JUNK, 0);

        // Simulate the junk filter setting the bit.
        camel_message_info_set_flags(info, CAMEL_MESSAGE_JUNK, CAMEL_MESSAGE_JUNK);
        let after = camel_message_info_get_flags(info);
        assert_ne!(after & CAMEL_MESSAGE_JUNK, 0);

        // The keyword set derived from the current flags word must now include $junk.
        let kw_after = row_keywords(info);
        assert!(
            kw_after.iter().any(|k| k == "$junk"),
            "row_keywords must contain $junk after CAMEL_MESSAGE_JUNK is set; got {:?}",
            kw_after.iter().collect::<Vec<_>>(),
        );

        g_object_unref(info.cast());
    }
}
