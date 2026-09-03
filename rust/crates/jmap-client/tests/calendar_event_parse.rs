// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalendarEvent/parse` (draft-ietf-jmap-calendars section 5.7) against the
//! mock server: an iCalendar blob turns into a JSCalendar `CalendarEvent`.

use jmap_client::{Client, Credentials};
use jmap_proto::blob::{BlobUploadRequest, UploadBlob};
use jmap_proto::calendars::CalendarEventParseRequest;

const VEVENT: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//test//test//EN\r\n\
BEGIN:VEVENT\r\n\
UID:event-1@example.com\r\n\
DTSTAMP:20260901T090000Z\r\n\
DTSTART:20260901T090000Z\r\n\
DURATION:PT30M\r\n\
SUMMARY:Dentist\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

fn upload(client: &Client, account_id: &jmap_proto::Id, text: &str) -> jmap_proto::Id {
    let created = client
        .blob_upload(
            &BlobUploadRequest::new(account_id.clone())
                .create_blob("b0", UploadBlob::from_text(text, "text/calendar")),
        )
        .expect("blob_upload")
        .created
        .expect("blob created");
    created.get("b0").expect("b0 was created").id.clone()
}

#[test]
fn parses_a_vevent_blob_into_a_calendar_event() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, VEVENT);

    let response = client
        .event_parse(&CalendarEventParseRequest::new(
            account_id.clone(),
            [blob_id.clone()],
        ))
        .expect("event_parse");

    let parsed = response.parsed.expect("parsed map");
    let event = parsed.get(&blob_id).expect("blob was parsed");
    assert_eq!(event.title.as_deref(), Some("Dentist"));
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
        .event_parse(&CalendarEventParseRequest::new(
            account_id,
            [missing.clone()],
        ))
        .expect("event_parse");

    assert!(response.parsed.is_none());
    assert_eq!(response.not_found, Some(vec![missing]));
}

#[test]
fn unparsable_content_is_reported_in_not_parsable() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, "this is not iCalendar text");

    let response = client
        .event_parse(&CalendarEventParseRequest::new(
            account_id,
            [blob_id.clone()],
        ))
        .expect("event_parse");

    assert!(response.parsed.is_none());
    assert_eq!(response.not_parsable, Some(vec![blob_id]));
}

#[test]
fn properties_filters_the_parsed_event() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let blob_id = upload(&client, &account_id, VEVENT);

    let response = client
        .event_parse(
            &CalendarEventParseRequest::new(account_id, [blob_id.clone()]).properties(["title"]),
        )
        .expect("event_parse");

    let parsed = response.parsed.expect("parsed map");
    let event = parsed.get(&blob_id).expect("blob was parsed");
    assert_eq!(event.title.as_deref(), Some("Dentist"));
    assert!(
        event.start.is_none(),
        "start was not requested and should be filtered out"
    );
}

#[test]
fn two_ids_report_independently() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let good = upload(&client, &account_id, VEVENT);
    let bad = upload(&client, &account_id, "garbage");

    let response = client
        .event_parse(&CalendarEventParseRequest::new(
            account_id,
            [good.clone(), bad.clone()],
        ))
        .expect("event_parse");

    assert!(response.parsed.expect("parsed map").contains_key(&good));
    assert_eq!(response.not_parsable, Some(vec![bad]));
}
