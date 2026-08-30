// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Property-based fuzzing of the untrusted-server boundary (Track A4,
//! `docs/ROADMAP.md`): a JMAP server is not trusted, so a response's
//! *shape* is hostile input, not a contract this crate's `Deserialize`
//! impls are allowed to assume. Every property here feeds arbitrary JSON
//! straight into an envelope type's deserialization; the only thing
//! asserted is that decoding never panics — an `Err` is a perfectly normal
//! outcome for garbage input, a panic is not.

use proptest::prelude::*;
use serde_json::Value;

/// A bounded-depth, bounded-breadth arbitrary JSON value.
///
/// The bounds exist to keep generation cheap, not to under-approximate
/// hostility: what this test hunts for is a `Deserialize` impl that
/// indexes, unwraps, or slices on an assumption about shape (a missing
/// field, a wrong type, a short array) rather than returning `Err`, and a
/// shallow-but-wide document exercises that exactly as well as a deep one.
/// Parser-level concerns like unbounded nesting depth are `serde_json`'s to
/// harden, not this crate's `Deserialize` impls'.
fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        ".{0,16}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::btree_map(".{0,8}", inner, 0..8)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn arbitrary_json_never_panics_deserializing_session(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::session::Session>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::request::Request>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::response::Response>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_method_error(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::error::MethodError>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_request_error(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::error::RequestError>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_changes_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::ChangesResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_query_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::QueryResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_echo(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::Echo>(&text);
    }


    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_contact_card(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::ContactCard>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_address_book(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::AddressBook>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_event(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarEvent>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::Calendar>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_mailbox(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::Mailbox>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::Email>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_submission(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailSubmission>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_delivery_status(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::DeliveryStatus>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_submission_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailSubmissionQueryFilter>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_search_snippet(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::SearchSnippet>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_address_book_rights(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::AddressBookRights>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_query_changes_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::QueryChangesResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_copy_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::CopyResponse<serde_json::Value>>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_principal(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::Principal>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_get_availability_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::GetAvailabilityResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_push_subscription(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::push::PushSubscription>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_push_verification(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::push::PushVerification>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_state_change(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::push::StateChange>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_parse_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailParseResponse>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_crypto_key(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::CryptoKey>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_directory(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Directory>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_personal_info(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::PersonalInfo>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_card_group(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::CardGroup>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_participant(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::Participant>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_location(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::Location>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_virtual_location(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::VirtualLocation>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_alert(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::Alert>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_preferences(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarPreferences>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_event_parse_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarEventParseResponse>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_share_notification(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::ShareNotification>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_blob_copy_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::BlobCopyResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_core_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::session::CoreCapability>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_mail_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::MailCapability>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_submission_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::SubmissionCapability>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_contacts_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::ContactsCapability>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_speak_to_as(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::SpeakToAs>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_language_pref(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::LanguagePref>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendars_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarsCapability>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_absolute_trigger(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::AbsoluteTrigger>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_event_relation(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::EventRelation>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_principals_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::PrincipalsCapability>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_identity(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::Identity>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_contact_card_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::ContactCardQueryFilter>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_event_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarEventQueryFilter>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailQueryFilter>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_result_reference(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::request::ResultReference>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_contact_card_parse_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::ContactCardParseResponse>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_contact_card_parse_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::ContactCardParseRequest>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_get_free_busy_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::GetFreeBusyResponse>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_get_free_busy_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::GetFreeBusyRequest>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_free_busy_block(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::FreeBusyBlock>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_get_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::GetRequest>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_get_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::GetResponse<serde_json::Value>>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_set_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::SetRequest<serde_json::Value>>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_set_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::SetResponse<serde_json::Value>>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_query_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::QueryRequest<serde_json::Value>>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_query_changes_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::QueryChangesRequest<serde_json::Value>>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_copy_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::CopyRequest<serde_json::Value>>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_blob_copy_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::BlobCopyRequest>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_comparator(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::Comparator>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_added_item(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::AddedItem>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_upload_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::methods::UploadResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_set_error(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::error::SetError>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_import_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailImportRequest>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_import_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailImportResponse>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_parse_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailParseRequest>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_submission_set_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailSubmissionSetRequest>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_envelope(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::Envelope>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_envelope_address(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EnvelopeAddress>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_header(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailHeader>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_address(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailAddress>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_address_group(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailAddressGroup>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_body_part(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailBodyPart>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_email_body_value(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::EmailBodyValue>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_vacation_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::VacationResponse>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_mailbox_rights(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::MailboxRights>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_thread(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::Thread>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_name(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Name>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_name_component(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::NameComponent>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_nickname(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Nickname>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_contacts_email_address(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::ContactEmail>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_phone(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::ContactPhone>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_organization(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Organization>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_org_unit(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::OrgUnit>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_title(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Title>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_address(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Address>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_address_component(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::AddressComponent>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_note(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Note>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_anniversary(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Anniversary>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_link(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Link>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_contact(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Calendar>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_media(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Media>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_online_service(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::OnlineService>(&text);
    }

    #[cfg(feature = "contacts")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_relation(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::contacts::Relation>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_rights(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarRights>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_recurrence_rule(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::RecurrenceRule>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_nday(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::NDay>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_offset_trigger(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::OffsetTrigger>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_event_parse_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarEventParseRequest>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_principal_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::PrincipalQueryFilter>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_get_availability_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::GetAvailabilityRequest>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_busy_period(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::BusyPeriod>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_push_subscription_keys(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::push::PushSubscriptionKeys>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_websocket_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::session::WebSocketCapability>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_mdn(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::MDN>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_mdn_disposition(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::MDNDisposition>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_mdn_send_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::MDNSendRequest>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_mdn_send_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::MDNSendResponse>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_search_snippet_get_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::SearchSnippetGetRequest>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_search_snippet_get_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::SearchSnippetGetResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_quota(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::quota::Quota>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_quota_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::quota::QuotaQueryFilter>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_quota_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::quota::QuotaCapability>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_blob_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::blob::BlobCapability>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_blob_info(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::blob::BlobInfo>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_blob_get_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::blob::BlobGetRequest>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_blob_get_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::blob::BlobGetResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_upload_blob(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::blob::UploadBlob>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_upload_blob_result(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::blob::UploadBlobResult>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_blob_upload_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::blob::BlobUploadRequest>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_blob_upload_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::blob::BlobUploadResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_task(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::tasks::Task>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_task_list(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::tasks::TaskList>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_task_list_rights(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::tasks::TaskListRights>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_tasks_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::tasks::TasksCapability>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_task_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::tasks::TaskQueryFilter>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_sieve_script(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::sieve::SieveScript>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_sieve_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::sieve::SieveCapability>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_sieve_script_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::sieve::SieveScriptQueryFilter>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_sieve_script_validate_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::sieve::SieveScriptValidateRequest>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_sieve_script_validate_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::sieve::SieveScriptValidateResponse>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_sieve_script_validate_error(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::sieve::SieveScriptValidateError>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_group(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarGroup>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_preferences_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarPreferencesCapability>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_mdn_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::MDNCapability>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_event_notification(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarEventNotification>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_event_notification_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarEventNotificationQueryFilter>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_event_send_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarEventSendRequest>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_send_calendar_event(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::SendCalendarEvent>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_calendar_event_send_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::CalendarEventSendResponse>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_send_calendar_event_result(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::SendCalendarEventResult>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_participant_problem(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::ParticipantProblem>(&text);
    }

    #[cfg(feature = "calendars")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_participant_reply(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::calendars::ParticipantReply>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_share_notification_query_filter(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::ShareNotificationQueryFilter>(&text);
    }

    #[cfg(feature = "principals")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_principals_owner_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::principals::PrincipalsOwnerCapability>(&text);
    }

    #[cfg(feature = "mail")]
    #[test]
    fn arbitrary_json_never_panics_deserializing_smime_verify_capability(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::mail::SmimeVerifyCapability>(&text);
    }
}
