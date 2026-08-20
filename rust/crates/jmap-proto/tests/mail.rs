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
