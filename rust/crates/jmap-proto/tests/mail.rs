// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the RFC 8621 mail types.

#![cfg(feature = "mail")]

use jmap_proto::mail::{Email, EmailImport, EmailQueryFilter, EmailSubmission, Mailbox};
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
    assert!(rights.may_read_items);
    assert!(!rights.may_add_items);
    assert!(rights.may_set_seen);

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
        may_read_items: true,
        may_add_items: true,
        may_remove_items: false,
        may_set_seen: true,
        may_set_keywords: true,
        may_create_child: false,
        may_rename: false,
        may_delete: false,
        may_submit: false,
        extra: BTreeMap::new(),
    };

    let mailbox = Mailbox {
        id: Some("mb1".into()),
        name: "Shared Support Inbox".to_owned(),
        share_with: Some(BTreeMap::from([("usr_carol".into(), Some(rights.clone()))])),
        my_rights: Some(rights.clone()),
        ..Mailbox::default()
    };

    let mb_val = serde_json::to_value(&mailbox).unwrap();
    assert_eq!(mb_val["shareWith"]["usr_carol"]["maySetSeen"], true);
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
