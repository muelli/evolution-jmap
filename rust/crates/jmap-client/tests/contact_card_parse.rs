// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ContactCard/parse` (RFC 9610 section 3.4) against the mock server: an
//! uploaded vCard blob turns into a `ContactCard`, without being filed into
//! any address book.

use jmap_client::{Client, Credentials};
use jmap_proto::blob::{BlobUploadRequest, UploadBlob};
use jmap_proto::contacts::ContactCardParseRequest;

const VCARD: &str = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Vera Oldenburg\r\n\
N:Oldenburg;Vera;;;\r\n\
EMAIL;TYPE=WORK:vera@example.com\r\n\
END:VCARD\r\n";

fn upload(client: &Client, account_id: &jmap_proto::Id, text: &str) -> jmap_proto::Id {
    let created = client
        .blob_upload(
            &BlobUploadRequest::new(account_id.clone())
                .create_blob("b0", UploadBlob::from_text(text, "text/vcard")),
        )
        .expect("blob_upload")
        .created
        .expect("blob created");
    created.get("b0").expect("b0 was created").id.clone()
}

#[test]
fn parses_a_vcard_blob_into_a_contact_card() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, VCARD);

    let response = client
        .contact_card_parse(&ContactCardParseRequest::new(account_id, [blob_id.clone()]))
        .expect("contact_card_parse");

    let parsed = response.parsed.expect("parsed map");
    let card = parsed.get(&blob_id).expect("blob was parsed");
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("Vera Oldenburg")
    );
    assert_eq!(
        card.emails
            .as_ref()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .address,
        "vera@example.com"
    );
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
        .contact_card_parse(&ContactCardParseRequest::new(account_id, [missing.clone()]))
        .expect("contact_card_parse");

    assert!(response.parsed.is_none());
    assert_eq!(response.not_found, Some(vec![missing]));
}

#[test]
fn unparsable_content_is_reported_in_not_parsable() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, "this is not a vCard");

    let response = client
        .contact_card_parse(&ContactCardParseRequest::new(account_id, [blob_id.clone()]))
        .expect("contact_card_parse");

    assert!(response.parsed.is_none());
    assert_eq!(response.not_parsable, Some(vec![blob_id]));
}

#[test]
fn properties_filters_the_parsed_card() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, VCARD);

    let response = client
        .contact_card_parse(
            &ContactCardParseRequest::new(account_id, [blob_id.clone()]).properties(["name"]),
        )
        .expect("contact_card_parse");

    let parsed = response.parsed.expect("parsed map");
    let card = parsed.get(&blob_id).expect("blob was parsed");
    assert!(card.name.is_some());
    assert!(
        card.emails.is_none(),
        "emails was not requested and should be filtered out"
    );
}

#[test]
fn two_ids_report_independently() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let good = upload(&client, &account_id, VCARD);
    let bad = upload(&client, &account_id, "garbage");

    let response = client
        .contact_card_parse(&ContactCardParseRequest::new(
            account_id,
            [good.clone(), bad.clone()],
        ))
        .expect("contact_card_parse");

    assert!(response.parsed.expect("parsed map").contains_key(&good));
    assert_eq!(response.not_parsable, Some(vec![bad]));
}

#[test]
fn parsed_contact_card_can_be_created_in_address_book() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let book_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Default", true)
    };

    let blob_id = upload(&client, &account_id, VCARD);

    let response = client
        .contact_card_parse(&ContactCardParseRequest::new(
            account_id.clone(),
            [blob_id.clone()],
        ))
        .expect("contact_card_parse");

    let parsed = response.parsed.expect("parsed map");
    let mut card = parsed.get(&blob_id).expect("blob was parsed").clone();
    card.address_book_ids = Some([(book_id.clone(), true)].into());

    let created = client
        .contact_create(&account_id, &card)
        .expect("contact_create");
    let card_id = created.id.expect("the server named the card");

    let fetched = client
        .contact_get(&account_id, &[card_id])
        .expect("contact_get")
        .list
        .into_iter()
        .next()
        .expect("created card found");
    assert_eq!(
        fetched.name.as_ref().unwrap().full.as_deref(),
        Some("Vera Oldenburg")
    );
}
