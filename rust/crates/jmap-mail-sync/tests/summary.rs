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

#[test]
fn multiple_references_and_in_reply_to_deduplicate_and_preserve_ancestry_order() {
    // 1. In-Reply-To already matching the last element of References is not duplicated
    let email_matched = Email {
        references: Some(vec![
            "root@example.com".to_owned(),
            "parent@example.com".to_owned(),
        ]),
        in_reply_to: Some(vec!["parent@example.com".to_owned()]),
        ..bare_email()
    };
    let summary_matched =
        MessageSummary::from_email(&email_matched).expect("a summary with matched in-reply-to");
    assert_eq!(
        summary_matched.references,
        vec![
            "root@example.com".to_owned(),
            "parent@example.com".to_owned()
        ]
    );

    // 2. In-Reply-To differing from References is appended to form complete lineage
    let email_diff = Email {
        references: Some(vec!["root@example.com".to_owned()]),
        in_reply_to: Some(vec!["parent@example.com".to_owned()]),
        ..bare_email()
    };
    let summary_diff =
        MessageSummary::from_email(&email_diff).expect("a summary with distinct in-reply-to");
    assert_eq!(
        summary_diff.references,
        vec![
            "root@example.com".to_owned(),
            "parent@example.com".to_owned()
        ]
    );
}

#[test]
fn summary_maps_combined_flags_and_custom_tags_faithfully() {
    let email = Email {
        keywords: Some(
            [
                (keyword::SEEN.to_owned(), true),
                (keyword::FLAGGED.to_owned(), true),
                (keyword::ANSWERED.to_owned(), true),
                (keyword::DRAFT.to_owned(), false),
                ("$custom_label".to_owned(), true),
                ("work/urgent".to_owned(), true),
                ("cleared_tag".to_owned(), false),
            ]
            .into(),
        ),
        has_attachment: Some(true),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("a summary with mixed flags");
    assert!(summary.flags.seen);
    assert!(summary.flags.flagged);
    assert!(summary.flags.answered);
    assert!(!summary.flags.draft);
    assert!(summary.flags.attachments);
    assert_eq!(
        summary.tags,
        vec!["$custom_label".to_owned(), "work/urgent".to_owned()]
    );
}

#[test]
fn summary_with_attachments_and_forwarded_flag_retains_all_attributes() {
    let email = Email {
        id: Some(Id::new("M-ATTACH-99")),
        blob_id: Some(Id::new("B-ATTACH-99")),
        thread_id: Some(Id::new("T-ATTACH-99")),
        size: Some(1048576),
        subject: Some("Quarterly Results Presentation".to_owned()),
        has_attachment: Some(true),
        keywords: Some(
            [
                (keyword::FORWARDED.to_owned(), true),
                (keyword::SEEN.to_owned(), true),
                ("finance".to_owned(), true),
            ]
            .into(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary with attachment");
    assert_eq!(summary.uid, Id::new("M-ATTACH-99"));
    assert_eq!(summary.size, 1048576);
    assert_eq!(
        summary.subject.as_deref(),
        Some("Quarterly Results Presentation")
    );
    assert!(summary.flags.attachments);
    assert!(summary.flags.forwarded);
    assert!(summary.flags.seen);
    assert!(!summary.flags.flagged);
    assert_eq!(summary.tags, vec!["finance".to_owned()]);
}

#[test]
fn summary_extracts_structured_addresses_and_message_id_cleanly() {
    let email = Email {
        id: Some(Id::new("M-ADDR-01")),
        blob_id: Some(Id::new("B-ADDR-01")),
        subject: Some("Syncing addresses and IDs".to_owned()),
        message_id: Some(vec!["<alpha-beta-123@example.com>".to_owned()]),
        from: Some(vec![EmailAddress::new(
            Some("Alice Sender"),
            "alice@example.com",
        )]),
        to: Some(vec![
            EmailAddress::new(Some("Bob Recipient"), "bob@example.com"),
            EmailAddress::new(None, "carol@example.com"),
        ]),
        cc: Some(vec![EmailAddress::new(
            Some("Dev Team"),
            "devs@example.com",
        )]),
        preview: Some("Brief summary snippet".to_owned()),
        sent_at: Some("2026-01-15T10:30:00+01:00".into()),
        received_at: Some("2026-01-15T09:30:00Z".into()),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary from email with addresses");
    assert_eq!(
        summary.message_id.as_deref(),
        Some("<alpha-beta-123@example.com>")
    );
    assert_eq!(summary.from.len(), 1);
    assert_eq!(summary.from[0].name.as_deref(), Some("Alice Sender"));
    assert_eq!(summary.from[0].email, "alice@example.com");

    assert_eq!(summary.to.len(), 2);
    assert_eq!(summary.to[0].name.as_deref(), Some("Bob Recipient"));
    assert_eq!(summary.to[0].email, "bob@example.com");
    assert_eq!(summary.to[1].name, None);
    assert_eq!(summary.to[1].email, "carol@example.com");

    assert_eq!(summary.cc.len(), 1);
    assert_eq!(summary.cc[0].email, "devs@example.com");

    assert_eq!(summary.preview.as_deref(), Some("Brief summary snippet"));
    // Both timestamps point to the same instant 1768469400
    assert_eq!(summary.sent_at, Some(1_768_469_400));
    assert_eq!(summary.received_at, Some(1_768_469_400));
}

#[test]
fn summary_handles_unicode_preview_and_subject_with_accents_and_emojis() {
    let email = Email {
        id: Some(Id::new("M-UNICODE-88")),
        blob_id: Some(Id::new("B-UNICODE-88")),
        subject: Some("JMAP Überprüfung 🚀 — 会議の招待".to_owned()),
        preview: Some("Wichtige Aktualisierung für das Projekt: Bitte prüfen 👍".to_owned()),
        from: Some(vec![EmailAddress::new(
            Some("Müller & 佐藤"),
            "team@example.com",
        )]),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary with unicode fields");
    assert_eq!(
        summary.subject.as_deref(),
        Some("JMAP Überprüfung 🚀 — 会議の招待")
    );
    assert_eq!(
        summary.preview.as_deref(),
        Some("Wichtige Aktualisierung für das Projekt: Bitte prüfen 👍")
    );
    assert_eq!(summary.from[0].name.as_deref(), Some("Müller & 佐藤"));
}

#[test]
fn summary_maps_various_user_tags_and_retains_case_sensitivities() {
    let email = Email {
        id: Some(Id::new("M-TAGS-77")),
        blob_id: Some(Id::new("B-TAGS-77")),
        keywords: Some(
            [
                (keyword::SEEN.to_owned(), true),
                (keyword::FLAGGED.to_owned(), false),
                (keyword::DRAFT.to_owned(), true),
                ("Project/Alpha".to_owned(), true),
                ("URGENT-REVIEW".to_owned(), true),
                ("Finance2026".to_owned(), true),
            ]
            .into(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary with multiple tags");
    assert!(summary.flags.seen);
    assert!(!summary.flags.flagged);
    assert!(summary.flags.draft);
    assert_eq!(summary.tags.len(), 3);
    assert!(summary.tags.contains(&"Project/Alpha".to_owned()));
    assert!(summary.tags.contains(&"URGENT-REVIEW".to_owned()));
    assert!(summary.tags.contains(&"Finance2026".to_owned()));
}

#[test]
fn summary_handles_html_entity_sequences_in_subject_and_preview() {
    let email = Email {
        id: Some(Id::new("M-ENTITY-55")),
        blob_id: Some(Id::new("B-ENTITY-55")),
        subject: Some("Invoice &amp; Statement #1234 &lt;Q1&gt;".to_owned()),
        preview: Some("Total amount due: &quot;$1,250.00&quot; &amp; fees".to_owned()),
        from: Some(vec![EmailAddress::new(
            Some("Billing &amp; Finance"),
            "billing@example.com",
        )]),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary with html entities");
    assert_eq!(
        summary.subject.as_deref(),
        Some("Invoice &amp; Statement #1234 &lt;Q1&gt;")
    );
    assert_eq!(
        summary.preview.as_deref(),
        Some("Total amount due: &quot;$1,250.00&quot; &amp; fees")
    );
    assert_eq!(
        summary.from[0].name.as_deref(),
        Some("Billing &amp; Finance")
    );
}

#[test]
fn summary_preserves_multiple_recipient_groups_and_custom_references() {
    let email = Email {
        id: Some(Id::new("M-RECIP-33")),
        blob_id: Some(Id::new("B-RECIP-33")),
        subject: Some("Group distribution update".to_owned()),
        from: Some(vec![
            EmailAddress::new(Some("Primary Sender"), "sender1@example.com"),
            EmailAddress::new(Some("Co-Sender"), "sender2@example.com"),
        ]),
        to: Some(vec![
            EmailAddress::new(Some("Alice Lead"), "alice@example.com"),
            EmailAddress::new(Some("Bob Reviewer"), "bob@example.com"),
        ]),
        cc: Some(vec![
            EmailAddress::new(Some("Carol Auditor"), "carol@example.com"),
            EmailAddress::new(None, "audit-archive@example.com"),
        ]),
        references: Some(vec![
            "<root-msg-001@example.com>".to_owned(),
            "<inter-msg-002@example.com>".to_owned(),
            "<prev-msg-003@example.com>".to_owned(),
        ]),
        in_reply_to: Some(vec!["<prev-msg-003@example.com>".to_owned()]),
        keywords: Some(
            [
                (keyword::SEEN.to_owned(), true),
                (keyword::FLAGGED.to_owned(), true),
                ("audit-complete".to_owned(), true),
            ]
            .into(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary with multiple recipients");
    assert_eq!(summary.from.len(), 2);
    assert_eq!(summary.to.len(), 2);
    assert_eq!(summary.cc.len(), 2);
    assert_eq!(summary.references.len(), 3);
    assert_eq!(
        summary.references[2],
        "<prev-msg-003@example.com>".to_owned()
    );
    assert!(summary.flags.seen);
    assert!(summary.flags.flagged);
    assert_eq!(summary.tags, vec!["audit-complete".to_owned()]);
}

#[test]
fn summary_handles_modified_utf7_tag_and_subject_strings() {
    let email = Email {
        id: Some(Id::new("M-UTF7-11")),
        blob_id: Some(Id::new("B-UTF7-11")),
        subject: Some("Ordner Entw&APw-rfe und Gel&APY-schte Elemente".to_owned()),
        preview: Some("Best&AOQ-tigung f&APw-r Benutzer".to_owned()),
        keywords: Some(
            [
                (keyword::SEEN.to_owned(), true),
                ("Posteingang/Privat & gesch&AOQ-ftlich".to_owned(), true),
            ]
            .into(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary with modified utf-7 patterns");
    assert_eq!(
        summary.subject.as_deref(),
        Some("Ordner Entw&APw-rfe und Gel&APY-schte Elemente")
    );
    assert_eq!(
        summary.preview.as_deref(),
        Some("Best&AOQ-tigung f&APw-r Benutzer")
    );
    assert_eq!(
        summary.tags,
        vec!["Posteingang/Privat & gesch&AOQ-ftlich".to_owned()]
    );
}

#[test]
fn summary_handles_mixed_case_standard_keywords_alongside_custom_tags() {
    let email = Email {
        id: Some(Id::new("M-MIX-KW-22")),
        blob_id: Some(Id::new("B-MIX-KW-22")),
        keywords: Some(
            [
                ("$SEEN".to_owned(), true),
                ("$Flagged".to_owned(), true),
                ("$ANSWERED".to_owned(), true),
                ("$dRaFt".to_owned(), true),
                ("$Forwarded".to_owned(), true),
                ("$Junk".to_owned(), true),
                ("$NotJunk".to_owned(), true),
                ("$MyCustomKeyword".to_owned(), true),
                ("category/inbox-2026".to_owned(), true),
            ]
            .into(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary with mixed-case keywords");
    assert!(summary.flags.seen);
    assert!(summary.flags.flagged);
    assert!(summary.flags.answered);
    assert!(summary.flags.draft);
    assert!(summary.flags.forwarded);
    assert!(summary.flags.junk);
    assert!(summary.flags.not_junk);
    assert_eq!(summary.tags.len(), 2);
    assert!(summary.tags.contains(&"$MyCustomKeyword".to_owned()));
    assert!(summary.tags.contains(&"category/inbox-2026".to_owned()));
}

#[test]
fn summary_normalizes_and_preserves_subject_with_nested_reply_prefixes() {
    let email = Email {
        id: Some(Id::new("M-SUBJ-PREFIX-01")),
        blob_id: Some(Id::new("B-SUBJ-PREFIX-01")),
        subject: Some("Re: Re[2]: Fwd: [Engineering] Q3 Roadmap Planning".to_owned()),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("summary with nested prefixes");
    assert_eq!(
        summary.subject.as_deref(),
        Some("Re: Re[2]: Fwd: [Engineering] Q3 Roadmap Planning")
    );
}

#[test]
fn summary_date_ordering_and_comparison_properties() {
    let email_earlier = Email {
        id: Some(Id::new("M-DATE-01")),
        blob_id: Some(Id::new("B-DATE-01")),
        received_at: Some("2026-08-15T00:00:00Z".into()),
        sent_at: Some("2026-08-14T23:59:50Z".into()),
        ..bare_email()
    };
    let email_later = Email {
        id: Some(Id::new("M-DATE-02")),
        blob_id: Some(Id::new("B-DATE-02")),
        received_at: Some("2026-08-15T01:00:00Z".into()),
        sent_at: Some("2026-08-15T00:59:50Z".into()),
        ..bare_email()
    };

    let s1 = MessageSummary::from_email(&email_earlier).expect("earlier summary");
    let s2 = MessageSummary::from_email(&email_later).expect("later summary");

    assert!(s1.received_at.unwrap() < s2.received_at.unwrap());
    assert!(s1.sent_at.unwrap() < s2.sent_at.unwrap());
}

/// `$junk` and `$notjunk` are distinct: both can be present simultaneously (a
/// server that sets `$junk` on a message and a local junk-filter that disagrees
/// by setting `$notjunk`).  The `MessageFlags` preserves both independently.
///
/// The `CAMEL_JUNK_STATUS` ordering (`MESSAGE_IS_JUNK = 2`,
/// `MESSAGE_IS_NOT_JUNK = 3`) is verified separately in
/// `eds-sys/tests/camel.rs::camel_junk_filter_interface_and_status_in_eds`;
/// this test confirms the `MessageSummary` layer correctly distinguishes the
/// two keywords and that neither implies the other.
#[test]
fn junk_and_not_junk_keywords_are_independently_tracked() {
    // Only $junk.
    let junk_email = Email {
        id: Some(Id::new("M-JUNK-01")),
        keywords: Some([(keyword::JUNK.to_owned(), true)].into_iter().collect()),
        ..bare_email()
    };
    let junk_summary = MessageSummary::from_email(&junk_email).expect("junk summary");
    assert!(
        junk_summary.flags.junk,
        "$junk keyword must set the junk flag"
    );
    assert!(
        !junk_summary.flags.not_junk,
        "$junk alone must not set the not_junk flag"
    );

    // Only $notjunk.
    let not_junk_email = Email {
        id: Some(Id::new("M-JUNK-02")),
        keywords: Some([(keyword::NOT_JUNK.to_owned(), true)].into_iter().collect()),
        ..bare_email()
    };
    let not_junk_summary = MessageSummary::from_email(&not_junk_email).expect("not-junk summary");
    assert!(
        !not_junk_summary.flags.junk,
        "$notjunk alone must not set the junk flag"
    );
    assert!(
        not_junk_summary.flags.not_junk,
        "$notjunk keyword must set the not_junk flag"
    );

    // Both simultaneously: a server verdict ($junk) the user overrode ($notjunk).
    let both_email = Email {
        id: Some(Id::new("M-JUNK-03")),
        keywords: Some(
            [
                (keyword::JUNK.to_owned(), true),
                (keyword::NOT_JUNK.to_owned(), true),
            ]
            .into_iter()
            .collect(),
        ),
        ..bare_email()
    };
    let both_summary = MessageSummary::from_email(&both_email).expect("both-flags summary");
    assert!(both_summary.flags.junk, "both: junk flag must be set");
    assert!(
        both_summary.flags.not_junk,
        "both: not_junk flag must be set"
    );
}

/// The `subject` and `preview` fields of a `MessageSummary` are the strings a
/// full-text indexer (e.g. `camel_index_name_add_buffer`, verified in
/// `eds-sys/tests/camel.rs::camel_text_index_creation_and_words_in_eds`) would
/// receive to index a message's searchable text content.  Verifies they round-
/// trip cleanly, including Unicode, and are non-empty when the server sends them.
#[test]
fn subject_and_preview_provide_indexable_text() {
    let email = Email {
        id: Some(Id::new("M-IDX-01")),
        subject: Some("Quarterly review: JMAP sync performance 🚀".to_owned()),
        preview: Some(
            "Please review the attached metrics before the meeting on Friday.".to_owned(),
        ),
        ..bare_email()
    };

    let summary = MessageSummary::from_email(&email).expect("indexable summary");

    let subject = summary.subject.as_deref().expect("subject must be present");
    let preview = summary.preview.as_deref().expect("preview must be present");

    // Both fields are non-empty — something a CamelTextIndex would tokenise.
    assert!(!subject.is_empty());
    assert!(!preview.is_empty());

    // Content is preserved exactly, including non-ASCII characters.
    assert!(
        subject.contains("JMAP"),
        "subject must contain 'JMAP'; got {subject:?}"
    );
    assert!(
        subject.contains('🚀'),
        "subject must preserve emoji; got {subject:?}"
    );
    assert!(
        preview.contains("Friday"),
        "preview must preserve its content; got {preview:?}"
    );

    // A message with no subject or preview produces `None` in those fields —
    // the indexer must not be handed an empty string as a document key.
    let bare = MessageSummary::from_email(&bare_email()).expect("bare summary");
    assert!(bare.subject.is_none());
    assert!(bare.preview.is_none());
}
