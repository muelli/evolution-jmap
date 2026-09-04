// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Email/parse` (RFC 8621 section 4.8) against the mock server: an RFC 5322
//! blob turns into an `Email`, without being imported into the mailbox.

use jmap_client::{Client, Credentials};
use jmap_proto::blob::{BlobUploadRequest, UploadBlob};
use jmap_proto::mail::EmailParseRequest;

const MESSAGE: &str = "From: alice@example.com\r\n\
To: bob@example.com\r\n\
Subject: Dentist\r\n\
Message-ID: <event-1@example.com>\r\n\
\r\n\
See you at three.\r\n";

fn upload(client: &Client, account_id: &jmap_proto::Id, text: &str) -> jmap_proto::Id {
    let created = client
        .blob_upload(
            &BlobUploadRequest::new(account_id.clone())
                .create_blob("b0", UploadBlob::from_text(text, "message/rfc822")),
        )
        .expect("blob_upload")
        .created
        .expect("blob created");
    created.get("b0").expect("b0 was created").id.clone()
}

#[test]
fn parses_a_message_blob_into_an_email() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, MESSAGE);

    let response = client
        .email_parse(&EmailParseRequest::new(account_id, [blob_id.clone()]))
        .expect("email_parse");

    let parsed = response.parsed.expect("parsed map");
    let email = parsed.get(&blob_id).expect("blob was parsed");
    assert_eq!(email.subject.as_deref(), Some("Dentist"));
    assert_eq!(email.from.as_ref().unwrap()[0].email, "alice@example.com");
    assert_eq!(email.to.as_ref().unwrap()[0].email, "bob@example.com");
    assert_eq!(
        email.message_id.as_deref(),
        Some(&["event-1@example.com".to_owned()][..])
    );
    assert_eq!(email.preview.as_deref(), Some("See you at three."));
    assert!(response.not_found.is_none());
    assert!(response.not_parsable.is_none());
}

#[test]
fn a_missing_blob_id_is_reported_in_not_found() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let missing = jmap_proto::Id::new("no-such-blob");
    let response = client
        .email_parse(&EmailParseRequest::new(account_id, [missing.clone()]))
        .expect("email_parse");

    assert!(response.parsed.is_none());
    assert_eq!(response.not_found, Some(vec![missing]));
}

#[test]
fn unparsable_content_is_reported_in_not_parsable() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, "this is not an RFC 5322 message");

    let response = client
        .email_parse(&EmailParseRequest::new(account_id, [blob_id.clone()]))
        .expect("email_parse");

    assert!(response.parsed.is_none());
    assert_eq!(response.not_parsable, Some(vec![blob_id]));
}

#[test]
fn properties_filters_the_parsed_email() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, MESSAGE);

    let response = client
        .email_parse(&EmailParseRequest::new(account_id, [blob_id.clone()]).properties(["subject"]))
        .expect("email_parse");

    let parsed = response.parsed.expect("parsed map");
    let email = parsed.get(&blob_id).expect("blob was parsed");
    assert_eq!(email.subject.as_deref(), Some("Dentist"));
    assert!(
        email.from.is_none(),
        "from was not requested and should be filtered out"
    );
}

#[test]
fn two_ids_report_independently() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let good = upload(&client, &account_id, MESSAGE);
    let bad = upload(&client, &account_id, "garbage");

    let response = client
        .email_parse(&EmailParseRequest::new(
            account_id,
            [good.clone(), bad.clone()],
        ))
        .expect("email_parse");

    assert!(response.parsed.expect("parsed map").contains_key(&good));
    assert_eq!(response.not_parsable, Some(vec![bad]));
}

#[test]
fn no_mime_tree_is_produced_matching_email_import() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, MESSAGE);

    let response = client
        .email_parse(&EmailParseRequest::new(account_id, [blob_id.clone()]))
        .expect("email_parse");

    let parsed = response.parsed.expect("parsed map");
    let email = parsed.get(&blob_id).expect("blob was parsed");
    assert!(email.text_body.is_none());
    assert!(email.html_body.is_none());
    assert!(email.attachments.is_none());
    assert!(email.body_structure.is_none());
    assert!(email.body_values.is_none());
    assert!(email.id.is_none());
    assert!(email.thread_id.is_none());
}
