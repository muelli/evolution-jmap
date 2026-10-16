// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::error::SetError;
use jmap_proto::mail::*;
use jmap_proto::methods::{Filter, SetRequest};
use jmap_proto::state::{State, UtcDate};
use serde_json::{Value, json};

#[test]
fn mailbox_builders_and_properties() {
    let mut share_with = BTreeMap::new();
    let principal = Id::from("p1");
    let rights = MailboxRights::all();
    share_with.insert(principal.clone(), rights.clone());

    let mailbox = Mailbox::new("Inbox")
        .with_parent_id("parent-1")
        .with_role(role::INBOX)
        .with_sort_order(10)
        .is_subscribed(true)
        .with_share_with(share_with.clone());

    assert_eq!(mailbox.name, "Inbox");
    assert_eq!(mailbox.parent_id, Some(Id::from("parent-1")));
    assert_eq!(mailbox.role, Some("inbox".to_owned()));
    assert_eq!(mailbox.sort_order, Some(10));
    assert_eq!(mailbox.is_subscribed, Some(true));
    assert_eq!(mailbox.share_with, Some(share_with));

    // with_shared_principal inserts into an existing or new share_with map
    let mailbox2 = Mailbox::new("Archive").with_shared_principal("p2", MailboxRights::read_only());
    assert!(mailbox2.share_with.is_some());
    assert_eq!(
        mailbox2.share_with.as_ref().unwrap().get(&Id::from("p2")),
        Some(&MailboxRights::read_only())
    );

    // Serialization roundtrip
    let json_val = serde_json::to_value(&mailbox).unwrap();
    assert_eq!(json_val["name"], "Inbox");
    assert_eq!(json_val["parentId"], "parent-1");
    assert_eq!(json_val["role"], "inbox");
    assert_eq!(json_val["sortOrder"], 10);
    assert_eq!(json_val["isSubscribed"], true);
}

#[test]
fn mail_share_capability_builders() {
    let mut extra = BTreeMap::new();
    extra.insert("customField".to_owned(), json!(true));

    let cap = MailShareCapability::new().with_extra(extra.clone());
    assert_eq!(cap.extra, extra);

    let json_val = serde_json::to_value(&cap).unwrap();
    assert_eq!(json_val["customField"], true);
    let deserialized: MailShareCapability = serde_json::from_value(json_val).unwrap();
    assert_eq!(deserialized, cap);
}

#[test]
fn mailbox_rights_presets_and_permissions() {
    let all = MailboxRights::all();
    assert_eq!(all.may_read_items, Some(true));
    assert_eq!(all.may_add_items, Some(true));
    assert_eq!(all.may_remove_items, Some(true));
    assert_eq!(all.may_set_seen, Some(true));
    assert_eq!(all.may_set_keywords, Some(true));
    assert_eq!(all.may_create_child, Some(true));
    assert_eq!(all.may_rename, Some(true));
    assert_eq!(all.may_delete, Some(true));
    assert_eq!(all.may_submit, Some(true));
    assert_eq!(all.may_share, Some(true));
    assert!(all.may_share());

    let ro = MailboxRights::read_only();
    assert_eq!(ro.may_read_items, Some(true));
    assert_eq!(ro.may_add_items, None);
    assert_eq!(ro.may_remove_items, None);
    assert_eq!(ro.may_set_seen, None);
    assert_eq!(ro.may_set_keywords, None);
    assert_eq!(ro.may_create_child, None);
    assert_eq!(ro.may_rename, None);
    assert_eq!(ro.may_delete, None);
    assert_eq!(ro.may_submit, None);
    assert_eq!(ro.may_share, None);
    assert!(!ro.may_share());

    let default_rights = MailboxRights::default();
    assert_eq!(default_rights.may_share, None);
    assert!(!default_rights.may_share());

    let with_share_false = MailboxRights::default().with_may_share(false);
    assert_eq!(with_share_false.may_share, Some(false));
    assert!(!with_share_false.may_share());

    let with_share_true = MailboxRights::default().with_may_share(true);
    assert_eq!(with_share_true.may_share, Some(true));
    assert!(with_share_true.may_share());
}

#[test]
fn thread_builders_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("custom".to_owned(), json!(123));

    let thread = Thread::new("t1", ["e1", "e2"])
        .with_id("t2")
        .with_email_ids(["e3", "e4"])
        .with_email_id("e5")
        .with_extra(extra.clone());

    assert_eq!(thread.id, Some(Id::from("t2")));
    assert_eq!(
        thread.email_ids,
        vec![Id::from("e3"), Id::from("e4"), Id::from("e5")]
    );
    assert_eq!(thread.extra, extra);

    let json_val = serde_json::to_value(&thread).unwrap();
    assert_eq!(json_val["id"], "t2");
    assert_eq!(json_val["custom"], 123);
    let deserialized: Thread = serde_json::from_value(json_val).unwrap();
    assert_eq!(deserialized, thread);
}

#[test]
fn vacation_response_builders_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customVacation".to_owned(), json!("val"));

    let from = UtcDate::from("2026-07-01T00:00:00Z");
    let to = UtcDate::from("2026-07-15T00:00:00Z");

    let vac = VacationResponse::new(false)
        .with_id("vac-1")
        .with_is_enabled(true)
        .with_from_date(from.clone())
        .with_to_date(to.clone())
        .with_subject("Out of office")
        .with_text_body("I am out of office until July 15.")
        .with_html_body("<p>I am out of office until July 15.</p>")
        .with_extra(extra.clone());

    assert_eq!(vac.id, Some(Id::from("vac-1")));
    assert!(vac.is_enabled);
    assert_eq!(vac.from_date, Some(from));
    assert_eq!(vac.to_date, Some(to));
    assert_eq!(vac.subject, Some("Out of office".to_owned()));
    assert_eq!(
        vac.text_body,
        Some("I am out of office until July 15.".to_owned())
    );
    assert_eq!(
        vac.html_body,
        Some("<p>I am out of office until July 15.</p>".to_owned())
    );
    assert_eq!(vac.extra, extra);

    let json_val = serde_json::to_value(&vac).unwrap();
    assert_eq!(json_val["id"], "vac-1");
    assert_eq!(json_val["isEnabled"], true);
    assert_eq!(json_val["subject"], "Out of office");
    assert_eq!(json_val["customVacation"], "val");
}

#[test]
fn vacation_response_capability_builders() {
    let mut extra = BTreeMap::new();
    extra.insert("vacCapExtra".to_owned(), json!(42));

    let cap = VacationResponseCapability::new().with_extra(extra.clone());
    assert_eq!(cap.extra, extra);

    let json_val = serde_json::to_value(&cap).unwrap();
    assert_eq!(json_val["vacCapExtra"], 42);
    let deserialized: VacationResponseCapability = serde_json::from_value(json_val).unwrap();
    assert_eq!(deserialized, cap);
}

#[test]
fn snooze_details_builders_and_properties() {
    let until = UtcDate::from("2026-10-20T09:00:00Z");
    let mut keywords = BTreeMap::new();
    keywords.insert("$flagged".to_owned(), true);

    let snooze = SnoozeDetails::new(until.clone())
        .with_move_to_mailbox_id("mbx-inbox")
        .with_set_keywords(keywords.clone());

    assert_eq!(snooze.until, until);
    assert_eq!(snooze.move_to_mailbox_id, Some(Id::from("mbx-inbox")));
    assert_eq!(snooze.set_keywords, Some(keywords));

    let json_val = serde_json::to_value(&snooze).unwrap();
    assert_eq!(json_val["until"], "2026-10-20T09:00:00Z");
    assert_eq!(json_val["moveToMailboxId"], "mbx-inbox");
    assert_eq!(json_val["setKeywords"]["$flagged"], true);
}

#[test]
fn email_builders_and_properties() {
    let received = UtcDate::from("2026-10-16T12:00:00Z");
    let sent_str = "2026-10-16T11:59:00Z";
    let mut mailbox_ids = BTreeMap::new();
    mailbox_ids.insert(Id::from("mbx-1"), true);
    let mut keywords = BTreeMap::new();
    keywords.insert("$seen".to_owned(), true);

    let from_addr = EmailAddress::from_email("alice@example.com").with_name("Alice");
    let to_addr = EmailAddress::from_email("bob@example.com").with_name("Bob");
    let cc_addr = EmailAddress::from_email("carol@example.com");
    let bcc_addr = EmailAddress::from_email("dave@example.com");
    let reply_to_addr = EmailAddress::from_email("support@example.com");

    let header1 = EmailHeader::new("X-Custom", "Val1");
    let header2 = EmailHeader::new("X-Priority", "1");

    let body_part = EmailBodyPart::default()
        .with_part_id("1")
        .with_content_type("text/plain");

    let mut body_values = BTreeMap::new();
    body_values.insert("1".to_owned(), EmailBodyValue::new("Body text"));

    let attachment = EmailBodyPart::default()
        .with_part_id("2")
        .with_content_type("application/pdf")
        .with_size(1024);

    let verified_at = UtcDate::from("2026-10-16T12:01:00Z");

    let email = Email::default()
        .with_id("msg-1")
        .with_blob_id("blob-1")
        .with_thread_id("th-1")
        .in_mailbox("mbx-1")
        .with_mailbox_ids(mailbox_ids.clone())
        .with_keyword("$seen")
        .with_keywords(keywords.clone())
        .with_size(2048)
        .with_received_at(received.clone())
        .with_from([from_addr.clone()])
        .with_to([to_addr.clone()])
        .with_cc([cc_addr.clone()])
        .with_bcc([bcc_addr.clone()])
        .with_reply_to([reply_to_addr.clone()])
        .with_subject("Test Subject")
        .with_sent_at(sent_str)
        .with_preview("Preview snippet...")
        .has_attachment(true)
        .with_headers([header1.clone()])
        .with_header("X-Priority", "1")
        .with_body_structure(body_part.clone())
        .with_body_values(body_values.clone())
        .with_text_body([body_part.clone()])
        .with_html_body([body_part.clone()])
        .with_attachments([attachment.clone()])
        .with_smime_status(smime_status::SIGNED)
        .with_smime_errors(["unknownCa"])
        .with_smime_verified_at(verified_at.clone());

    assert_eq!(email.id, Some(Id::from("msg-1")));
    assert_eq!(email.blob_id, Some(Id::from("blob-1")));
    assert_eq!(email.thread_id, Some(Id::from("th-1")));
    assert_eq!(email.mailbox_ids, Some(mailbox_ids));
    assert_eq!(email.keywords, Some(keywords));
    assert_eq!(email.size, Some(2048));
    assert_eq!(email.received_at, Some(received));
    assert_eq!(email.from, Some(vec![from_addr]));
    assert_eq!(email.to, Some(vec![to_addr]));
    assert_eq!(email.cc, Some(vec![cc_addr]));
    assert_eq!(email.bcc, Some(vec![bcc_addr]));
    assert_eq!(email.reply_to, Some(vec![reply_to_addr]));
    assert_eq!(email.subject, Some("Test Subject".to_owned()));
    assert_eq!(email.sent_at, Some(sent_str.to_owned()));
    assert_eq!(email.preview, Some("Preview snippet...".to_owned()));
    assert_eq!(email.has_attachment, Some(true));
    assert_eq!(email.headers, Some(vec![header1, header2]));
    assert_eq!(email.body_structure, Some(body_part.clone()));
    assert_eq!(email.body_values, Some(body_values));
    assert_eq!(email.text_body, Some(vec![body_part.clone()]));
    assert_eq!(email.html_body, Some(vec![body_part]));
    assert_eq!(email.attachments, Some(vec![attachment]));
    assert_eq!(email.smime_status, Some("signed".to_owned()));
    assert_eq!(email.smime_errors, Some(vec!["unknownCa".to_owned()]));
    assert_eq!(email.smime_verified_at, Some(verified_at));
}

#[test]
fn email_import_builders() {
    let received = UtcDate::from("2026-10-16T12:00:00Z");
    let import = EmailImport::new("blob-99", "mbx-1")
        .keyword("$flagged")
        .received_at(received.clone());

    assert_eq!(import.blob_id, Some(Id::from("blob-99")));
    let mut expected_mbx = BTreeMap::new();
    expected_mbx.insert(Id::from("mbx-1"), true);
    assert_eq!(import.mailbox_ids, Some(expected_mbx));
    let mut expected_kw = BTreeMap::new();
    expected_kw.insert("$flagged".to_owned(), true);
    assert_eq!(import.keywords, Some(expected_kw));
    assert_eq!(import.received_at, Some(received));

    let req = EmailImportRequest::new("acc-1")
        .import("c1", import.clone())
        .if_in_state("state-123");

    assert_eq!(req.account_id, Id::from("acc-1"));
    assert_eq!(req.if_in_state, Some(State::from("state-123")));
    assert_eq!(req.emails.get("c1"), Some(&import));

    let mut created = BTreeMap::new();
    created.insert("c1".to_owned(), Email::default().with_id("msg-created"));
    let mut not_created = BTreeMap::new();
    not_created.insert("c2".to_owned(), SetError::new("notFound"));

    let resp = EmailImportResponse::new("acc-1", "state-new")
        .with_old_state("state-old")
        .with_created(created.clone())
        .with_not_created(not_created.clone());

    assert_eq!(resp.account_id, Id::from("acc-1"));
    assert_eq!(resp.new_state, State::from("state-new"));
    assert_eq!(resp.old_state, Some(State::from("state-old")));
    assert_eq!(resp.created, Some(created));
    assert_eq!(resp.not_created, Some(not_created));
}

#[test]
fn email_get_request_builders() {
    let req = EmailGetRequest::default()
        .with_account_id("acc-xyz")
        .with_ids(["msg-1", "msg-2"])
        .with_properties(["id", "subject"])
        .with_body_properties(["partId", "contentType"])
        .fetch_text_body_values()
        .fetch_html_body_values()
        .fetch_all_body_values()
        .with_max_body_value_bytes(4096);

    assert_eq!(req.account_id, Some(Id::from("acc-xyz")));
    assert_eq!(req.ids, Some(vec![Id::from("msg-1"), Id::from("msg-2")]));
    assert_eq!(
        req.properties,
        Some(vec!["id".to_owned(), "subject".to_owned()])
    );
    assert_eq!(
        req.body_properties,
        Some(vec!["partId".to_owned(), "contentType".to_owned()])
    );
    assert!(req.fetch_text_body_values);
    assert!(req.fetch_html_body_values);
    assert!(req.fetch_all_body_values);
    assert_eq!(req.max_body_value_bytes, Some(4096));
}

#[test]
fn email_parse_request_and_response_builders_and_is_false() {
    let req_default = EmailParseRequest::new("acc-1", ["blob-1"]);
    let json_default = serde_json::to_value(&req_default).unwrap();
    // Default false values MUST be skipped in JSON
    assert!(json_default.get("fetchTextBodyValues").is_none());
    assert!(json_default.get("fetchHTMLBodyValues").is_none());
    assert!(json_default.get("fetchAllBodyValues").is_none());

    let req_true = EmailParseRequest::new("acc-1", ["blob-1"])
        .properties(["id", "subject"])
        .body_properties(["partId"])
        .fetch_text_body_values()
        .fetch_html_body_values()
        .fetch_all_body_values()
        .max_body_value_bytes(8192);

    let json_true = serde_json::to_value(&req_true).unwrap();
    // When true, fields MUST be serialized
    assert_eq!(json_true["fetchTextBodyValues"], true);
    assert_eq!(json_true["fetchHTMLBodyValues"], true);
    assert_eq!(json_true["fetchAllBodyValues"], true);
    assert_eq!(json_true["maxBodyValueBytes"], 8192);

    let mut parsed = BTreeMap::new();
    parsed.insert(Id::from("blob-1"), Email::default().with_id("msg-1"));
    let not_parsable = vec![Id::from("blob-bad")];
    let not_found = vec![Id::from("blob-missing")];

    let resp = EmailParseResponse::new("acc-1")
        .with_parsed(parsed.clone())
        .with_not_parsable(not_parsable.clone())
        .with_not_found(not_found.clone());

    assert_eq!(resp.account_id, Id::from("acc-1"));
    assert_eq!(resp.parsed, Some(parsed));
    assert_eq!(resp.not_parsable, Some(not_parsable));
    assert_eq!(resp.not_found, Some(not_found));
}

#[test]
fn email_header_and_address_builders() {
    let header = EmailHeader::new("X-Test", "Val")
        .with_name("X-Name")
        .with_value("V2");
    assert_eq!(header.name, "X-Name");
    assert_eq!(header.value, "V2");

    let addr = EmailAddress::from_email("test@example.com")
        .with_name("Test User")
        .with_email("updated@example.com");
    assert_eq!(addr.name, Some("Test User".to_owned()));
    assert_eq!(addr.email, "updated@example.com");

    let group = EmailAddressGroup::new([addr.clone()])
        .with_name("Core Devs")
        .with_address(EmailAddress::from_email("extra@example.com"));

    assert_eq!(group.name, Some("Core Devs".to_owned()));
    assert_eq!(group.addresses.len(), 2);
    assert_eq!(group.addresses[0], addr);
}

#[test]
fn email_body_part_builders() {
    let verified_at = UtcDate::from("2026-10-16T12:00:00Z");
    let sub = EmailBodyPart::default().with_part_id("1.1");
    let header = EmailHeader::new("Content-Disposition", "inline");

    let part = EmailBodyPart::default()
        .with_part_id("1")
        .with_blob_id("blob-part")
        .with_size(1024)
        .with_name("file.txt")
        .with_content_type("text/plain")
        .with_charset("utf-8")
        .with_disposition("attachment")
        .with_cid("cid-123")
        .with_location("https://example.com/part")
        .with_sub_parts([sub.clone()])
        .with_headers([header.clone()])
        .with_smime_status(smime_status::SIGNED_VERIFIED)
        .with_smime_errors(["expiredCertificate"])
        .with_smime_verified_at(verified_at.clone());

    assert_eq!(part.part_id, Some("1".to_owned()));
    assert_eq!(part.blob_id, Some(Id::from("blob-part")));
    assert_eq!(part.size, Some(1024));
    assert_eq!(part.name, Some("file.txt".to_owned()));
    assert_eq!(part.content_type, Some("text/plain".to_owned()));
    assert_eq!(part.charset, Some("utf-8".to_owned()));
    assert_eq!(part.disposition, Some("attachment".to_owned()));
    assert_eq!(part.cid, Some("cid-123".to_owned()));
    assert_eq!(part.location, Some("https://example.com/part".to_owned()));
    assert_eq!(part.sub_parts, Some(vec![sub]));
    assert_eq!(part.headers, Some(vec![header]));
    assert_eq!(part.smime_status, Some("signed/verified".to_owned()));
    assert_eq!(
        part.smime_errors,
        Some(vec!["expiredCertificate".to_owned()])
    );
    assert_eq!(part.smime_verified_at, Some(verified_at));
}

#[test]
fn email_query_filter_builders() {
    let before = UtcDate::from("2026-10-16T12:00:00Z");
    let after = UtcDate::from("2026-10-01T00:00:00Z");

    let filter = EmailQueryFilter::in_mailbox("mbx-inbox")
        .in_mailbox_other_than(["mbx-trash"])
        .min_size(100)
        .max_size(100000)
        .has_keyword("$flagged")
        .not_keyword("$junk")
        .all_in_thread_have_keyword("$seen")
        .some_in_thread_have_keyword("$flagged")
        .none_in_thread_have_keyword("$junk")
        .has_attachment(true)
        .text("hello")
        .body("world")
        .from("alice@example.com")
        .to("bob@example.com")
        .cc("carol@example.com")
        .bcc("dave@example.com")
        .subject("urgent")
        .header("X-Spam", "no")
        .before(before.clone())
        .after(after.clone())
        .time_range(Some(after.clone()), Some(before.clone()))
        .with_has_smime(true)
        .with_has_verified_smime(true);

    assert_eq!(filter.in_mailbox, Some(Id::from("mbx-inbox")));
    assert_eq!(
        filter.in_mailbox_other_than,
        Some(vec![Id::from("mbx-trash")])
    );
    assert_eq!(filter.min_size, Some(100));
    assert_eq!(filter.max_size, Some(100000));
    assert_eq!(filter.has_keyword, Some("$flagged".to_owned()));
    assert_eq!(filter.not_keyword, Some("$junk".to_owned()));
    assert_eq!(filter.all_in_thread_have_keyword, Some("$seen".to_owned()));
    assert_eq!(
        filter.some_in_thread_have_keyword,
        Some("$flagged".to_owned())
    );
    assert_eq!(filter.none_in_thread_have_keyword, Some("$junk".to_owned()));
    assert_eq!(filter.has_attachment, Some(true));
    assert_eq!(filter.text, Some("hello".to_owned()));
    assert_eq!(filter.body, Some("world".to_owned()));
    assert_eq!(filter.from, Some("alice@example.com".to_owned()));
    assert_eq!(filter.to, Some("bob@example.com".to_owned()));
    assert_eq!(filter.cc, Some("carol@example.com".to_owned()));
    assert_eq!(filter.bcc, Some("dave@example.com".to_owned()));
    assert_eq!(filter.subject, Some("urgent".to_owned()));
    assert_eq!(
        filter.header,
        Some(vec!["X-Spam".to_owned(), "no".to_owned()])
    );
    assert_eq!(filter.before, Some(before));
    assert_eq!(filter.after, Some(after));
    assert_eq!(filter.has_smime, Some(true));
    assert_eq!(filter.has_verified_smime, Some(true));
}

#[test]
fn identity_builders_and_properties() {
    let reply_to = EmailAddress::from_email("reply@example.com");
    let bcc = EmailAddress::from_email("audit@example.com");

    let identity = Identity::new("Alice", "alice@example.com")
        .with_reply_to([reply_to.clone()])
        .with_bcc([bcc.clone()])
        .with_text_signature("Best regards,\nAlice")
        .with_html_signature("<p>Best regards,<br>Alice</p>")
        .with_draft_mailbox_id("mbx-drafts")
        .may_send(true);

    assert_eq!(identity.name, "Alice");
    assert_eq!(identity.email, "alice@example.com");
    assert_eq!(identity.reply_to, Some(vec![reply_to]));
    assert_eq!(identity.bcc, Some(vec![bcc]));
    assert_eq!(
        identity.text_signature,
        Some("Best regards,\nAlice".to_owned())
    );
    assert_eq!(
        identity.html_signature,
        Some("<p>Best regards,<br>Alice</p>".to_owned())
    );
    assert_eq!(identity.draft_mailbox_id, Some(Id::from("mbx-drafts")));
    assert_eq!(identity.may_send, Some(true));
}

#[test]
fn email_submission_builders_and_delivery_status() {
    let env_addr = EnvelopeAddress::new("alice@example.com");
    let env = Envelope::new(env_addr.clone(), [env_addr]);

    let submission = EmailSubmission::new("id-1", "msg-1")
        .with_thread_id("th-1")
        .with_envelope(env.clone())
        .with_undo_status(undo_status::PENDING);

    assert_eq!(submission.identity_id, Id::from("id-1"));
    assert_eq!(submission.email_id, Id::from("msg-1"));
    assert_eq!(submission.thread_id, Some(Id::from("th-1")));
    assert_eq!(submission.envelope, Some(env));
    assert_eq!(submission.undo_status, Some("pending".to_owned()));

    // DeliveryStatus defaults
    let status_default = DeliveryStatus::default();
    assert_eq!(status_default.delivered, delivered::UNKNOWN);
    assert_eq!(status_default.displayed, displayed::UNKNOWN);
    assert_eq!(status_default.delivered, "unknown");
    assert_eq!(status_default.displayed, "unknown");

    let status_new = DeliveryStatus::new("250 2.1.5 Ok");
    assert_eq!(status_new.smtp_reply, "250 2.1.5 Ok");
    assert_eq!(status_new.delivered, delivered::UNKNOWN);
    assert_eq!(status_new.displayed, displayed::UNKNOWN);

    // Deserialization of DeliveryStatus without delivered/displayed
    let json_val = json!({"smtpReply": "550 User unknown"});
    let deserialized: DeliveryStatus = serde_json::from_value(json_val).unwrap();
    assert_eq!(deserialized.delivered, delivered::UNKNOWN);
    assert_eq!(deserialized.displayed, displayed::UNKNOWN);

    let mut extra = BTreeMap::new();
    extra.insert("ext".to_owned(), json!(1));
    let status_custom = DeliveryStatus::new("250 OK")
        .with_smtp_reply("250 Sent")
        .with_delivered(delivered::YES)
        .with_displayed(displayed::YES)
        .with_extra(extra.clone());

    assert_eq!(status_custom.smtp_reply, "250 Sent");
    assert_eq!(status_custom.delivered, "yes");
    assert_eq!(status_custom.displayed, "yes");
    assert_eq!(status_custom.extra, extra);
}

#[test]
fn email_submission_query_filter_builders() {
    let before = UtcDate::from("2026-10-16T12:00:00Z");
    let after = UtcDate::from("2026-10-01T00:00:00Z");

    let filter = EmailSubmissionQueryFilter::default()
        .with_identity_ids(["id-1"])
        .with_email_ids(["msg-1"])
        .with_thread_ids(["th-1"])
        .with_undo_status(undo_status::FINAL)
        .before(before.clone())
        .after(after.clone())
        .time_range(Some(after.clone()), Some(before.clone()));

    assert_eq!(filter.identity_ids, Some(vec![Id::from("id-1")]));
    assert_eq!(filter.email_ids, Some(vec![Id::from("msg-1")]));
    assert_eq!(filter.thread_ids, Some(vec![Id::from("th-1")]));
    assert_eq!(filter.undo_status, Some("final".to_owned()));
    assert_eq!(filter.before, Some(before));
    assert_eq!(filter.after, Some(after));
}

#[test]
fn search_snippet_and_request_builders() {
    let snippet = SearchSnippet::new("msg-1")
        .with_subject("Found Subject")
        .with_preview("Snippet preview text");

    assert_eq!(snippet.email_id, Id::from("msg-1"));
    assert_eq!(snippet.subject, Some("Found Subject".to_owned()));
    assert_eq!(snippet.preview, Some("Snippet preview text".to_owned()));

    let filter = EmailQueryFilter::in_mailbox("mbx-inbox");
    let req = SearchSnippetGetRequest::new("acc-1", ["msg-1"])
        .with_filter(Filter::Condition(filter.clone()));

    assert_eq!(req.account_id, Id::from("acc-1"));
    assert_eq!(req.email_ids, vec![Id::from("msg-1")]);
    assert_eq!(req.filter, Some(Filter::Condition(filter)));

    let resp = SearchSnippetGetResponse::new("acc-1", [snippet]).with_not_found(["msg-99"]);
    assert_eq!(resp.account_id, Id::from("acc-1"));
    assert_eq!(resp.not_found, Some(vec![Id::from("msg-99")]));
}

#[test]
fn email_submission_set_request_builders() {
    let mut update_map = BTreeMap::new();
    update_map.insert("c1".to_owned(), json!({"keywords/$draft": null}));

    let set = SetRequest::<EmailSubmission>::new("acc-1");
    let req = EmailSubmissionSetRequest::new(set)
        .with_on_success_update_email(update_map.clone())
        .with_on_success_destroy_email(["msg-old"]);

    assert_eq!(req.set.account_id, Id::from("acc-1"));
    assert_eq!(req.on_success_update_email, Some(update_map));
    assert_eq!(
        req.on_success_destroy_email,
        Some(vec!["msg-old".to_owned()])
    );
}

#[test]
fn envelope_schedule_and_parameters() {
    let hold_for = Schedule::HoldFor(600);
    let (name1, val1) = hold_for.parameter();
    assert_eq!(name1, "HOLDFOR");
    assert_eq!(val1, Value::String("600".to_owned()));

    let until_date = UtcDate::from("2026-10-20T12:00:00Z");
    let hold_until = Schedule::HoldUntil(until_date.clone());
    let (name2, val2) = hold_until.parameter();
    assert_eq!(name2, "HOLDUNTIL");
    assert_eq!(val2, Value::String("2026-10-20T12:00:00Z".to_owned()));

    // Envelope builders
    let mut params = serde_json::Map::new();
    params.insert("BODY".to_owned(), json!("8BITMIME"));
    let mail_from =
        EnvelopeAddress::new("sender@example.com").with_parameters(Value::Object(params));

    let rcpt1 = EnvelopeAddress::new("r1@example.com");
    let rcpt2 = EnvelopeAddress::new("r2@example.com");

    let env = Envelope::new(mail_from.clone(), [rcpt1.clone()])
        .with_mail_from(mail_from.clone())
        .with_rcpt_to([rcpt1.clone()])
        .with_recipient(rcpt2.clone());

    assert_eq!(env.mail_from, mail_from);
    assert_eq!(env.rcpt_to, vec![rcpt1, rcpt2]);

    // with_schedule preserves existing parameters in mail_from
    let env_with_sched = env.with_schedule(&hold_for);
    let sched_params = env_with_sched.mail_from.parameters.unwrap();
    assert_eq!(sched_params["BODY"], "8BITMIME");
    assert_eq!(sched_params["HOLDFOR"], "600");

    // with_schedule when parameters was None
    let env_no_params = Envelope::new(
        EnvelopeAddress::new("s@ex.com"),
        [EnvelopeAddress::new("r@ex.com")],
    )
    .with_schedule(&hold_until);
    let sched_params2 = env_no_params.mail_from.parameters.unwrap();
    assert_eq!(sched_params2["HOLDUNTIL"], "2026-10-20T12:00:00Z");
}

#[test]
fn mail_capability_and_submission_capability() {
    let mut extra = BTreeMap::new();
    extra.insert("customExt".to_owned(), json!(true));

    let mail_cap = MailCapability::default()
        .with_max_size_attachments_per_email(50_000_000)
        .with_max_size_email_in_bytes(100_000_000)
        .with_max_size_body_value_bytes(2_000_000)
        .with_max_number_of_attachments_per_email(50)
        .with_max_number_of_recipients_per_email(100)
        .may_create_top_level_mailbox(true)
        .with_extra(extra.clone());

    assert_eq!(mail_cap.max_size_attachments_per_email, 50_000_000);
    assert_eq!(mail_cap.max_size_email_in_bytes, 100_000_000);
    assert_eq!(mail_cap.max_size_body_value_bytes, 2_000_000);
    assert_eq!(mail_cap.max_number_of_attachments_per_email, 50);
    assert_eq!(mail_cap.max_number_of_recipients_per_email, 100);
    assert!(mail_cap.may_create_top_level_mailbox);
    assert_eq!(mail_cap.extra, extra);

    let mut extensions = BTreeMap::new();
    extensions.insert("DSN".to_owned(), vec![]);
    let sub_cap_no_fr = SubmissionCapability::default()
        .with_max_delayed_send(300)
        .with_submission_extensions(extensions.clone())
        .with_extra(extra.clone());

    assert_eq!(sub_cap_no_fr.max_delayed_send, 300);
    assert_eq!(sub_cap_no_fr.submission_extensions, extensions);
    assert_eq!(sub_cap_no_fr.extra, extra);
    assert!(!sub_cap_no_fr.supports_future_release());

    // Case-insensitive FUTURERELEASE detection
    let mut extensions_fr_lower = BTreeMap::new();
    extensions_fr_lower.insert("futurerelease".to_owned(), vec!["300".to_owned()]);
    let sub_cap_lower =
        SubmissionCapability::default().with_submission_extensions(extensions_fr_lower);
    assert!(sub_cap_lower.supports_future_release());

    let mut extensions_fr_upper = BTreeMap::new();
    extensions_fr_upper.insert("FUTURERELEASE".to_owned(), vec![]);
    let sub_cap_upper =
        SubmissionCapability::default().with_submission_extensions(extensions_fr_upper);
    assert!(sub_cap_upper.supports_future_release());
}

#[test]
fn mdn_disposition_and_send_request_response() {
    let mut extra = BTreeMap::new();
    extra.insert("mdnExtra".to_owned(), json!("test"));

    let disp = MDNDisposition::new(
        mdn_action_mode::MANUAL_ACTION,
        mdn_sending_mode::MDN_SENT_MANUALLY,
        mdn_disposition_type::DISPLAYED,
    )
    .with_error("error details")
    .with_modifiers(["warning"])
    .with_extra(extra.clone());

    assert_eq!(disp.action_mode, "manual-action");
    assert_eq!(disp.sending_mode, "mdn-sent-manually");
    assert_eq!(disp.disposition_type, "displayed");
    assert_eq!(disp.error, Some("error details".to_owned()));
    assert_eq!(disp.modifiers, Some(vec!["warning".to_owned()]));
    assert_eq!(disp.extra, extra);

    let mut mdn_ext = BTreeMap::new();
    mdn_ext.insert("X-Custom-MDN".to_owned(), "ext-val".to_owned());

    let mdn = MDN::new("msg-1", disp.clone())
        .with_subject("Read: Important Notice")
        .with_text_body("Your message was displayed.")
        .with_include_original_message(true)
        .with_reporting_ua("Evolution/3.50")
        .with_final_recipient("alice@example.com")
        .with_original_message_id("<msg-123@example.com>")
        .with_mdn_gateway("mail.example.com")
        .with_original_recipient("alice-orig@example.com")
        .with_error(["none"])
        .with_extension_fields(mdn_ext.clone())
        .with_extra(extra.clone());

    assert_eq!(mdn.for_email_id, Id::from("msg-1"));
    assert_eq!(mdn.subject, Some("Read: Important Notice".to_owned()));
    assert_eq!(
        mdn.text_body,
        Some("Your message was displayed.".to_owned())
    );
    assert_eq!(mdn.include_original_message, Some(true));
    assert_eq!(mdn.reporting_ua, Some("Evolution/3.50".to_owned()));
    assert_eq!(mdn.final_recipient, Some("alice@example.com".to_owned()));
    assert_eq!(
        mdn.original_message_id,
        Some("<msg-123@example.com>".to_owned())
    );
    assert_eq!(mdn.mdn_gateway, Some("mail.example.com".to_owned()));
    assert_eq!(
        mdn.original_recipient,
        Some("alice-orig@example.com".to_owned())
    );
    assert_eq!(mdn.error, Some(vec!["none".to_owned()]));
    assert_eq!(mdn.extension_fields, Some(mdn_ext));
    assert_eq!(mdn.extra, extra);

    let mut send_map = BTreeMap::new();
    send_map.insert("m1".to_owned(), mdn.clone());
    let mut update_map = BTreeMap::new();
    update_map.insert(Id::from("msg-1"), json!({"keywords/$mdnsent": true}));

    let send_req = MDNSendRequest::new("acc-1", send_map.clone())
        .with_on_success_update_email(update_map.clone());

    assert_eq!(send_req.account_id, Id::from("acc-1"));
    assert_eq!(send_req.send, send_map);
    assert_eq!(send_req.on_success_update_email, Some(update_map));

    let mut sent_map = BTreeMap::new();
    sent_map.insert("m1".to_owned(), mdn);
    let mut not_sent_map = BTreeMap::new();
    not_sent_map.insert("m2".to_owned(), SetError::new("notFound"));

    let send_resp = MDNSendResponse::new("acc-1")
        .with_sent(sent_map.clone())
        .with_not_sent(not_sent_map.clone());

    assert_eq!(send_resp.account_id, Id::from("acc-1"));
    assert_eq!(send_resp.sent, Some(sent_map));
    assert_eq!(send_resp.not_sent, Some(not_sent_map));

    let mdn_cap = MDNCapability::default().with_extra(json!({"ext": true}));
    assert_eq!(mdn_cap.extra["ext"], true);

    let smime_cap = SmimeVerifyCapability::default().with_extra(json!({"ext": true}));
    assert_eq!(smime_cap.extra["ext"], true);
}

#[test]
fn mail_constants_verification() {
    assert_eq!(mailbox_set_error::HAS_CHILD, "mailboxHasChild");
    assert_eq!(mailbox_set_error::HAS_EMAIL, "mailboxHasEmail");

    assert_eq!(role::ALL, "all");
    assert_eq!(role::ARCHIVE, "archive");
    assert_eq!(role::DRAFTS, "drafts");
    assert_eq!(role::FLAGGED, "flagged");
    assert_eq!(role::IMPORTANT, "important");
    assert_eq!(role::INBOX, "inbox");
    assert_eq!(role::JUNK, "junk");
    assert_eq!(role::SCHEDULED, "scheduled");
    assert_eq!(role::SENT, "sent");
    assert_eq!(role::SNOOZED, "snoozed");
    assert_eq!(role::SUBSCRIBED, "subscribed");
    assert_eq!(role::TRASH, "trash");

    assert_eq!(email_set_error::BLOB_NOT_FOUND, "blobNotFound");
    assert_eq!(email_set_error::TOO_MANY_KEYWORDS, "tooManyKeywords");
    assert_eq!(email_set_error::TOO_MANY_MAILBOXES, "tooManyMailboxes");

    assert_eq!(
        email_submission_set_error::TOO_MANY_RECIPIENTS,
        "tooManyRecipients"
    );
    assert_eq!(
        email_submission_set_error::INVALID_RECIPIENTS,
        "invalidRecipients"
    );
    assert_eq!(email_submission_set_error::CANNOT_UNSEND, "cannotUnsend");
    assert_eq!(email_submission_set_error::NO_RECIPIENTS, "noRecipients");
    assert_eq!(
        email_submission_set_error::FORBIDDEN_MAIL_FROM,
        "forbiddenMailFrom"
    );
    assert_eq!(email_submission_set_error::FORBIDDEN_FROM, "forbiddenFrom");
    assert_eq!(
        email_submission_set_error::FORBIDDEN_TO_SEND,
        "forbiddenToSend"
    );

    assert_eq!(identity_set_error::FORBIDDEN_FROM, "forbiddenFrom");
    assert_eq!(
        identity_set_error::CANNOT_DESTROY_DEFAULT,
        "cannotDestroyDefault"
    );

    assert_eq!(mdn_set_error::MDN_ALREADY_SENT, "mdnAlreadySent");
    assert_eq!(mdn_set_error::FORBIDDEN_FROM, "forbiddenFrom");

    assert_eq!(undo_status::PENDING, "pending");
    assert_eq!(undo_status::FINAL, "final");
    assert_eq!(undo_status::CANCELED, "canceled");

    assert_eq!(delivered::UNKNOWN, "unknown");
    assert_eq!(delivered::YES, "yes");
    assert_eq!(delivered::NO, "no");

    assert_eq!(displayed::UNKNOWN, "unknown");
    assert_eq!(displayed::YES, "yes");

    assert_eq!(smime_status::UNKNOWN, "unknown");
    assert_eq!(smime_status::SIGNED, "signed");
    assert_eq!(smime_status::SIGNED_VERIFIED, "signed/verified");
    assert_eq!(smime_status::SIGNED_FAILED, "signed/failed");

    assert_eq!(mdn_action_mode::MANUAL_ACTION, "manual-action");
    assert_eq!(mdn_action_mode::AUTOMATIC_ACTION, "automatic-action");

    assert_eq!(mdn_sending_mode::MDN_SENT_MANUALLY, "mdn-sent-manually");
    assert_eq!(
        mdn_sending_mode::MDN_SENT_AUTOMATICALLY,
        "mdn-sent-automatically"
    );

    assert_eq!(mdn_disposition_type::DISPLAYED, "displayed");
    assert_eq!(mdn_disposition_type::DELETED, "deleted");
    assert_eq!(mdn_disposition_type::DISPATCHED, "dispatched");
    assert_eq!(mdn_disposition_type::PROCESSED, "processed");
}
