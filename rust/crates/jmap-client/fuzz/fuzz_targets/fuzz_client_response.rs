// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials};
use jmap_proto::Id;

const MINIMAL_AUTHENTICATED_SESSION: &str = r#"{"capabilities":{"urn:ietf:params:jmap:core":{"maxSizeUpload":50000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-numeric","i;ascii-casemap","i;unicode-casemap"]},"urn:ietf:params:jmap:mail":{},"urn:ietf:params:jmap:contacts":{},"urn:ietf:params:jmap:calendars":{},"urn:ietf:params:jmap:principals":{},"urn:ietf:params:jmap:sieve":{},"urn:ietf:params:jmap:quota":{},"urn:ietf:params:jmap:blob":{}},"accounts":{"acc1":{"name":"user","isPersonal":true,"isReadOnly":false,"accountCapabilities":{"urn:ietf:params:jmap:core":{},"urn:ietf:params:jmap:mail":{},"urn:ietf:params:jmap:contacts":{},"urn:ietf:params:jmap:calendars":{},"urn:ietf:params:jmap:principals":{},"urn:ietf:params:jmap:sieve":{},"urn:ietf:params:jmap:quota":{},"urn:ietf:params:jmap:blob":{}}}},"primaryAccounts":{"urn:ietf:params:jmap:mail":"acc1","urn:ietf:params:jmap:contacts":"acc1","urn:ietf:params:jmap:calendars":"acc1","urn:ietf:params:jmap:principals":"acc1"},"username":"user","apiUrl":"https://mail.example.internal/jmap/","downloadUrl":"https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}","uploadUrl":"https://mail.example.internal/jmap/upload/{accountId}/","eventSourceUrl":"https://mail.example.internal/jmap/eventsource/","state":"0"}"#;

struct FuzzTransport {
    response_body: Vec<u8>,
}

impl Transport for FuzzTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let (status, body) = match request.method {
            HttpMethod::Get => (200, MINIMAL_AUTHENTICATED_SESSION.as_bytes().to_vec()),
            HttpMethod::Post => (200, self.response_body.clone()),
        };
        Ok(HttpResponse {
            status,
            content_type: Some("application/json".to_owned()),
            body,
            final_url: request.url.to_owned(),
        })
    }
}

struct FuzzSessionTransport {
    session_body: Vec<u8>,
}

impl Transport for FuzzSessionTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: self.session_body.clone(),
            final_url: request.url.to_owned(),
        })
    }
}

fuzz_target!(|data: &[u8]| {
    // Verify that arbitrary responses do not panic during session connection or method dispatch.
    let session_transport = FuzzSessionTransport {
        session_body: data.to_vec(),
    };
    if let Ok(mut session_client) = Client::builder()
        .transport(session_transport)
        .connect("https://mail.example.internal", Credentials::none())
    {
        let _ = session_client.is_session_stale();
        let _ = session_client.last_session_state();
        let _ = session_client.refresh_session();
    }

    let transport = FuzzTransport {
        response_body: data.to_vec(),
    };
    if let Ok(client) = Client::builder()
        .transport(transport)
        .connect("https://mail.example.internal", Credentials::none())
    {
        let account_id = Id::new("acc1");
        let _ = client.mailbox_get(&account_id);
        let _ = client.email_get(&account_id, &[Id::new("e1")], None);
        let _ = client.calendars(&account_id);
        let _ = client.event_get(&account_id, &[Id::new("ev1")]);
        let _ = client.address_books(&account_id);
        let _ = client.contact_get(&account_id, &[Id::new("c1")]);
        let _ = client.changes(&account_id, "Mailbox", &jmap_proto::State::new("0"));
        let _ = client.changes(&account_id, "Email", &jmap_proto::State::new("0"));
        let _ = client.changes(&account_id, "ContactCard", &jmap_proto::State::new("0"));
        let _ = client.changes(&account_id, "CalendarEvent", &jmap_proto::State::new("0"));
        let _ = client.download_blob(&account_id, &Id::new("b1"), "test.txt", 1024);
        let _ = client.upload_blob(&account_id, "application/octet-stream", vec![0]);
        let _ = client.echo(serde_json::json!({}));
        let _ = client.principal_get(&account_id, &[Id::new("p1")]);
        let _ = client.principals(&account_id);
        let _ = client.participant_identities(&account_id);
        let _ = client.calendar_event_notifications(&account_id);
        let _ = client.share_notifications(&account_id);
        let _ = client.vacation_response_get(&account_id);
        let _ = client.identities(&account_id);
        let _ = client.thread_get(&account_id, [Id::new("t1")]);
        let _ = client.mailbox_create(
            &account_id,
            &jmap_proto::mail::Mailbox::new("fuzz_test_folder"),
        );
        let _ = client.mailbox_destroy(&account_id, &Id::new("m1"));
        let _ = client.calendar_create(
            &account_id,
            &jmap_proto::calendars::Calendar::new("fuzz_cal"),
        );
        let _ = client.calendar_destroy(&account_id, &Id::new("c1"));
        let _ = client.contact_create(&account_id, &jmap_proto::contacts::ContactCard::new());
        let _ = client.contact_destroy(&account_id, &Id::new("card1"));
        let _ = client.event_create(&account_id, &jmap_proto::calendars::CalendarEvent::new());
        let _ = client.event_destroy(&account_id, &Id::new("ev1"));
        let _ = client.event_parse(&jmap_proto::calendars::CalendarEventParseRequest::new(
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.contact_card_parse(&jmap_proto::contacts::ContactCardParseRequest::new(
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.sieve_script_get(&account_id, &Id::new("s1"));
        let _ = client.sieve_script_active(&account_id);
        let _ = client.sieve_script_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.principal_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.share_notification_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.calendar_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.event_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.participant_identity_changes(&account_id, &jmap_proto::State::new("0"));
        let _ =
            client.calendar_event_notification_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.events(&account_id);
        let _ = client.contact_cards(&account_id);
        let _ = client.quotas(&account_id);
        let _ = client.quota_query(&account_id, jmap_proto::quota::QuotaQueryFilter::new());
        let _ = client.blob_get(&jmap_proto::blob::BlobGetRequest::new(
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.blob_lookup(&jmap_proto::blob::BlobLookupRequest::new(
            account_id.clone(),
            ["Email"],
            [Id::new("b1")],
        ));
        let _ = client.blob_copy(&jmap_proto::methods::BlobCopyRequest::new(
            account_id.clone(),
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.contact_copy(&jmap_proto::methods::CopyRequest::new(
            account_id.clone(),
            account_id.clone(),
        ));
        let _ = client.contact_query_changes(&jmap_proto::methods::QueryChangesRequest::new(
            account_id.clone(),
            jmap_proto::State::new("0"),
        ));
        let _ = client.event_copy(&jmap_proto::methods::CopyRequest::new(
            account_id.clone(),
            account_id.clone(),
        ));
        let _ = client.event_query_changes(&jmap_proto::methods::QueryChangesRequest::new(
            account_id.clone(),
            jmap_proto::State::new("0"),
        ));
        let _ = client.calendar_event_notification_query(
            &account_id,
            jmap_proto::calendars::CalendarEventNotificationQueryFilter::new(),
        );
        let _ = client.calendar_event_notification_destroy(&account_id, &Id::new("notif1"));
        let _ = client.email_copy(&jmap_proto::methods::CopyRequest::new(
            account_id.clone(),
            account_id.clone(),
        ));
        let _ = client.search_snippet_get(
            &account_id,
            vec![Id::new("e1")],
            None::<jmap_proto::mail::EmailQueryFilter>,
        );
        let _ = client.share_notification_get(&account_id, &[Id::new("sn1")]);
        let _ = client.share_notification_query(
            &account_id,
            jmap_proto::principals::ShareNotificationQueryFilter::new(),
        );
        let _ = client.share_notification_query_changes(
            &jmap_proto::methods::QueryChangesRequest::new(
                account_id.clone(),
                jmap_proto::State::new("0"),
            ),
        );
        let _ = client.share_notification_destroy(&account_id, &Id::new("sn1"));
        let _ = client.sieve_scripts(&account_id);
        let _ = client.sieve_script_query(
            &account_id,
            jmap_proto::sieve::SieveScriptQueryFilter::new(),
        );
        let _ = client.get_availability(
            &account_id,
            &Id::new("p1"),
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
            false,
        );
        let _ = client.snoozed_mailbox(&account_id);
        let _ = client.current_user_principal(&account_id);
        let _ = client.current_user_principal_id(&account_id);
        let _ = client.principal_query(
            &account_id,
            jmap_proto::principals::PrincipalQueryFilter::default(),
        );
        let _ = client.principal_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::principals::Principal,
        >::new(account_id.clone()));
        let _ = client.principal_query_changes(&jmap_proto::methods::QueryChangesRequest::new(
            account_id.clone(),
            jmap_proto::State::new("0"),
        ));
        let _ = client.share_notification_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::principals::ShareNotification,
        >::new(account_id.clone()));
        let _ = client.mailbox_update(
            &account_id,
            &Id::new("m1"),
            serde_json::json!({"name": "renamed"}),
        );
        let _ = client.mailbox_set(
            &jmap_proto::methods::SetRequest::<jmap_proto::mail::Mailbox>::new(account_id.clone()),
        );
        let _ = client.vacation_response_get_with_state(&account_id);
        let _ =
            client.vacation_response_update(&account_id, serde_json::json!({"isEnabled": true}));
        let _ = client.vacation_response_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::mail::VacationResponse,
        >::new(account_id.clone()));
        let _ = client.email_state(&account_id);
        let _ = client.email_query(
            &account_id,
            jmap_proto::mail::EmailQueryFilter::default(),
            None,
            None,
            0,
        );
        let _ = client.email_query_every_id(
            &account_id,
            jmap_proto::mail::EmailQueryFilter::default(),
            None,
        );
        let _ = client.email_query_then_get(
            &account_id,
            jmap_proto::mail::EmailQueryFilter::default(),
            None,
            None,
        );
        let _ = client.email_set(
            &jmap_proto::methods::SetRequest::<jmap_proto::mail::Email>::new(account_id.clone()),
        );
        let _ = client.email_update(
            &account_id,
            &Id::new("e1"),
            serde_json::json!({"keywords": {"$seen": true}}),
        );
        let _ = client.email_destroy(&account_id, &Id::new("e1"));
        let _ = client.email_import(
            &account_id,
            &jmap_proto::mail::EmailImport::new(Id::new("b1"), Id::new("m1")),
        );
        let _ = client.email_parse(&jmap_proto::mail::EmailParseRequest::new(
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.email_submission_get(&account_id, [Id::new("sub1")]);
        let _ = client.email_submission_query(
            &account_id,
            jmap_proto::mail::EmailSubmissionQueryFilter::new(),
        );
        let _ = client.cancel_email_submission(&account_id, &Id::new("sub1"));
        let _ = client.sieve_script_create(
            &account_id,
            &jmap_proto::sieve::SieveScript::new("script_name", Id::new("b1")),
        );
        let _ = client.sieve_script_activate(&account_id, &Id::new("s1"));
        let _ = client.sieve_script_deactivate(&account_id);
        let _ = client.sieve_script_destroy(&account_id, &Id::new("s1"));
        let _ = client.calendar_update(
            &account_id,
            &Id::new("c1"),
            serde_json::json!({"name": "renamed"}),
        );
        let _ = client.calendar_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::calendars::Calendar,
        >::new(account_id.clone()));
        let _ = client.calendar_event_state(&account_id);
        let _ = client.event_update(
            &account_id,
            &Id::new("ev1"),
            serde_json::json!({"title": "updated"}),
        );
        let _ = client.event_set(&jmap_proto::calendars::CalendarEventSetRequest::new(
            jmap_proto::methods::SetRequest::new(account_id.clone()),
        ));
        let _ = client.event_query(
            &account_id,
            jmap_proto::calendars::CalendarEventQueryFilter::default(),
        );
        let _ = client.event_query_every_id(
            &account_id,
            jmap_proto::calendars::CalendarEventQueryFilter::default(),
        );
        let _ = client.participant_identity_create(
            &account_id,
            &jmap_proto::calendars::ParticipantIdentity::new("pi"),
        );
        let _ = client.participant_identity_update(
            &account_id,
            &Id::new("pi1"),
            serde_json::json!({"name": "pi"}),
        );
        let _ = client.participant_identity_destroy(&account_id, &Id::new("pi1"));
        let _ = client.participant_identity_set_default(&account_id, &Id::new("pi1"));
        let _ = client.contact_update(
            &account_id,
            &Id::new("card1"),
            serde_json::json!({"notes": "note"}),
        );
        let _ = client.contact_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::contacts::ContactCard,
        >::new(account_id.clone()));
        let _ = client.contact_state(&account_id);
        let _ =
            client.address_book_create(&account_id, &jmap_proto::contacts::AddressBook::new("ab"));
        let _ = client.address_book_destroy(&account_id, &Id::new("ab1"));
        let _ = client.address_book_update(
            &account_id,
            &Id::new("ab1"),
            serde_json::json!({"name": "ab_renamed"}),
        );
        let _ = client.address_book_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::contacts::AddressBook,
        >::new(account_id.clone()));
        let _ = client.contact_query(
            &account_id,
            jmap_proto::contacts::ContactCardQueryFilter::default(),
        );
        let _ = client.contact_query_every_id(
            &account_id,
            jmap_proto::contacts::ContactCardQueryFilter::default(),
        );
        let _ = client.all_changes(&account_id, "Mailbox", &jmap_proto::State::new("0"));
        let _ = client.all_changes(&account_id, "Email", &jmap_proto::State::new("0"));
        let _ = client.blob_upload(&jmap_proto::blob::BlobUploadRequest::new(
            account_id.clone(),
        ));
        let _ = client.snooze_email(
            &account_id,
            &Id::new("e1"),
            &Id::new("m_snooze"),
            &jmap_proto::mail::SnoozeDetails::new(jmap_proto::UtcDate::new("2026-12-01T00:00:00Z")),
        );
        let _ = client.is_session_stale();
        let _ = client.last_session_state();
    }
});
