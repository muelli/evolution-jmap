// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! One `Email` as the row Camel keeps about a message.
//!
//! From `Email` literals rather than from a server: what a mock can produce is
//! a well-formed message, and most of this mapping is about the fields a
//! well-formed message leaves out. `messages.rs` is the same mapping over the
//! wire.

use jmap_mail_sync::{MessageFlags, MessageSummary};
use jmap_proto::Id;
use jmap_proto::mail::{Email, EmailAddress, keyword};

/// The least a server may answer `Email/get` with: RFC 8620 §5.1 guarantees
/// only the id.
fn bare_email() -> Email {
    Email {
        id: Some(Id::new("M1")),
        ..Email::default()
    }
}

#[test]
fn what_camel_keeps_about_a_message_comes_off_the_email() {
    let email = Email {
        id: Some(Id::new("M42")),
        blob_id: Some(Id::new("B42")),
        thread_id: Some(Id::new("T42")),
        size: Some(4096),
        received_at: Some("2026-01-15T09:31:00Z".into()),
        sent_at: Some("2026-01-15T10:30:00+01:00".to_owned()),
        subject: Some("Lunch?".to_owned()),
        from: Some(vec![EmailAddress::new(Some("Bob"), "bob@example.com")]),
        to: Some(vec![EmailAddress::new(Some("Alice"), "alice@example.com")]),
        cc: Some(vec![EmailAddress::new(None, "carol@example.com")]),
        preview: Some("Are you free at one?".to_owned()),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(summary.uid, Id::new("M42"), "the JMAP id is the Camel uid");
    assert_eq!(summary.blob_id, Some(Id::new("B42")));
    assert_eq!(summary.thread_id, Some(Id::new("T42")));
    assert_eq!(summary.size, 4096);
    assert_eq!(summary.subject.as_deref(), Some("Lunch?"));
    assert_eq!(summary.from, email.from.unwrap());
    assert_eq!(summary.to, email.to.unwrap());
    assert_eq!(summary.cc, email.cc.unwrap());
    assert_eq!(summary.preview.as_deref(), Some("Are you free at one?"));
    // Both dates as seconds since the epoch, the zone read rather than
    // assumed: the sender wrote 10:30, an hour east of UTC, and the message
    // arrived a minute after it was sent rather than an hour before.
    assert_eq!(summary.received_at, Some(1_768_469_460));
    assert_eq!(summary.sent_at, Some(1_768_469_400));
}

#[test]
fn a_message_the_server_says_nothing_about_is_still_a_row() {
    let summary = MessageSummary::from_email(&bare_email()).expect("a summary");

    assert_eq!(summary.uid, Id::new("M1"));
    assert_eq!(summary.blob_id, None);
    assert_eq!(summary.thread_id, None);
    assert_eq!(summary.size, 0);
    assert_eq!(summary.received_at, None);
    assert_eq!(summary.sent_at, None);
    assert_eq!(summary.subject, None);
    assert!(summary.from.is_empty());
    assert!(summary.to.is_empty());
    assert!(summary.cc.is_empty());
    assert_eq!(summary.message_id, None);
    assert!(summary.references.is_empty());
    assert!(summary.tags.is_empty());
    assert_eq!(summary.flags, MessageFlags::default());
}

#[test]
fn an_email_without_an_id_is_a_protocol_error() {
    let email = Email {
        id: None,
        subject: Some("Who am I".to_owned()),
        ..Email::default()
    };

    let error = MessageSummary::from_email(&email).expect_err("no id, no uid");

    assert!(
        error.to_string().contains("without an id"),
        "unhelpful error: {error}"
    );
}

#[test]
fn the_keywords_camel_has_a_flag_for_become_flags() {
    let email = Email {
        keywords: Some(
            [
                (keyword::SEEN.to_owned(), true),
                (keyword::ANSWERED.to_owned(), true),
                (keyword::FLAGGED.to_owned(), true),
                (keyword::DRAFT.to_owned(), true),
                (keyword::FORWARDED.to_owned(), true),
                (keyword::JUNK.to_owned(), true),
                (keyword::NOT_JUNK.to_owned(), true),
            ]
            .into(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(
        summary.flags,
        MessageFlags {
            seen: true,
            answered: true,
            flagged: true,
            draft: true,
            forwarded: true,
            junk: true,
            not_junk: true,
            attachments: false,
        }
    );
    assert!(
        summary.tags.is_empty(),
        "a mapped keyword is not also a tag: {:?}",
        summary.tags
    );
}

#[test]
fn a_keyword_camel_has_no_flag_for_becomes_a_tag() {
    let email = Email {
        keywords: Some(
            [
                (keyword::SEEN.to_owned(), true),
                ("$phishing".to_owned(), true),
                ("todo".to_owned(), true),
            ]
            .into(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert!(summary.flags.seen);
    // Verbatim, including the `$`: what goes back to the server on a flag
    // change is the keyword the server sent.
    assert_eq!(
        summary.tags,
        vec!["$phishing".to_owned(), "todo".to_owned()]
    );
}

#[test]
fn a_keyword_set_to_false_is_not_set_at_all() {
    let email = Email {
        keywords: Some(
            [
                (keyword::SEEN.to_owned(), false),
                ("todo".to_owned(), false),
            ]
            .into(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert!(!summary.flags.seen, "false is not a set keyword");
    assert!(summary.tags.is_empty());
}

#[test]
fn a_keyword_is_matched_whatever_case_it_arrives_in() {
    let email = Email {
        keywords: Some([("$Seen".to_owned(), true)].into()),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert!(summary.flags.seen, "keywords are case-insensitive");
    assert!(summary.tags.is_empty());
}

#[test]
fn an_attachment_is_a_flag_camel_reads_but_no_keyword_carries() {
    let email = Email {
        has_attachment: Some(true),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert!(summary.flags.attachments);
    assert!(summary.tags.is_empty(), "hasAttachment is not a keyword");
}

#[test]
fn the_threading_headers_come_over_as_the_server_wrote_them() {
    let email = Email {
        message_id: Some(vec!["c@example.com".to_owned()]),
        in_reply_to: Some(vec!["b@example.com".to_owned()]),
        references: Some(vec!["a@example.com".to_owned(), "b@example.com".to_owned()]),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(summary.message_id.as_deref(), Some("c@example.com"));
    // In-Reply-To adds nothing here: References already ends in it.
    assert_eq!(
        summary.references,
        vec!["a@example.com".to_owned(), "b@example.com".to_owned()]
    );
}

#[test]
fn a_reply_whose_parent_is_only_in_in_reply_to_still_has_a_parent() {
    let email = Email {
        in_reply_to: Some(vec!["b@example.com".to_owned()]),
        references: Some(vec!["a@example.com".to_owned()]),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(
        summary.references,
        vec!["a@example.com".to_owned(), "b@example.com".to_owned()],
        "the parent belongs at the end of the chain"
    );
}

#[test]
fn a_message_id_header_with_several_ids_keeps_the_first() {
    let email = Email {
        message_id: Some(vec![
            "first@example.com".to_owned(),
            "second@invalid".to_owned(),
        ]),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(summary.message_id.as_deref(), Some("first@example.com"));
}

#[test]
fn an_empty_message_id_header_is_no_message_id() {
    let email = Email {
        message_id: Some(Vec::new()),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(summary.message_id, None);
}

#[test]
fn a_size_beyond_camels_counter_saturates_rather_than_wraps() {
    let email = Email {
        size: Some(u64::from(u32::MAX) + 1),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(
        summary.size,
        u32::MAX,
        "a size that wraps reads as a tiny message"
    );
}

#[test]
fn a_date_before_the_epoch_is_a_negative_time() {
    let email = Email {
        received_at: Some("1969-07-20T20:17:40Z".into()),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(summary.received_at, Some(-14_182_940));
}

#[test]
fn a_fractional_second_is_dropped_rather_than_rejected() {
    let email = Email {
        received_at: Some("2026-01-15T09:30:00.512Z".into()),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary");

    assert_eq!(summary.received_at, Some(1_768_469_400));
}

#[test]
fn a_date_the_client_cannot_read_leaves_the_message_dateless() {
    for unreadable in [
        "yesterday",
        "2026-01-15",
        "2026-01-15T09:30:00",
        "2026-13-15T09:30:00Z",
        "2026-02-30T09:30:00Z",
        "2026-01-15T25:30:00Z",
        "2026-01-15T09:30:00+0X:00",
    ] {
        let email = Email {
            received_at: Some(unreadable.into()),
            subject: Some("Unreadable date".to_owned()),
            ..bare_email()
        };

        let summary = MessageSummary::from_email(&email)
            .expect("a message with a broken date is still a message");

        assert_eq!(
            summary.received_at, None,
            "{unreadable} should not have parsed"
        );
        assert_eq!(summary.subject.as_deref(), Some("Unreadable date"));
    }
}
