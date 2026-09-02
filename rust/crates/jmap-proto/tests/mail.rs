// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the RFC 8621 mail types.

#![cfg(feature = "mail")]

use jmap_proto::mail::{
    Email, EmailImport, EmailQueryFilter, EmailSubmission, Mailbox, MailboxRights,
};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn roundtrip<T>(value: &Value) -> Value
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let typed: T = serde_json::from_value(value.clone()).expect("deserialize");
    serde_json::to_value(&typed).expect("serialize")
}

#[test]
fn mailbox_roundtrip() {
    let value = fixture("mail/mailbox.json");
    assert_eq!(roundtrip::<Mailbox>(&value), value);

    let mailbox: Mailbox = serde_json::from_value(value).unwrap();
    assert_eq!(mailbox.name, "Urgent");
    assert_eq!(mailbox.role.as_deref(), Some("inbox"));
    assert_eq!(mailbox.total_emails, Some(2));
    assert_eq!(mailbox.unread_emails, Some(1));
}

#[test]
fn mailbox_my_rights_roundtrip() {
    let value = fixture("mail/mailbox_with_rights.json");
    assert_eq!(roundtrip::<Mailbox>(&value), value);

    let mailbox: Mailbox = serde_json::from_value(value).unwrap();
    let rights: MailboxRights = mailbox.my_rights.expect("myRights");
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_remove_items, Some(false));
    assert_eq!(rights.may_submit, Some(true));
}

#[test]
fn email_object_roundtrip() {
    let value = fixture("mail/email.json");
    assert_eq!(roundtrip::<Email>(&value), value);

    let email: Email = serde_json::from_value(value).unwrap();
    assert_eq!(email.subject.as_deref(), Some("Hello"));
    let from = email.from.as_ref().unwrap();
    assert_eq!(from[0].email, "bob@example.com");
    assert_eq!(from[0].name.as_deref(), Some("Bob"));

    // Body text is reachable through textBody's partId → bodyValues.
    let text_part = &email.text_body.as_ref().unwrap()[0];
    let part_id = text_part.part_id.as_ref().unwrap();
    let body_values = email.body_values.as_ref().unwrap();
    assert_eq!(body_values[part_id].value, "Hi Alice");

    let attachment = &email.attachments.as_ref().unwrap()[0];
    assert_eq!(attachment.name.as_deref(), Some("report.pdf"));
    assert_eq!(attachment.blob_id.as_ref().unwrap().as_str(), "B102");
}

#[test]
fn email_submission_roundtrip() {
    let value = fixture("mail/email_submission.json");
    assert_eq!(roundtrip::<EmailSubmission>(&value), value);

    let submission: EmailSubmission = serde_json::from_value(value).unwrap();
    assert_eq!(submission.email_id.as_str(), "E5");
    assert_eq!(submission.identity_id.as_str(), "I1");
    let envelope = submission.envelope.unwrap();
    assert_eq!(envelope.mail_from.email, "alice@example.com");
    assert_eq!(envelope.rcpt_to[0].email, "bob@example.com");
}

#[test]
fn email_import_keyword_and_received_at_set_only_those_fields() {
    let import = EmailImport::new("B1", "M1")
        .keyword("$seen")
        .received_at("2026-01-15T13:00:00Z");
    assert_eq!(import.blob_id.as_ref().unwrap().as_str(), "B1");
    let mailbox_ids: Vec<_> = import
        .mailbox_ids
        .as_ref()
        .unwrap()
        .iter()
        .map(|(id, included)| (id.as_str(), *included))
        .collect();
    assert_eq!(mailbox_ids, [("M1", true)]);
    assert_eq!(import.keywords.as_ref().unwrap().get("$seen"), Some(&true));
    assert_eq!(
        import.received_at.as_ref().unwrap().as_str(),
        "2026-01-15T13:00:00Z"
    );
}

#[test]
fn email_query_filter_in_mailbox_sets_only_that_field() {
    let filter = EmailQueryFilter::in_mailbox("M1");
    assert_eq!(filter.in_mailbox.as_ref().unwrap().as_str(), "M1");
    assert_eq!(filter.subject, None);
    assert_eq!(filter.before, None);
    assert_eq!(filter.after, None);
}

#[test]
fn mailbox_roles_cover_rfc8621_and_registry() {
    use jmap_proto::mail::role::*;
    assert_eq!(ALL, "all");
    assert_eq!(ARCHIVE, "archive");
    assert_eq!(DRAFTS, "drafts");
    assert_eq!(FLAGGED, "flagged");
    assert_eq!(IMPORTANT, "important");
    assert_eq!(INBOX, "inbox");
    assert_eq!(JUNK, "junk");
    assert_eq!(SENT, "sent");
    assert_eq!(SUBSCRIBED, "subscribed");
    assert_eq!(TRASH, "trash");
}

#[test]
fn email_submission_set_errors_cover_rfc8621() {
    use jmap_proto::mail::email_submission_set_error::*;
    assert_eq!(CANNOT_UNSEND, "cannotUnsend");
    assert_eq!(TOO_MANY_RECIPIENTS, "tooManyRecipients");
    assert_eq!(NO_RECIPIENTS, "noRecipients");
    assert_eq!(INVALID_RECIPIENTS, "invalidRecipients");
    assert_eq!(FORBIDDEN_MAIL_FROM, "forbiddenMailFrom");
    assert_eq!(FORBIDDEN_FROM, "forbiddenFrom");
}

#[test]
fn email_query_filter_properties_cover_rfc8621() {
    let filter: EmailQueryFilter = serde_json::from_value(serde_json::json!({
        "inMailboxOtherThan": ["M2", "M3"],
        "minSize": 1024,
        "maxSize": 20480,
        "hasAttachment": true,
        "body": "search text",
        "cc": "cc@example.com",
        "bcc": "bcc@example.com"
    }))
    .unwrap();

    assert_eq!(filter.in_mailbox_other_than.unwrap().len(), 2);
    assert_eq!(filter.min_size, Some(1024));
    assert_eq!(filter.max_size, Some(20480));
    assert_eq!(filter.has_attachment, Some(true));
    assert_eq!(filter.body.as_deref(), Some("search text"));
    assert_eq!(filter.cc.as_deref(), Some("cc@example.com"));
    assert_eq!(filter.bcc.as_deref(), Some("bcc@example.com"));
}

#[test]
fn email_address_and_submission_deserialize_with_missing_optional_fields() {
    let addr: jmap_proto::mail::EmailAddress = serde_json::from_value(serde_json::json!({
        "email": "user@example.com"
    }))
    .expect("EmailAddress without name must deserialize cleanly");
    assert_eq!(addr.email, "user@example.com");
    assert_eq!(addr.name, None);

    let sub: EmailSubmission = serde_json::from_value(serde_json::json!({
        "id": "S1"
    }))
    .expect("Partial EmailSubmission must deserialize cleanly");
    assert_eq!(sub.id.as_ref().unwrap().as_str(), "S1");
}

#[test]
fn email_submission_delivery_status_and_undo_status_cover_rfc8621() {
    use jmap_proto::mail::undo_status::*;
    assert_eq!(PENDING, "pending");
    assert_eq!(FINAL, "final");
    assert_eq!(CANCELED, "canceled");

    let sub: EmailSubmission = serde_json::from_value(serde_json::json!({
        "id": "S2",
        "identityId": "I1",
        "emailId": "M1",
        "undoStatus": "pending",
        "deliveryStatus": {
            "bob@example.com": {
                "smtpReply": "250 2.1.5 Ok",
                "delivered": "queued"
            }
        },
        "dsnBlobIds": ["B1", "B2"],
        "mdnBlobIds": ["B3"]
    }))
    .unwrap();

    assert_eq!(sub.undo_status.as_deref(), Some("pending"));
    assert!(
        sub.delivery_status
            .as_ref()
            .unwrap()
            .contains_key("bob@example.com")
    );
    assert_eq!(
        sub.extra.get("dsnBlobIds"),
        Some(&serde_json::json!(["B1", "B2"]))
    );
    assert_eq!(
        sub.extra.get("mdnBlobIds"),
        Some(&serde_json::json!(["B3"]))
    );
}

#[test]
fn mailbox_rights_and_thread_roundtrip_cover_rfc8621() {
    let mailbox: Mailbox = serde_json::from_value(serde_json::json!({
        "name": "Shared Archive",
        "myRights": {
            "mayReadItems": true,
            "mayAddItems": false,
            "maySetSeen": true
        }
    }))
    .unwrap();

    assert_eq!(mailbox.name, "Shared Archive");
    let rights = mailbox.my_rights.as_ref().unwrap();
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_add_items, Some(false));
    assert_eq!(rights.may_set_seen, Some(true));

    let thread: jmap_proto::mail::Thread = serde_json::from_value(serde_json::json!({
        "id": "T1",
        "emailIds": ["M1", "M2", "M3"]
    }))
    .unwrap();

    assert_eq!(thread.id.as_ref().unwrap().as_str(), "T1");
    assert_eq!(thread.email_ids.len(), 3);
}

#[test]
fn vacation_response_roundtrip_covers_rfc8621() {
    let vacation: jmap_proto::mail::VacationResponse = serde_json::from_value(serde_json::json!({
        "id": "singleton",
        "isEnabled": true,
        "fromDate": "2026-09-01T00:00:00Z",
        "toDate": "2026-09-10T00:00:00Z",
        "subject": "Out of office",
        "textBody": "I am on annual leave."
    }))
    .unwrap();

    assert_eq!(vacation.id.as_ref().unwrap().as_str(), "singleton");
    assert!(vacation.is_enabled);
    assert_eq!(
        vacation.from_date.as_ref().unwrap().as_str(),
        "2026-09-01T00:00:00Z"
    );
    assert_eq!(vacation.subject.as_deref(), Some("Out of office"));
    assert_eq!(vacation.text_body.as_deref(), Some("I am on annual leave."));
}

#[test]
fn delivery_status_and_submission_query_filter_roundtrip_cover_rfc8621() {
    use jmap_proto::mail::{
        DeliveryStatus, EmailSubmissionQueryFilter, delivered, displayed, identity_set_error,
    };
    use std::collections::BTreeMap;

    assert_eq!(delivered::QUEUED, "queued");
    assert_eq!(delivered::YES, "yes");
    assert_eq!(delivered::NO, "no");
    assert_eq!(delivered::UNKNOWN, "unknown");

    assert_eq!(displayed::UNKNOWN, "unknown");
    assert_eq!(displayed::YES, "yes");

    assert_eq!(identity_set_error::FORBIDDEN_FROM, "forbiddenFrom");
    assert_eq!(
        identity_set_error::CANNOT_DESTROY_DEFAULT,
        "cannotDestroyDefault"
    );

    let status = DeliveryStatus {
        smtp_reply: "250 2.1.5 Ok".to_owned(),
        delivered: delivered::YES.to_owned(),
        displayed: displayed::YES.to_owned(),
        extra: BTreeMap::new(),
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["smtpReply"], "250 2.1.5 Ok");
    assert_eq!(json["delivered"], "yes");
    assert_eq!(json["displayed"], "yes");

    let round_tripped: DeliveryStatus = serde_json::from_value(json).unwrap();
    assert_eq!(round_tripped, status);

    let sub = EmailSubmission {
        id: Some("S1".into()),
        identity_id: "I1".into(),
        email_id: "M1".into(),
        delivery_status: Some(BTreeMap::from([(
            "bob@example.com".to_owned(),
            status.clone(),
        )])),
        ..EmailSubmission::default()
    };
    let sub_json = serde_json::to_value(&sub).unwrap();
    assert_eq!(
        sub_json["deliveryStatus"]["bob@example.com"]["delivered"],
        "yes"
    );

    let filter: EmailSubmissionQueryFilter = serde_json::from_value(serde_json::json!({
        "identityIds": ["I1"],
        "emailIds": ["M1", "M2"],
        "threadIds": ["T1"],
        "undoStatus": "pending",
        "before": "2026-09-01T00:00:00Z"
    }))
    .unwrap();

    assert_eq!(filter.identity_ids.unwrap().len(), 1);
    assert_eq!(filter.email_ids.unwrap().len(), 2);
    assert_eq!(filter.thread_ids.unwrap().len(), 1);
    assert_eq!(filter.undo_status.as_deref(), Some("pending"));
    assert_eq!(
        filter.before.as_ref().unwrap().as_str(),
        "2026-09-01T00:00:00Z"
    );
}

#[test]
fn search_snippet_roundtrip_covers_rfc8621() {
    use jmap_proto::mail::SearchSnippet;

    let snippet: SearchSnippet = serde_json::from_value(serde_json::json!({
        "emailId": "M1",
        "subject": "Hello <b>world</b>",
        "preview": "This is <b>important</b> message"
    }))
    .unwrap();

    assert_eq!(snippet.email_id.as_str(), "M1");
    assert_eq!(snippet.subject.as_deref(), Some("Hello <b>world</b>"));
    assert_eq!(
        snippet.preview.as_deref(),
        Some("This is <b>important</b> message")
    );
}

#[test]
fn email_parse_and_headers_roundtrip_cover_rfc8621() {
    use jmap_proto::mail::{
        EmailAddress, EmailAddressGroup, EmailHeader, EmailParseRequest, EmailParseResponse,
    };

    let req = EmailParseRequest::new("A1", ["b1", "b2"])
        .properties(["id", "subject", "from"])
        .body_properties(["partId", "value"])
        .fetch_text_body_values()
        .fetch_html_body_values()
        .fetch_all_body_values()
        .max_body_value_bytes(4096);

    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["accountId"], "A1");
    assert_eq!(json["blobIds"], serde_json::json!(["b1", "b2"]));
    assert_eq!(
        json["properties"],
        serde_json::json!(["id", "subject", "from"])
    );
    assert_eq!(
        json["bodyProperties"],
        serde_json::json!(["partId", "value"])
    );
    assert_eq!(json["fetchTextBodyValues"], true);
    assert_eq!(json["fetchHTMLBodyValues"], true);
    assert_eq!(json["fetchAllBodyValues"], true);
    assert_eq!(json["maxBodyValueBytes"], 4096);

    let resp_val = serde_json::json!({
        "accountId": "A1",
        "parsed": {
            "b1": {
                "id": "E1",
                "subject": "Parsed Email"
            }
        },
        "notParsable": ["b2"],
        "notFound": ["b3"]
    });
    let resp: EmailParseResponse = serde_json::from_value(resp_val.clone()).unwrap();
    assert_eq!(resp.account_id.as_str(), "A1");
    assert_eq!(
        resp.parsed.as_ref().unwrap()[&jmap_proto::Id::new("b1")]
            .subject
            .as_deref(),
        Some("Parsed Email")
    );
    assert_eq!(resp.not_parsable.as_ref().unwrap().len(), 1);
    assert_eq!(resp.not_found.as_ref().unwrap().len(), 1);

    let header = EmailHeader {
        name: "X-Spam-Score".to_owned(),
        value: "0.0".to_owned(),
    };
    let h_val = serde_json::to_value(&header).unwrap();
    assert_eq!(h_val["name"], "X-Spam-Score");
    assert_eq!(h_val["value"], "0.0");
    assert_eq!(
        serde_json::from_value::<EmailHeader>(h_val).unwrap(),
        header
    );

    let addr_group = EmailAddressGroup {
        name: Some("Engineering".to_owned()),
        addresses: vec![
            EmailAddress::new(Some("Alice"), "alice@example.com"),
            EmailAddress::new(Some("Bob"), "bob@example.com"),
        ],
    };
    let ag_val = serde_json::to_value(&addr_group).unwrap();
    assert_eq!(ag_val["name"], "Engineering");
    assert_eq!(ag_val["addresses"].as_array().unwrap().len(), 2);
    assert_eq!(
        serde_json::from_value::<EmailAddressGroup>(ag_val).unwrap(),
        addr_group
    );
}

/// MailCapability and SubmissionCapability cover RFC 8621 §1.3 and §1.4.
#[test]
fn mail_capabilities_roundtrip_covers_rfc8621() {
    use jmap_proto::mail::{MailCapability, SubmissionCapability};
    use std::collections::BTreeMap;

    let mail_cap = MailCapability {
        max_size_attachments_per_email: 50_000_000,
        max_size_email_in_bytes: 75_000_000,
        max_size_body_value_bytes: 2_000_000,
        max_number_of_attachments_per_email: 100,
        max_number_of_recipients_per_email: 50,
        may_create_top_level_mailbox: true,
        extra: BTreeMap::new(),
    };
    let mc_val = serde_json::to_value(&mail_cap).unwrap();
    assert_eq!(mc_val["maxSizeAttachmentsPerEmail"], 50_000_000);
    assert_eq!(mc_val["maxSizeEmailInBytes"], 75_000_000);
    assert_eq!(mc_val["maxSizeBodyValueBytes"], 2_000_000);
    assert_eq!(mc_val["maxNumberOfAttachmentsPerEmail"], 100);
    assert_eq!(mc_val["maxNumberOfRecipientsPerEmail"], 50);
    assert_eq!(mc_val["mayCreateTopLevelMailbox"], true);

    let round_mc: MailCapability = serde_json::from_value(mc_val).unwrap();
    assert_eq!(round_mc, mail_cap);

    let sub_cap = SubmissionCapability {
        max_delayed_send: 86400,
        submission_extensions: BTreeMap::from([(
            "FUTURERELEASE".to_owned(),
            vec!["MAXDISCARD".to_owned()],
        )]),
        extra: BTreeMap::new(),
    };
    let sc_val = serde_json::to_value(&sub_cap).unwrap();
    assert_eq!(sc_val["maxDelayedSend"], 86400);
    assert_eq!(
        sc_val["submissionExtensions"]["FUTURERELEASE"],
        serde_json::json!(["MAXDISCARD"])
    );

    let round_sc: SubmissionCapability = serde_json::from_value(sc_val).unwrap();
    assert_eq!(round_sc, sub_cap);
}

#[test]
fn mailbox_sharing_identity_draft_and_email_filter_builders_roundtrip() {
    use jmap_proto::UtcDate;
    use jmap_proto::mail::{
        Email, EmailHeader, EmailQueryFilter, Identity, Mailbox, MailboxRights,
    };
    use std::collections::BTreeMap;

    let rights = MailboxRights {
        may_read_items: Some(true),
        may_add_items: Some(true),
        may_remove_items: Some(false),
        may_set_seen: Some(true),
        may_set_keywords: Some(true),
        may_create_child: Some(false),
        may_rename: Some(false),
        may_delete: Some(false),
        may_submit: Some(false),
        may_share: Some(false),
        extra: BTreeMap::new(),
    };

    let mailbox = Mailbox {
        id: Some("mb1".into()),
        name: "Shared Support Inbox".to_owned(),
        my_rights: Some(rights.clone()),
        ..Mailbox::default()
    };

    let mb_val = serde_json::to_value(&mailbox).unwrap();
    assert_eq!(mb_val["myRights"]["maySetKeywords"], true);

    let round_mb: Mailbox = serde_json::from_value(mb_val).unwrap();
    assert_eq!(round_mb, mailbox);

    let identity = Identity {
        id: Some("id1".into()),
        name: "Support".to_owned(),
        email: "support@example.com".to_owned(),
        draft_mailbox_id: Some("mb_drafts".into()),
        ..Identity::default()
    };

    let id_val = serde_json::to_value(&identity).unwrap();
    assert_eq!(id_val["draftMailboxId"], "mb_drafts");

    let round_id: Identity = serde_json::from_value(id_val).unwrap();
    assert_eq!(round_id, identity);

    let email = Email {
        id: Some("em1".into()),
        subject: Some("Report".to_owned()),
        headers: Some(vec![EmailHeader::new("X-Custom", "value1")]),
        ..Email::default()
    };

    let em_val = serde_json::to_value(&email).unwrap();
    assert_eq!(em_val["headers"][0]["name"], "X-Custom");
    assert_eq!(em_val["headers"][0]["value"], "value1");

    let round_em: Email = serde_json::from_value(em_val).unwrap();
    assert_eq!(round_em, email);

    let filter = EmailQueryFilter::in_mailbox("mb1")
        .in_mailbox_other_than(["mb_trash", "mb_junk"])
        .has_keyword("$flagged")
        .not_keyword("$seen")
        .has_attachment(true)
        .from("boss@example.com")
        .to("team@example.com")
        .subject("Quarterly Report")
        .text("urgent")
        .time_range(
            Some(UtcDate::new("2026-09-01T00:00:00Z")),
            Some(UtcDate::new("2026-09-02T00:00:00Z")),
        );

    assert_eq!(filter.in_mailbox.as_ref().unwrap().as_str(), "mb1");
    assert_eq!(filter.in_mailbox_other_than.as_ref().unwrap().len(), 2);
    assert_eq!(filter.has_keyword.as_deref(), Some("$flagged"));
    assert_eq!(filter.not_keyword.as_deref(), Some("$seen"));
    assert_eq!(filter.has_attachment, Some(true));
    assert_eq!(filter.from.as_deref(), Some("boss@example.com"));
    assert_eq!(filter.to.as_deref(), Some("team@example.com"));
    assert_eq!(filter.subject.as_deref(), Some("Quarterly Report"));
    assert_eq!(filter.text.as_deref(), Some("urgent"));
    assert_eq!(
        filter.before.as_ref().unwrap().as_str(),
        "2026-09-02T00:00:00Z"
    );
    assert_eq!(
        filter.after.as_ref().unwrap().as_str(),
        "2026-09-01T00:00:00Z"
    );
}

#[test]
fn mail_spec_properties_roundtrip() {
    use jmap_proto::mail::{EmailBodyPart, EmailHeader, EmailQueryFilter, Identity};

    let identity = Identity {
        id: Some("id_spec".into()),
        name: "Security Team".to_owned(),
        email: "sec@example.com".to_owned(),
        may_send: Some(true),
        may_delete: Some(false),
        ..Identity::default()
    };
    let id_json = serde_json::to_value(&identity).unwrap();
    assert_eq!(id_json["maySend"], true);
    assert_eq!(id_json["mayDelete"], false);
    assert_eq!(
        serde_json::from_value::<Identity>(id_json).unwrap(),
        identity
    );

    let part = EmailBodyPart {
        part_id: Some("part1".to_owned()),
        content_type: Some("message/rfc822".to_owned()),
        headers: Some(vec![EmailHeader::new(
            "Content-Description",
            "Embedded Message",
        )]),
        ..EmailBodyPart::default()
    };
    let p_json = serde_json::to_value(&part).unwrap();
    assert_eq!(p_json["headers"][0]["name"], "Content-Description");
    assert_eq!(p_json["headers"][0]["value"], "Embedded Message");
    assert_eq!(
        serde_json::from_value::<EmailBodyPart>(p_json).unwrap(),
        part
    );

    let filter = EmailQueryFilter::in_mailbox("mb_inbox")
        .all_in_thread_have_keyword("$seen")
        .some_in_thread_have_keyword("$flagged")
        .none_in_thread_have_keyword("$junk")
        .header("List-Id", "<dev.example.com>");

    assert_eq!(filter.all_in_thread_have_keyword.as_deref(), Some("$seen"));
    assert_eq!(
        filter.some_in_thread_have_keyword.as_deref(),
        Some("$flagged")
    );
    assert_eq!(filter.none_in_thread_have_keyword.as_deref(), Some("$junk"));
    assert_eq!(
        filter.header.as_ref().unwrap(),
        &["List-Id".to_owned(), "<dev.example.com>".to_owned()]
    );
    let f_json = serde_json::to_value(&filter).unwrap();
    assert_eq!(f_json["allInThreadHaveKeyword"], "$seen");
    assert_eq!(f_json["someInThreadHaveKeyword"], "$flagged");
    assert_eq!(f_json["noneInThreadHaveKeyword"], "$junk");
    assert_eq!(f_json["header"][0], "List-Id");
    assert_eq!(f_json["header"][1], "<dev.example.com>");
}

#[test]
fn email_set_errors_cover_rfc8621() {
    use jmap_proto::mail::email_set_error;

    assert_eq!(email_set_error::BLOB_NOT_FOUND, "blobNotFound");
    assert_eq!(email_set_error::TOO_MANY_KEYWORDS, "tooManyKeywords");
    assert_eq!(email_set_error::TOO_MANY_MAILBOXES, "tooManyMailboxes");
}

#[test]
fn mailbox_identity_and_submission_builders_roundtrip() {
    use jmap_proto::UtcDate;
    use jmap_proto::mail::{
        EmailAddress, EmailSubmission, Envelope, EnvelopeAddress, Identity, Mailbox,
    };

    let mb = Mailbox::new("Projects")
        .with_parent_id("mb_work")
        .with_role("inbox")
        .with_sort_order(10)
        .is_subscribed(true);

    assert_eq!(mb.name, "Projects");
    assert_eq!(mb.parent_id.as_ref().unwrap().as_str(), "mb_work");
    assert_eq!(mb.role.as_deref(), Some("inbox"));
    assert_eq!(mb.sort_order, Some(10));
    assert_eq!(mb.is_subscribed, Some(true));

    let mb_val = serde_json::to_value(&mb).unwrap();
    assert_eq!(mb_val["name"], "Projects");
    assert_eq!(mb_val["parentId"], "mb_work");
    assert_eq!(mb_val["role"], "inbox");
    assert_eq!(mb_val["sortOrder"], 10);
    assert_eq!(mb_val["isSubscribed"], true);
    assert_eq!(serde_json::from_value::<Mailbox>(mb_val).unwrap(), mb);

    let ident = Identity::new("Engineering", "eng@example.com")
        .with_reply_to([EmailAddress::new(Some("Lead"), "lead@example.com")])
        .with_bcc([EmailAddress::new(None, "archive@example.com")])
        .with_text_signature("-- \nEng Team")
        .with_html_signature("<p>-- <br>Eng Team</p>")
        .with_draft_mailbox_id("mb_drafts")
        .may_send(true);

    assert_eq!(ident.name, "Engineering");
    assert_eq!(ident.email, "eng@example.com");
    assert_eq!(ident.reply_to.as_ref().unwrap().len(), 1);
    assert_eq!(ident.bcc.as_ref().unwrap().len(), 1);
    assert_eq!(ident.text_signature.as_deref(), Some("-- \nEng Team"));
    assert_eq!(
        ident.html_signature.as_deref(),
        Some("<p>-- <br>Eng Team</p>")
    );
    assert_eq!(
        ident.draft_mailbox_id.as_ref().unwrap().as_str(),
        "mb_drafts"
    );
    assert_eq!(ident.may_send, Some(true));

    let ident_val = serde_json::to_value(&ident).unwrap();
    assert_eq!(ident_val["name"], "Engineering");
    assert_eq!(ident_val["email"], "eng@example.com");
    assert_eq!(ident_val["draftMailboxId"], "mb_drafts");
    assert_eq!(ident_val["maySend"], true);
    assert_eq!(
        serde_json::from_value::<Identity>(ident_val).unwrap(),
        ident
    );

    let sub = EmailSubmission::new("ident_1", "email_1")
        .with_thread_id("th_1")
        .with_envelope(Envelope {
            mail_from: EnvelopeAddress::new("eng@example.com"),
            rcpt_to: vec![EnvelopeAddress::new("customer@example.com")],
        })
        .with_send_at(UtcDate::new("2026-09-01T12:00:00Z"))
        .with_undo_status("pending");

    assert_eq!(sub.identity_id.as_str(), "ident_1");
    assert_eq!(sub.email_id.as_str(), "email_1");
    assert_eq!(sub.thread_id.as_ref().unwrap().as_str(), "th_1");
    assert_eq!(
        sub.envelope.as_ref().unwrap().mail_from.email,
        "eng@example.com"
    );
    assert_eq!(
        sub.send_at.as_ref().unwrap().as_str(),
        "2026-09-01T12:00:00Z"
    );
    assert_eq!(sub.undo_status.as_deref(), Some("pending"));

    let sub_val = serde_json::to_value(&sub).unwrap();
    assert_eq!(sub_val["identityId"], "ident_1");
    assert_eq!(sub_val["emailId"], "email_1");
    assert_eq!(sub_val["threadId"], "th_1");
    assert_eq!(sub_val["undoStatus"], "pending");
    assert_eq!(
        serde_json::from_value::<EmailSubmission>(sub_val).unwrap(),
        sub
    );
}

#[test]
fn mdn_send_request_response_and_disposition_roundtrip_covers_rfc9007() {
    use jmap_proto::error::SetError;
    use jmap_proto::mail::{
        MDN, MDNDisposition, MDNSendRequest, MDNSendResponse, mdn_action_mode,
        mdn_disposition_type, mdn_sending_mode, mdn_set_error,
    };
    use std::collections::BTreeMap;

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
    assert_eq!(mdn_set_error::MDN_ALREADY_SENT, "mdnAlreadySent");
    assert_eq!(mdn_set_error::FORBIDDEN_FROM, "forbiddenFrom");

    let disp = MDNDisposition::new(
        mdn_action_mode::MANUAL_ACTION,
        mdn_sending_mode::MDN_SENT_MANUALLY,
        mdn_disposition_type::DISPLAYED,
    );
    assert_eq!(disp.action_mode, "manual-action");
    assert_eq!(disp.sending_mode, "mdn-sent-manually");
    assert_eq!(disp.disposition_type, "displayed");

    let mdn = MDN::new("email_42", disp)
        .with_subject("Read: Project Proposal")
        .with_text_body("The message was displayed on 2026-08-29.")
        .with_reporting_ua("Evolution-JMAP/0.2.0")
        .with_final_recipient("user@example.com")
        .with_original_message_id("<msg-123@example.org>");

    assert_eq!(mdn.for_email_id.as_str(), "email_42");
    assert_eq!(mdn.subject.as_deref(), Some("Read: Project Proposal"));
    assert_eq!(mdn.reporting_ua.as_deref(), Some("Evolution-JMAP/0.2.0"));
    assert_eq!(mdn.final_recipient.as_deref(), Some("user@example.com"));

    let req = MDNSendRequest::new("acc1", BTreeMap::from([("k1".to_string(), mdn.clone())]))
        .with_on_success_update_email(BTreeMap::from([(
            "email_42".into(),
            serde_json::json!({"keywords/$mdnsent": true}),
        )]));

    assert_eq!(req.account_id.as_str(), "acc1");
    assert_eq!(req.send.len(), 1);
    assert_eq!(req.send["k1"].for_email_id.as_str(), "email_42");

    let req_val = serde_json::to_value(&req).unwrap();
    assert_eq!(req_val["accountId"], "acc1");
    assert_eq!(req_val["send"]["k1"]["forEmailId"], "email_42");
    assert_eq!(
        req_val["send"]["k1"]["disposition"]["actionMode"],
        "manual-action"
    );
    assert_eq!(
        req_val["onSuccessUpdateEmail"]["email_42"]["keywords/$mdnsent"],
        true
    );

    let resp = MDNSendResponse {
        account_id: "acc1".into(),
        sent: Some(BTreeMap::from([("k1".to_string(), mdn)])),
        not_sent: Some(BTreeMap::from([(
            "k2".to_string(),
            SetError::new(mdn_set_error::MDN_ALREADY_SENT),
        )])),
    };

    let resp_val = serde_json::to_value(&resp).unwrap();
    assert_eq!(resp_val["accountId"], "acc1");
    assert_eq!(resp_val["sent"]["k1"]["forEmailId"], "email_42");
    assert_eq!(resp_val["notSent"]["k2"]["type"], "mdnAlreadySent");
    assert_eq!(
        serde_json::from_value::<MDNSendResponse>(resp_val).unwrap(),
        resp
    );
}

#[test]
fn search_snippet_get_request_and_response_roundtrip_and_builders() {
    use jmap_proto::mail::{SearchSnippet, SearchSnippetGetRequest, SearchSnippetGetResponse};

    let snippet = SearchSnippet::new("email_101")
        .with_subject("Re: Roadmap Discussion")
        .with_preview("Here is the latest draft of the plan...");

    assert_eq!(snippet.email_id.as_str(), "email_101");
    assert_eq!(snippet.subject.as_deref(), Some("Re: Roadmap Discussion"));
    assert_eq!(
        snippet.preview.as_deref(),
        Some("Here is the latest draft of the plan...")
    );

    let snippet_val = serde_json::to_value(&snippet).unwrap();
    assert_eq!(snippet_val["emailId"], "email_101");
    assert_eq!(snippet_val["subject"], "Re: Roadmap Discussion");
    assert_eq!(
        snippet_val["preview"],
        "Here is the latest draft of the plan..."
    );
    assert_eq!(
        serde_json::from_value::<SearchSnippet>(snippet_val).unwrap(),
        snippet
    );

    let req = SearchSnippetGetRequest::new("acc1", ["email_101", "email_102"])
        .with_filter(EmailQueryFilter::in_mailbox("inbox"));

    assert_eq!(req.account_id.as_str(), "acc1");
    assert_eq!(req.email_ids.len(), 2);
    let jmap_proto::methods::Filter::Condition(condition) = req.filter.as_ref().unwrap() else {
        panic!("a single EmailQueryFilter builds a Filter::Condition, not an operator tree");
    };
    assert_eq!(condition.in_mailbox.as_ref().unwrap().as_str(), "inbox");

    let req_val = serde_json::to_value(&req).unwrap();
    assert_eq!(req_val["accountId"], "acc1");
    assert_eq!(req_val["emailIds"][0], "email_101");
    assert_eq!(req_val["emailIds"][1], "email_102");
    assert_eq!(req_val["filter"]["inMailbox"], "inbox");
    assert_eq!(
        serde_json::from_value::<SearchSnippetGetRequest>(req_val).unwrap(),
        req
    );

    let resp =
        SearchSnippetGetResponse::new("acc1", [snippet.clone()]).with_not_found(["email_102"]);

    assert_eq!(resp.account_id.as_str(), "acc1");
    assert_eq!(resp.list.len(), 1);
    assert_eq!(resp.not_found.as_ref().unwrap().len(), 1);
    assert_eq!(resp.not_found.as_ref().unwrap()[0].as_str(), "email_102");

    let resp_val = serde_json::to_value(&resp).unwrap();
    assert_eq!(resp_val["accountId"], "acc1");
    assert_eq!(resp_val["list"][0]["emailId"], "email_101");
    assert_eq!(resp_val["notFound"][0], "email_102");
    assert_eq!(
        serde_json::from_value::<SearchSnippetGetResponse>(resp_val).unwrap(),
        resp
    );
}

#[test]
fn email_responses_and_entities_builders_roundtrip() {
    use jmap_proto::UtcDate;
    use jmap_proto::error::SetError;
    use jmap_proto::mail::{
        EmailAddress, EmailAddressGroup, EmailImportResponse, EmailParseResponse,
        EmailSubmissionSetRequest, MailboxRights, Thread, VacationResponse,
    };
    use jmap_proto::methods::SetRequest;
    use std::collections::BTreeMap;

    let grp = EmailAddressGroup::new([
        EmailAddress::new(Some("Alice"), "alice@example.com"),
        EmailAddress::new(Some("Bob"), "bob@example.com"),
    ])
    .with_name("Core Team");

    assert_eq!(grp.name.as_deref(), Some("Core Team"));
    assert_eq!(grp.addresses.len(), 2);
    let grp_val = serde_json::to_value(&grp).unwrap();
    assert_eq!(grp_val["name"], "Core Team");
    assert_eq!(grp_val["addresses"].as_array().unwrap().len(), 2);
    assert_eq!(
        serde_json::from_value::<EmailAddressGroup>(grp_val).unwrap(),
        grp
    );

    let thread = Thread::new("th_1", ["e_1", "e_2"]);
    assert_eq!(thread.id.as_ref().unwrap().as_str(), "th_1");
    assert_eq!(thread.email_ids.len(), 2);
    let thread_val = serde_json::to_value(&thread).unwrap();
    assert_eq!(thread_val["id"], "th_1");
    assert_eq!(
        serde_json::from_value::<Thread>(thread_val).unwrap(),
        thread
    );

    let rights_all = MailboxRights::all();
    assert_eq!(rights_all.may_read_items, Some(true));
    assert_eq!(rights_all.may_add_items, Some(true));
    assert_eq!(rights_all.may_remove_items, Some(true));
    assert_eq!(rights_all.may_set_seen, Some(true));
    assert_eq!(rights_all.may_set_keywords, Some(true));
    assert_eq!(rights_all.may_create_child, Some(true));
    assert_eq!(rights_all.may_rename, Some(true));
    assert_eq!(rights_all.may_delete, Some(true));
    assert_eq!(rights_all.may_submit, Some(true));

    let rights_ro = MailboxRights::read_only();
    assert_eq!(rights_ro.may_read_items, Some(true));
    assert_eq!(rights_ro.may_add_items, None);
    assert_eq!(rights_ro.may_delete, None);

    let vac = VacationResponse::new(true)
        .with_id("vac_1")
        .with_from_date(UtcDate::new("2026-07-01T00:00:00Z"))
        .with_to_date(UtcDate::new("2026-07-15T00:00:00Z"))
        .with_subject("Out of office")
        .with_text_body("I am currently away.")
        .with_html_body("<p>I am currently away.</p>");

    assert!(vac.is_enabled);
    assert_eq!(vac.id.as_ref().unwrap().as_str(), "vac_1");
    assert_eq!(
        vac.from_date.as_ref().unwrap().as_str(),
        "2026-07-01T00:00:00Z"
    );
    assert_eq!(
        vac.to_date.as_ref().unwrap().as_str(),
        "2026-07-15T00:00:00Z"
    );
    assert_eq!(vac.subject.as_deref(), Some("Out of office"));
    assert_eq!(vac.text_body.as_deref(), Some("I am currently away."));
    assert_eq!(
        vac.html_body.as_deref(),
        Some("<p>I am currently away.</p>")
    );

    let vac_val = serde_json::to_value(&vac).unwrap();
    assert_eq!(vac_val["isEnabled"], true);
    assert_eq!(vac_val["subject"], "Out of office");
    assert_eq!(
        serde_json::from_value::<VacationResponse>(vac_val).unwrap(),
        vac
    );

    let sub_req = EmailSubmissionSetRequest::new(SetRequest::new("acc1"))
        .with_on_success_update_email(BTreeMap::from([(
            "e_sub_1".to_string(),
            serde_json::json!({"keywords/$seen": true}),
        )]))
        .with_on_success_destroy_email(["e_draft_1"]);

    assert_eq!(sub_req.set.account_id.as_str(), "acc1");
    assert!(sub_req.on_success_update_email.is_some());
    assert_eq!(sub_req.on_success_destroy_email.as_ref().unwrap().len(), 1);

    let import_resp = EmailImportResponse::new("acc1", "s2")
        .with_old_state("s1")
        .with_created(BTreeMap::from([("k1".to_string(), Email::default())]))
        .with_not_created(BTreeMap::from([(
            "k2".to_string(),
            SetError::new("invalidEmail"),
        )]));

    assert_eq!(import_resp.account_id.as_str(), "acc1");
    assert_eq!(import_resp.old_state.as_ref().unwrap().as_str(), "s1");
    assert_eq!(import_resp.new_state.as_str(), "s2");
    assert_eq!(import_resp.created.as_ref().unwrap().len(), 1);
    assert_eq!(import_resp.not_created.as_ref().unwrap().len(), 1);

    let parse_resp = EmailParseResponse::new("acc1")
        .with_parsed(BTreeMap::from([("b1".into(), Email::default())]))
        .with_not_parsable(["b2"])
        .with_not_found(["b3"]);

    assert_eq!(parse_resp.account_id.as_str(), "acc1");
    assert_eq!(parse_resp.parsed.as_ref().unwrap().len(), 1);
    assert_eq!(parse_resp.not_parsable.as_ref().unwrap().len(), 1);
    assert_eq!(parse_resp.not_found.as_ref().unwrap().len(), 1);
}

#[test]
fn email_body_part_capability_and_submission_filter_builders() {
    use jmap_proto::mail::{
        Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailHeader,
        EmailSubmissionQueryFilter, MailCapability, SubmissionCapability,
    };
    use jmap_proto::{Id, UtcDate};
    use std::collections::BTreeMap;

    let mail_cap = MailCapability::new()
        .with_max_size_attachments_per_email(50_000_000)
        .with_max_size_email_in_bytes(100_000_000)
        .with_max_size_body_value_bytes(1_000_000)
        .with_max_number_of_attachments_per_email(20)
        .with_max_number_of_recipients_per_email(100)
        .may_create_top_level_mailbox(true);

    assert_eq!(mail_cap.max_size_attachments_per_email, 50_000_000);
    assert_eq!(mail_cap.max_size_email_in_bytes, 100_000_000);
    assert_eq!(mail_cap.max_size_body_value_bytes, 1_000_000);
    assert_eq!(mail_cap.max_number_of_attachments_per_email, 20);
    assert_eq!(mail_cap.max_number_of_recipients_per_email, 100);
    assert!(mail_cap.may_create_top_level_mailbox);

    let sub_cap = SubmissionCapability::new()
        .with_max_delayed_send(86400)
        .with_submission_extensions(BTreeMap::from([(
            "FUTURERELEASE".to_string(),
            vec!["max=604800".to_string()],
        )]));

    assert_eq!(sub_cap.max_delayed_send, 86400);
    assert_eq!(sub_cap.submission_extensions.len(), 1);

    let part = EmailBodyPart::new()
        .with_part_id("1")
        .with_blob_id("b_text_1")
        .with_size(1024)
        .with_name("notes.txt")
        .with_content_type("text/plain")
        .with_charset("utf-8")
        .with_disposition("inline")
        .with_cid("cid_notes")
        .with_location("https://example.com/notes.txt")
        .with_headers([EmailHeader::new("Content-Description", "Text Notes")]);

    assert_eq!(part.part_id.as_deref(), Some("1"));
    assert_eq!(part.blob_id.as_ref().unwrap().as_str(), "b_text_1");
    assert_eq!(part.size, Some(1024));
    assert_eq!(part.name.as_deref(), Some("notes.txt"));
    assert_eq!(part.content_type.as_deref(), Some("text/plain"));
    assert_eq!(part.charset.as_deref(), Some("utf-8"));
    assert_eq!(part.disposition.as_deref(), Some("inline"));
    assert_eq!(part.cid.as_deref(), Some("cid_notes"));
    assert_eq!(
        part.location.as_deref(),
        Some("https://example.com/notes.txt")
    );
    assert_eq!(part.headers.as_ref().unwrap().len(), 1);

    let email = Email::new()
        .with_id("e_100")
        .with_blob_id("b_msg_100")
        .with_thread_id("th_100")
        .in_mailbox("mb_inbox")
        .with_keyword("$seen")
        .with_size(2048)
        .with_received_at(UtcDate::new("2026-08-29T10:00:00Z"))
        .with_from([EmailAddress::new(Some("Alice"), "alice@example.com")])
        .with_to([EmailAddress::new(Some("Bob"), "bob@example.com")])
        .with_cc([EmailAddress::new(None, "carol@example.com")])
        .with_bcc([EmailAddress::new(None, "dave@example.com")])
        .with_reply_to([EmailAddress::new(Some("Alice"), "alice-reply@example.com")])
        .with_subject("Release update")
        .with_sent_at("Sat, 29 Aug 2026 10:00:00 +0000")
        .with_preview("Here is the release update...")
        .has_attachment(true)
        .with_header("X-Custom", "Value")
        .with_body_structure(part.clone())
        .with_body_values(BTreeMap::from([(
            "1".to_string(),
            EmailBodyValue::new("Here is the release update text"),
        )]))
        .with_text_body([part.clone()])
        .with_attachments([part]);

    assert_eq!(email.id.as_ref().unwrap().as_str(), "e_100");
    assert_eq!(email.blob_id.as_ref().unwrap().as_str(), "b_msg_100");
    assert_eq!(email.thread_id.as_ref().unwrap().as_str(), "th_100");
    assert!(
        email
            .mailbox_ids
            .as_ref()
            .unwrap()
            .contains_key(&Id::new("mb_inbox"))
    );
    assert!(email.keywords.as_ref().unwrap().contains_key("$seen"));
    assert_eq!(email.size, Some(2048));
    assert_eq!(email.from.as_ref().unwrap().len(), 1);
    assert_eq!(email.to.as_ref().unwrap().len(), 1);
    assert_eq!(email.cc.as_ref().unwrap().len(), 1);
    assert_eq!(email.bcc.as_ref().unwrap().len(), 1);
    assert_eq!(email.reply_to.as_ref().unwrap().len(), 1);
    assert_eq!(email.subject.as_deref(), Some("Release update"));
    assert_eq!(
        email.preview.as_deref(),
        Some("Here is the release update...")
    );
    assert_eq!(email.has_attachment, Some(true));
    assert_eq!(email.headers.as_ref().unwrap().len(), 1);
    assert!(email.body_structure.is_some());
    assert_eq!(email.text_body.as_ref().unwrap().len(), 1);
    assert_eq!(email.attachments.as_ref().unwrap().len(), 1);

    let sub_filter = EmailSubmissionQueryFilter::new()
        .with_identity_ids(["id_1", "id_2"])
        .with_email_ids(["e_1"])
        .with_thread_ids(["th_1"])
        .with_undo_status("pending")
        .time_range(
            Some(UtcDate::new("2026-08-01T00:00:00Z")),
            Some(UtcDate::new("2026-09-01T00:00:00Z")),
        );

    assert_eq!(sub_filter.identity_ids.as_ref().unwrap().len(), 2);
    assert_eq!(sub_filter.email_ids.as_ref().unwrap().len(), 1);
    assert_eq!(sub_filter.thread_ids.as_ref().unwrap().len(), 1);
    assert_eq!(sub_filter.undo_status.as_deref(), Some("pending"));
    assert_eq!(
        sub_filter.after.as_ref().unwrap().as_str(),
        "2026-08-01T00:00:00Z"
    );
    assert_eq!(
        sub_filter.before.as_ref().unwrap().as_str(),
        "2026-09-01T00:00:00Z"
    );
}

#[test]
fn mdn_capability_accessor() {
    use jmap_proto::State;
    use jmap_proto::mail::MDNCapability;
    use jmap_proto::session::{CAPABILITY_MDN, Session};

    let mut session = Session::new(
        "alice@example.com",
        "https://jmap.example.com/api/",
        "https://jmap.example.com/download/",
        "https://jmap.example.com/upload/",
        State::new("s1"),
    );

    assert!(session.mdn_capability().is_none());

    session = session.with_capability(
        CAPABILITY_MDN,
        serde_json::json!({"customMdnExtension": 123}),
    );

    let cap = session.mdn_capability().expect("capability present");
    assert_eq!(
        cap,
        MDNCapability::default().with_extra(serde_json::json!({"customMdnExtension": 123}))
    );
}

#[test]
fn smime_verify_capability_and_session_accessor() {
    use jmap_proto::State;
    use jmap_proto::mail::SmimeVerifyCapability;
    use jmap_proto::session::{CAPABILITY_SMIME_VERIFY, Session};

    let mut session = Session::new(
        "alice@example.com",
        "https://jmap.example.com/api/",
        "https://jmap.example.com/download/",
        "https://jmap.example.com/upload/",
        State::new("s1"),
    );

    assert!(session.smime_verify_capability().is_none());

    session = session.with_capability(
        CAPABILITY_SMIME_VERIFY,
        serde_json::json!({"customSmimeOption": "enabled"}),
    );

    let cap = session.smime_verify_capability().expect("smime capability");
    assert_eq!(
        cap,
        SmimeVerifyCapability::new()
            .with_extra(serde_json::json!({"customSmimeOption": "enabled"}))
    );
}

#[test]
fn email_and_body_part_smime_properties_and_builders() {
    use jmap_proto::UtcDate;
    use jmap_proto::mail::{Email, EmailBodyPart, smime_status};

    let body_part = EmailBodyPart::new()
        .with_part_id("1")
        .with_content_type("text/plain")
        .with_smime_status(smime_status::SIGNED_VERIFIED)
        .with_smime_errors(["signer certificate expired but accepted by policy".to_string()])
        .with_smime_verified_at(UtcDate::new("2026-08-30T07:00:00Z"));

    assert_eq!(
        body_part.smime_status.as_deref(),
        Some(smime_status::SIGNED_VERIFIED)
    );
    assert_eq!(
        body_part.smime_errors.as_ref().unwrap(),
        &["signer certificate expired but accepted by policy"]
    );
    assert_eq!(
        body_part.smime_verified_at.as_ref().unwrap().as_str(),
        "2026-08-30T07:00:00Z"
    );

    let email = Email::new()
        .with_id("msg1")
        .with_smime_status(smime_status::SIGNED_VERIFIED)
        .with_smime_errors(["no errors".to_string()])
        .with_smime_verified_at(UtcDate::new("2026-08-30T07:00:00Z"))
        .with_text_body([body_part.clone()]);

    assert_eq!(
        email.smime_status.as_deref(),
        Some(smime_status::SIGNED_VERIFIED)
    );
    assert_eq!(email.smime_errors.as_ref().unwrap(), &["no errors"]);
    assert_eq!(
        email.smime_verified_at.as_ref().unwrap().as_str(),
        "2026-08-30T07:00:00Z"
    );

    // Verify roundtrip through JSON
    let json = serde_json::to_value(&email).expect("serialize email");
    assert_eq!(json["smimeStatus"], "signed/verified");
    assert_eq!(json["smimeErrors"], serde_json::json!(["no errors"]));
    assert_eq!(json["smimeVerifiedAt"], "2026-08-30T07:00:00Z");

    let deserialized: Email = serde_json::from_value(json).expect("deserialize email");
    assert_eq!(deserialized, email);
}

#[test]
fn email_query_filter_smime_filters() {
    use jmap_proto::mail::EmailQueryFilter;

    let filter = EmailQueryFilter::default()
        .with_has_smime(true)
        .with_has_verified_smime(true);

    assert_eq!(filter.has_smime, Some(true));
    assert_eq!(filter.has_verified_smime, Some(true));

    let json = serde_json::to_value(&filter).expect("serialize filter");
    assert_eq!(json["hasSmime"], true);
    assert_eq!(json["hasVerifiedSmime"], true);

    let deserialized: EmailQueryFilter = serde_json::from_value(json).expect("deserialize filter");
    assert_eq!(deserialized, filter);
}

#[test]
fn email_header_and_email_address_builders() {
    use jmap_proto::mail::{EmailAddress, EmailHeader};

    let header = EmailHeader::new("X-Custom-Header", "Value123");
    assert_eq!(header.name, "X-Custom-Header");
    assert_eq!(header.value, "Value123");

    let modified_header = EmailHeader::default()
        .with_name("Subject")
        .with_value("Test Subject");
    assert_eq!(modified_header.name, "Subject");
    assert_eq!(modified_header.value, "Test Subject");

    let addr = EmailAddress::from_email("carol@example.com").with_name("Carol");
    assert_eq!(addr.email, "carol@example.com");
    assert_eq!(addr.name.as_deref(), Some("Carol"));
}

#[test]
fn mail_share_capability_and_mailbox_sharing_roundtrip() {
    use jmap_proto::mail::{MailShareCapability, Mailbox, MailboxRights};
    use jmap_proto::session::{CAPABILITY_MAIL_SHARE, Session};
    use serde_json::json;

    assert_eq!(CAPABILITY_MAIL_SHARE, "urn:ietf:params:jmap:mail:share");

    let cap = MailShareCapability::new().with_extra(
        json!({"vendorOption": true})
            .as_object()
            .unwrap()
            .clone()
            .into_iter()
            .collect(),
    );
    let cap_val = serde_json::to_value(&cap).expect("serialize MailShareCapability");
    assert_eq!(cap_val["vendorOption"], true);
    let roundtripped_cap: MailShareCapability =
        serde_json::from_value(cap_val).expect("deserialize MailShareCapability");
    assert_eq!(roundtripped_cap, cap);

    let session = Session::new(
        "alice@example.com",
        "https://api.example.com/jmap/",
        "https://api.example.com/download/{blobId}",
        "https://api.example.com/upload/",
        "s1",
    )
    .with_capability(CAPABILITY_MAIL_SHARE, json!({}));

    let session_cap = session
        .mail_share_capability()
        .expect("has mail_share_capability");
    assert_eq!(session_cap, MailShareCapability::new());

    let rights = MailboxRights::read_only().with_may_share(true);
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_share, Some(true));
    assert!(rights.may_share());

    let all_rights = MailboxRights::all();
    assert_eq!(all_rights.may_share, Some(true));
    assert!(all_rights.may_share());

    let mailbox = Mailbox::new("Shared Team Inbox")
        .with_shared_principal("principal_alice", rights.clone())
        .with_shared_principal("principal_bob", MailboxRights::all());

    assert!(mailbox.share_with.is_some());
    let share_map = mailbox.share_with.as_ref().unwrap();
    assert_eq!(share_map.len(), 2);
    assert_eq!(share_map.get(&"principal_alice".into()), Some(&rights));

    let mbx_val = serde_json::to_value(&mailbox).expect("serialize Mailbox");
    assert_eq!(mbx_val["shareWith"]["principal_alice"]["mayShare"], true);
    assert_eq!(
        mbx_val["shareWith"]["principal_alice"]["mayReadItems"],
        true
    );

    let roundtripped_mbx: Mailbox = serde_json::from_value(mbx_val).expect("deserialize Mailbox");
    assert_eq!(roundtripped_mbx, mailbox);
}

#[test]
fn vacation_response_capability_and_mail_types_fluent_builders_roundtrip() {
    use jmap_proto::mail::{
        DeliveryStatus, EmailAddress, EmailAddressGroup, Envelope, EnvelopeAddress, MDN,
        MDNDisposition, Thread, VacationResponse, VacationResponseCapability,
    };
    use jmap_proto::session::{CAPABILITY_VACATION_RESPONSE, Session};
    use serde_json::json;
    use std::collections::BTreeMap;

    assert_eq!(
        CAPABILITY_VACATION_RESPONSE,
        "urn:ietf:params:jmap:vacationresponse"
    );

    // 1. VacationResponseCapability
    let cap = VacationResponseCapability::new()
        .with_extra(BTreeMap::from([("vendorOpt".to_string(), json!(true))]));
    let cap_val = serde_json::to_value(&cap).expect("serialize VacationResponseCapability");
    assert_eq!(cap_val["vendorOpt"], true);
    let roundtripped_cap: VacationResponseCapability =
        serde_json::from_value(cap_val).expect("deserialize VacationResponseCapability");
    assert_eq!(roundtripped_cap, cap);

    let session = Session::new(
        "alice@example.com",
        "https://api.example.com/jmap/",
        "https://api.example.com/download/{blobId}",
        "https://api.example.com/upload/",
        "s1",
    )
    .with_capability(CAPABILITY_VACATION_RESPONSE, json!({}));

    let session_cap = session
        .vacation_response_capability()
        .expect("has vacation_response_capability");
    assert_eq!(session_cap, VacationResponseCapability::new());

    // 2. VacationResponse builders
    let vacation = VacationResponse::new(false)
        .with_id("singleton")
        .with_is_enabled(true)
        .with_subject("On Leave")
        .with_text_body("I am out of office until Monday.")
        .with_html_body("<p>I am out of office until Monday.</p>")
        .with_extra(BTreeMap::from([("customField".to_string(), json!(123))]));
    assert!(vacation.is_enabled);
    assert_eq!(vacation.subject.as_deref(), Some("On Leave"));
    assert_eq!(vacation.extra["customField"], 123);

    // 3. Thread builders
    let thread = Thread::new("T1", ["E1"])
        .with_id("T2")
        .with_email_ids(["E2", "E3"])
        .with_email_id("E4")
        .with_extra(BTreeMap::from([("starred".to_string(), json!(true))]));
    assert_eq!(thread.id.as_ref().unwrap().as_str(), "T2");
    let thread_email_ids: Vec<_> = thread.email_ids.iter().map(|id| id.as_str()).collect();
    assert_eq!(thread_email_ids, vec!["E2", "E3", "E4"]);
    assert_eq!(thread.extra["starred"], true);

    // 4. EmailAddressGroup builders
    let addr_group = EmailAddressGroup::new([EmailAddress::from_email("alice@example.com")])
        .with_name("Engineering")
        .with_addresses([EmailAddress::from_email("bob@example.com")])
        .with_address(EmailAddress::from_email("carol@example.com"));
    assert_eq!(addr_group.name.as_deref(), Some("Engineering"));
    assert_eq!(addr_group.addresses.len(), 2);
    assert_eq!(addr_group.addresses[0].email, "bob@example.com");
    assert_eq!(addr_group.addresses[1].email, "carol@example.com");

    // 5. Envelope and EnvelopeAddress builders
    let env_addr =
        EnvelopeAddress::new("alice@example.com").with_parameters(json!({"auth": "sender"}));
    assert_eq!(env_addr.email, "alice@example.com");
    assert_eq!(env_addr.parameters, Some(json!({"auth": "sender"})));

    let envelope = Envelope::new(
        EnvelopeAddress::new("sender@example.com"),
        [EnvelopeAddress::new("rcpt1@example.com")],
    )
    .with_mail_from(EnvelopeAddress::new("from@example.com"))
    .with_rcpt_to([EnvelopeAddress::new("rcpt2@example.com")])
    .with_recipient(EnvelopeAddress::new("rcpt3@example.com"));
    assert_eq!(envelope.mail_from.email, "from@example.com");
    assert_eq!(envelope.rcpt_to.len(), 2);
    assert_eq!(envelope.rcpt_to[0].email, "rcpt2@example.com");
    assert_eq!(envelope.rcpt_to[1].email, "rcpt3@example.com");

    // 6. DeliveryStatus builders
    let delivery = DeliveryStatus::default()
        .with_smtp_reply("250 2.1.5 Ok")
        .with_delivered("yes")
        .with_displayed("unknown")
        .with_extra(BTreeMap::from([(
            "tlsVersion".to_string(),
            json!("TLSv1.3"),
        )]));
    assert_eq!(delivery.smtp_reply, "250 2.1.5 Ok");
    assert_eq!(delivery.delivered, "yes");
    assert_eq!(delivery.displayed, "unknown");
    assert_eq!(delivery.extra["tlsVersion"], "TLSv1.3");

    // 7. MDN and MDNDisposition builders
    let disp = MDNDisposition::new("manual-action", "mdn-sent-manually", "displayed")
        .with_error("no error")
        .with_modifiers(["error"])
        .with_extra(BTreeMap::from([("dispExtra".to_string(), json!(true))]));
    assert_eq!(disp.action_mode, "manual-action");
    assert_eq!(disp.error.as_deref(), Some("no error"));
    assert_eq!(disp.modifiers.as_ref().unwrap(), &vec!["error".to_string()]);
    assert_eq!(disp.extra["dispExtra"], true);

    let mdn = MDN::new("E1", disp)
        .with_mdn_gateway("mail.example.com")
        .with_original_recipient("rfc822; orig@example.com")
        .with_error(["syntax error"])
        .with_extension_fields(BTreeMap::from([(
            "X-Report".to_string(),
            "Detail".to_string(),
        )]))
        .with_extra(BTreeMap::from([("mdnExtra".to_string(), json!(42))]));
    assert_eq!(mdn.for_email_id.as_str(), "E1");
    assert_eq!(mdn.mdn_gateway.as_deref(), Some("mail.example.com"));
    assert_eq!(
        mdn.original_recipient.as_deref(),
        Some("rfc822; orig@example.com")
    );
    assert_eq!(
        mdn.error.as_ref().unwrap(),
        &vec!["syntax error".to_string()]
    );
    assert_eq!(
        mdn.extension_fields.as_ref().unwrap().get("X-Report"),
        Some(&"Detail".to_string())
    );
    assert_eq!(mdn.extra["mdnExtra"], 42);
}
