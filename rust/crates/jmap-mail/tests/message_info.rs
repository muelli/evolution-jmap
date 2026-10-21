// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelMessageInfo`: one summary row, as Camel stores it.
//!
//! `jmap-mail-sync`'s [`MessageSummary`] is the same row in Rust, read from an
//! `Email/get` response. The two are not a field-for-field copy of each other.
//! Camel computes three columns instead of copying them from the server. The
//! flags word is a bitfield over the keywords. The address columns are one
//! formatted string per header. The two threading columns are 64-bit digests
//! derived from the `Message-ID` text.
//!
//! Half of this file tests the threading columns against an oracle, for this
//! reason: Camel also builds message infos itself, through
//! `camel_message_info_new_from_headers`, for a locally-parsed message. If this
//! crate's digests disagreed with that path, the two paths would thread the
//! same conversation as two conversations when they met. So the tests do not
//! assert a digest this crate chose. Each test gives Camel the same headers the
//! JMAP properties came from, and asserts that Camel's own computed digest
//! matches this crate's digest.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::compat::CamelSummaryMessageRecord;
use eds_sys::{
    CAMEL_MESSAGE_ANSWERED, CAMEL_MESSAGE_ATTACHMENTS, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DRAFT,
    CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FORWARDED, CAMEL_MESSAGE_JUNK, CAMEL_MESSAGE_NOTJUNK,
    CAMEL_MESSAGE_SEEN, CamelMessageInfo, camel_message_info_get_cc,
    camel_message_info_get_date_received, camel_message_info_get_date_sent,
    camel_message_info_get_flags, camel_message_info_get_folder_flagged,
    camel_message_info_get_from, camel_message_info_get_message_id, camel_message_info_get_preview,
    camel_message_info_get_references, camel_message_info_get_size, camel_message_info_get_subject,
    camel_message_info_get_to, camel_message_info_get_uid, camel_message_info_get_user_flag,
    camel_message_info_load, camel_message_info_new_from_headers, camel_message_info_save,
    camel_message_info_set_flags, camel_name_value_array_append, camel_name_value_array_free,
    camel_name_value_array_new,
};
use glib_sys::{GFALSE, GTRUE, g_free, g_string_free, g_string_new, gchar};
use gobject_sys::{g_object_unref, g_type_check_instance_is_a};
use jmap_mail::message_info::{
    message_info_type, new_message_info, row_keywords, server_keywords, server_thread_id,
    update_message_info,
};
use jmap_mail_sync::{Keywords, MessageFlags, MessageSummary};
use jmap_proto::Id;
use jmap_proto::mail::EmailAddress;

/// A row with every field empty except the uid, which every row must have.
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
/// `None` when Camel kept nothing at all. The difference matters. An empty
/// `Cc` column and an absent one look the same to a reader. The database
/// Camel writes the summary to stores them as different values.
///
/// # Safety
///
/// `text` must be NULL or a NUL-terminated string that outlives the call.
unsafe fn column(text: *const gchar) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the accessor returns a string owned by the info. The caller
    // keeps the info alive across this call.
    Some(
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// What Camel itself makes of the headers a JMAP `Email` was derived from.
///
/// This is the oracle for every column below that Camel computes rather than
/// copies. The summary argument is NULL, which is what `camel_message_info_new`
/// falls back on when a summary declares no message-info type of its own. This
/// provider's summary is in that case, so the row comes out a
/// `CamelMessageInfoBase` either way.
fn camel_info_from(headers: &[(&str, &str)]) -> *mut CamelMessageInfo {
    // SAFETY: the array is freshly allocated and owned here. `append` copies
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
    // SAFETY: the array is owned by the info and read-only here. Its elements
    // are `guint64`, by the contract of `camel_message_info_take_references`.
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
/// email id verbatim. Camel wants a uid to be a server-assigned immutable
/// identifier. The email id already is one, so there is nothing to invent
/// here, unlike the folder path.
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

/// The two dates are different columns, read from different JMAP properties.
/// `receivedAt` is the server's clock, and a mailbox sorts by it. `sentAt` is
/// the `Date` header, and gives the sender's clock instead. Reading one date
/// in place of the other is invisible on most messages, but wrong on the ones
/// that matter. This test keeps the two dates far apart, so a mix-up shows.
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

/// A message with no keywords set has none of those bits. The mapping must
/// not be a constant that happens to satisfy the test above. It also must not
/// set `DELETED`: JMAP has no deleted keyword. A row marked deleted by mistake
/// would be a message Evolution hides, and the next expunge would remove it
/// from the server.
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

/// Keywords Camel has no bit for are Evolution's labels. They go in verbatim,
/// including a leading `$`. A flag change sends the keyword back to the
/// server. Renaming a label on the way in would therefore send the wrong
/// keyword back on the way out.
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

/// The address columns. JMAP sends a name and an address for each entry, but
/// Camel stores one formatted string per header, so this code formats them.
/// `CamelInternetAddress` already implements RFC 5322's quoting rules, so the
/// oracle is that same formatter, reading the header a mail client would send.
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

/// A header the message did not carry leaves the column unset, not empty.
/// Camel writes the summary to a database and reads it back. There, an empty
/// string means a `Cc:` header that was present but blank.
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

/// The digest, checked against Camel's own. Camel stores a message id as 64
/// bits of an MD5 it computes over the `Message-ID` value with the angle
/// brackets removed. This is exactly what JMAP's `messageId` property already
/// is, so the two digests must agree. Only Camel can say what agreeing means.
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

/// The ancestors, checked against Camel's own, including their order. The
/// stored order is not the order the header lists them in.
/// `camel_message_info_new_from_headers` stores the chain nearest-ancestor
/// first, with the `In-Reply-To` parent at the head. Camel's threader walks
/// the chain from the front, looking for an ancestor it already has. A
/// provider that filled the array in header order would thread every reply
/// onto the root of its conversation, instead of onto its parent.
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
/// zero and the array absent, and this provider must do the same. An empty
/// references array makes the threader walk the array for nothing on every
/// rebuild. A zero digest is how Camel says a message threads on nothing.
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

/// A JMAP string is a JSON string, so any JMAP value can carry a NUL byte,
/// which RFC 5322 forbids. Handing the raw bytes to Camel would cut the value
/// at the NUL: a subject line would end where the sender hid a NUL. So this
/// code replaces the NUL rather than passing it through, the same way it
/// handles folder names.
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

/// The keywords the row remembers the server holding. This is a fourth
/// column, and the only one that is neither a copy nor a computation. It is
/// what this provider knows, and Camel has no field for it.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn remembered(info: *mut CamelMessageInfo) -> Keywords {
    // SAFETY: the accessor reads the row's own copy of the set and clones it.
    unsafe { server_keywords(info) }.expect("a row of this provider's own kind")
}

/// A row is no longer a `CamelMessageInfoBase`, for one reason: the keywords
/// the last listing found. Without that column, a flag change has no before
/// state to compare against. The row's own flags word is only the after
/// state, mutated in place by the user. Without the before state, every write
/// would have to replace the whole keyword set, over keywords no client here
/// has ever seen.
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

/// The whole point of the column. Marking a row read writes Camel's flags
/// word and nothing else. The row now claims `$seen`, but the column still
/// says the server has not been told. This difference is what lets the
/// provider send exactly the one changed keyword, instead of guessing.
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

/// The other end of the same difference, read back out of the two columns a
/// user's click changes. An untouched row must claim exactly what it
/// remembers, or every synchronisation would write every message back to the
/// server.
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

/// Once the user clicks, the row claims the click. `attachments` is the one
/// bit of the flags word that must not travel back to the server. RFC 8621
/// §4.1.4 has the server compute `hasAttachment`, so sending it back would add
/// a label that every other client would then show.
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

/// A row built from a listing carries no change of the user's, because it did
/// not exist a moment ago. So it must not arrive on the folder's work list.
/// Camel's column setters put every new row there, and `new_message_info`
/// undoes this. Left alone, every message in every mailbox would be written
/// back to the server it was just listed from.
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

/// A refresh that meets a row the user changed, but has not yet saved, leaves
/// the row waiting. Taking the row off the work list would lose the change
/// silently, instead of retrying it. The group of tests below covers what the
/// listing does to the flags of such a row.
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

/// The race that the work list alone does not settle. Evolution's refresh
/// timer can fire between the user marking a message read and the folder
/// synchronising. The listing it brings back is one the server made before
/// the click, so it still reports the message unread. Writing that listing
/// whole would undo the click on screen. It would also make the row claim
/// exactly what the server already holds. Then the next synchronisation would
/// see no difference, and never send the change. The row stays queued either
/// way: what a wrong write loses is not the queued bit, but what the row has
/// to say.
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

/// The other half of the same rule: why a queued row cannot simply refuse a
/// listing. What another client did meanwhile is news that the user's
/// outstanding change says nothing about. So the row applies the listing, and
/// replays the user's change on top of it.
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

/// A keyword the server stopped holding comes off a queued row too. The row
/// keeps only what it is itself waiting to say. Both directions matter. A rule
/// that only ever added keywords would leave a label on screen after another
/// client removed it, until the user next touched the message.
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

/// A row with no outstanding change takes the listing whole. This is the
/// ordinary case, and the only way a drifted row returns to what the server
/// says. The replay above is only for a row waiting to speak; a row that is
/// not waiting has nothing to add to the server's answer.
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
/// server's own conclusion about the message, so every path takes it from the
/// listing. Without this, a queued row would lose it: the keyword set the
/// replay works on cannot carry this bit.
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

/// A refresh renews the remembered set. After a listing reports a keyword
/// another client added, this folder no longer treats that keyword as
/// something to remove.
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

/// The set is written into the one field of the summary database that Camel
/// reserves for what a provider knows and Camel does not. It is read back in
/// the same order. The test calls Camel's own `load` and `save` entry points,
/// rather than checking the encoding directly, because the encoding belongs to
/// Camel. A keyword containing a space proves this: a format that joined
/// names with spaces would read the keyword back as two keywords.
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
        let mut record: CamelSummaryMessageRecord = std::mem::zeroed();
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
        free_saved_strings(&mut record);
        g_string_free(bdata, GTRUE);
        g_object_unref(restored.cast());
        g_object_unref(info.cast());
    }
}

/// A row whose stored data says nothing about keywords is a row written by a
/// summary from before this column existed. Such a row remembers nothing; it
/// does not fail to load. The empty set is also the safe answer. A difference
/// from the empty set only ever adds keywords, so a folder that lost the
/// column removes none.
#[test]
fn a_row_stored_without_the_column_loads_as_a_row_that_remembers_nothing() {
    let mut message = row("Em0026");
    message.tags = vec!["Work".to_owned()];
    let info = info_of(&message);

    // SAFETY: as above, with a record whose bdata is the empty string a summary
    // that wrote no provider data leaves behind.
    unsafe {
        let mut record: CamelSummaryMessageRecord = std::mem::zeroed();
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

/// Checks that repeated headers (for example, multiple `Received` headers) and
/// custom `X-` headers convert safely into a `CamelMessageInfo`, through a
/// `CamelNameValueArray`.
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

/// Checks that a multi-level reply chain keeps nearest-ancestor ordering, and
/// that `CamelMessageInfo` stores non-zero 64-bit digests for it.
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
/// `$notjunk` becomes `CAMEL_MESSAGE_NOTJUNK`. This checks the bit-level
/// mapping between keywords and the `CamelMessageInfo` flags that Camel, and
/// in turn a `CamelJunkFilter` implementation, reads.
///
/// The values (`CAMEL_JUNK_STATUS_MESSAGE_IS_JUNK = 2`,
/// `CAMEL_JUNK_STATUS_MESSAGE_IS_NOT_JUNK = 3`) are an EDS invariant, checked
/// in `eds-sys/tests/camel.rs::camel_junk_filter_interface_and_status_in_eds`.
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

/// A flag-word round trip. Set `CAMEL_MESSAGE_JUNK` on an info built from a
/// non-junk row, then read the current keywords back through `row_keywords`,
/// and confirm `$junk` appears.
///
/// This checks that `camel_message_info_set_flags` and `row_keywords` stay
/// consistent when local Camel state changes independently of the server. For
/// example, a `CamelJunkFilter` might reclassify the message after a sync.
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

/// The thread id `Email/get` reported. This is a fifth column, and like the
/// keywords the server was last seen holding, it is something only this
/// provider knows: Camel has no field for a server-assigned thread.
#[test]
fn a_row_remembers_the_thread_id_the_listing_carried() {
    let mut message = row("Em0040");
    message.thread_id = Some(Id::new("T1"));
    let info = info_of(&message);

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(server_thread_id(info), Some(Id::new("T1")));
        g_object_unref(info.cast());
    }
}

/// A message `Email/get` reported no thread for remembers none. It does not
/// get a digest computed some other way, because only the server can say two
/// messages share a thread.
#[test]
fn a_row_with_no_thread_id_remembers_none() {
    let info = info_of(&row("Em0041"));

    // SAFETY: `info` is a live message info this test owns.
    unsafe {
        assert_eq!(server_thread_id(info), None);
        g_object_unref(info.cast());
    }
}

/// The thread id survives the same round trip through the summary database as
/// the keywords column, through the same `save` and `load` entry points.
#[test]
fn the_thread_id_survives_a_trip_through_the_summary_database() {
    let mut message = row("Em0042");
    message.thread_id = Some(Id::new("T42"));
    let info = info_of(&message);
    let restored = info_of(&row("Em0042"));

    // SAFETY: both infos are live and owned here. The record is zeroed, which
    // is the state Camel hands `save` one in, and every string `save` leaves in
    // it is read back before it goes out of scope.
    unsafe {
        let mut record: CamelSummaryMessageRecord = std::mem::zeroed();
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

        assert_eq!(server_thread_id(restored), Some(Id::new("T42")));
        free_saved_strings(&mut record);
        g_string_free(bdata, GTRUE);
        g_object_unref(restored.cast());
        g_object_unref(info.cast());
    }
}

/// A row stored without the column, one written before the column existed,
/// loads as a row that remembers no thread id. This is the same conservative
/// fallback the keywords column uses.
#[test]
fn a_row_stored_without_the_thread_column_loads_as_remembering_none() {
    let info = info_of(&row("Em0043"));

    // SAFETY: as above, with a record whose bdata is the empty string a
    // summary that wrote no provider data leaves behind.
    unsafe {
        let mut record: CamelSummaryMessageRecord = std::mem::zeroed();
        let bdata = g_string_new(c"".as_ptr());
        record.uid = camel_message_info_get_uid(info);
        record.bdata = (*bdata).str;
        let mut cursor = record.bdata;

        assert_ne!(
            camel_message_info_load(info, ptr::addr_of!(record), ptr::addr_of_mut!(cursor)),
            GFALSE,
            "a row with no provider data would not load"
        );

        assert_eq!(server_thread_id(info), None);
        g_string_free(bdata, GTRUE);
        g_object_unref(info.cast());
    }
}

/// Frees the strings that `save` puts in the heap fields of `record`.
///
/// The string pool owns the `const` fields. Each test frees its own `bdata`.
///
/// # Safety
///
/// `record` must come from one `camel_message_info_save` call.
unsafe fn free_saved_strings(record: &mut CamelSummaryMessageRecord) {
    for field in [
        &mut record.part,
        &mut record.labels,
        &mut record.usertags,
        &mut record.cinfo,
        &mut record.userheaders,
        &mut record.preview,
    ] {
        // SAFETY: `save` allocates each field with GLib, or leaves it NULL.
        unsafe { g_free((*field).cast()) };
        *field = ptr::null_mut();
    }
}
