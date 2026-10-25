// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Property tests for jmap-proto serde implementations.
//! Verifies roundtrip serialization and preservation of extra properties.

use std::collections::BTreeMap;

use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventSetRequest, CalendarRights, CalendarSetRequest, NDay,
    RecurrenceRule,
};
use jmap_proto::contacts::{
    Address, AddressBook, AddressBookRights, AddressBookSetRequest, Anniversary, Author,
    ContactCard, ContactEmail, ContactPhone, Name, NameComponent, Note, Organization, PartialDate,
    Title,
};
use jmap_proto::error::SetError;
use jmap_proto::id::Id;
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailGetRequest, EmailHeader, Identity,
    Mailbox, MailboxRights, SnoozeDetails,
};
use jmap_proto::methods::{GetRequest, GetResponse, SetRequest, SetResponse};
use jmap_proto::request::ResultReference;
use jmap_proto::state::{State, UtcDate};
use proptest::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn arb_json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-10000..10000i64).prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9 _-]{0,24}".prop_map(Value::String),
    ];
    leaf.prop_recursive(2, 8, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Value::Array),
            prop::collection::btree_map("[a-z0-9_]{1,8}", inner, 0..3)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

fn arb_unknown_key() -> impl Strategy<Value = String> {
    "[a-z0-9_]{1,12}".prop_map(|s| format!("x_custom_{s}"))
}

fn arb_extra() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(arb_unknown_key(), arb_json_value(), 0..2)
}

fn arb_id() -> impl Strategy<Value = Id> {
    "[a-z0-9_-]{1,16}".prop_map(Id::from)
}

fn arb_state() -> impl Strategy<Value = State> {
    "[a-z0-9]{4,16}".prop_map(State::from)
}

fn arb_utc_date() -> impl Strategy<Value = UtcDate> {
    (
        2020..2035u32,
        1..=12u32,
        1..=28u32,
        0..=23u32,
        0..=59u32,
        0..=59u32,
    )
        .prop_map(|(y, m, d, h, min, s)| {
            UtcDate::new(format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z"))
        })
}

fn arb_result_reference() -> impl Strategy<Value = ResultReference> {
    (
        "[a-z0-9]{1,8}",
        "[a-zA-Z0-9/]{1,16}",
        "/list/*/[a-z0-9]{1,8}",
    )
        .prop_map(|(res, name, path)| ResultReference::new(res, name, path))
}

prop_compose! {
    fn arb_mailbox_rights()(
        may_read_items in any::<Option<bool>>(),
        may_add_items in any::<Option<bool>>(),
        may_remove_items in any::<Option<bool>>(),
        may_set_seen in any::<Option<bool>>(),
        may_set_keywords in any::<Option<bool>>(),
        may_create_child in any::<Option<bool>>(),
        may_rename in any::<Option<bool>>(),
        may_delete in any::<Option<bool>>(),
        may_submit in any::<Option<bool>>(),
        may_share in any::<Option<bool>>(),
        extra in arb_extra(),
    ) -> MailboxRights {
        let mut rights = MailboxRights::default();
        rights.may_read_items = may_read_items;
        rights.may_add_items = may_add_items;
        rights.may_remove_items = may_remove_items;
        rights.may_set_seen = may_set_seen;
        rights.may_set_keywords = may_set_keywords;
        rights.may_create_child = may_create_child;
        rights.may_rename = may_rename;
        rights.may_delete = may_delete;
        rights.may_submit = may_submit;
        rights.may_share = may_share;
        rights.extra = extra;
        rights
    }
}

prop_compose! {
    fn arb_mailbox()(
        id in prop::option::of(arb_id()),
        name in "[A-Za-z0-9 _-]{1,24}",
        parent_id in prop::option::of(arb_id()),
        role in prop::option::of(prop_oneof![
            Just("inbox".to_string()),
            Just("sent".to_string()),
            Just("trash".to_string()),
            Just("archive".to_string()),
            Just("drafts".to_string()),
        ]),
        sort_order in any::<Option<u32>>(),
        total_emails in any::<Option<u64>>(),
        unread_emails in any::<Option<u64>>(),
        total_threads in any::<Option<u64>>(),
        unread_threads in any::<Option<u64>>(),
        is_subscribed in any::<Option<bool>>(),
        share_with in prop::option::of(prop::collection::btree_map(arb_id(), arb_mailbox_rights(), 0..2)),
        my_rights in prop::option::of(arb_mailbox_rights()),
        extra in arb_extra(),
    ) -> Mailbox {
        let mut mailbox = Mailbox::default();
        mailbox.id = id;
        mailbox.name = name;
        mailbox.parent_id = parent_id;
        mailbox.role = role;
        mailbox.sort_order = sort_order;
        mailbox.total_emails = total_emails;
        mailbox.unread_emails = unread_emails;
        mailbox.total_threads = total_threads;
        mailbox.unread_threads = unread_threads;
        mailbox.is_subscribed = is_subscribed;
        mailbox.share_with = share_with;
        mailbox.my_rights = my_rights;
        mailbox.extra = extra;
        mailbox
    }
}

prop_compose! {
    fn arb_email_address()(
        name in prop::option::of("[A-Za-z ]{1,20}"),
        email in "[a-z0-9]{1,10}@[a-z0-9]{1,10}\\.com",
    ) -> EmailAddress {
        let mut address = EmailAddress::default();
        address.name = name;
        address.email = email;
        address
    }
}

prop_compose! {
    fn arb_email_header()(
        name in "[A-Za-z-]{1,16}",
        value in "[A-Za-z0-9 ;=_-]{1,32}",
    ) -> EmailHeader {
        EmailHeader::new(name, value)
    }
}

prop_compose! {
    fn arb_email_body_value()(
        value in "[A-Za-z0-9 \n\r]{0,32}",
        is_encoding_problem in any::<bool>(),
        is_truncated in any::<bool>(),
    ) -> EmailBodyValue {
        let mut body_value = EmailBodyValue::new(value);
        body_value.is_encoding_problem = is_encoding_problem;
        body_value.is_truncated = is_truncated;
        body_value
    }
}

prop_compose! {
    fn arb_email_body_part()(
        part_id in prop::option::of("[0-9]{1,3}"),
        blob_id in prop::option::of(arb_id()),
        size in any::<Option<u64>>(),
        name in prop::option::of("[A-Za-z0-9._-]{1,20}"),
        content_type in prop::option::of(prop_oneof![
            Just("text/plain".to_string()),
            Just("text/html".to_string()),
            Just("application/pdf".to_string()),
        ]),
        charset in prop::option::of("utf-8"),
        disposition in prop::option::of("inline"),
        cid in prop::option::of("<cid123@example.com>"),
        extra in arb_extra(),
    ) -> EmailBodyPart {
        let mut part = EmailBodyPart::default();
        part.part_id = part_id;
        part.blob_id = blob_id;
        part.size = size;
        part.name = name;
        part.content_type = content_type;
        part.charset = charset;
        part.disposition = disposition;
        part.cid = cid;
        part.extra = extra;
        part
    }
}

prop_compose! {
    fn arb_snooze_details()(
        until in arb_utc_date(),
        move_to_mailbox_id in prop::option::of(arb_id()),
        set_keywords in prop::option::of(prop::collection::btree_map("[a-z]{1,8}", any::<bool>(), 0..2)),
        extra in arb_extra(),
    ) -> SnoozeDetails {
        let mut details = SnoozeDetails::new(until);
        details.move_to_mailbox_id = move_to_mailbox_id;
        details.set_keywords = set_keywords;
        details.extra = extra;
        details
    }
}

prop_compose! {
    fn arb_email_meta()(
        id in prop::option::of(arb_id()),
        blob_id in prop::option::of(arb_id()),
        thread_id in prop::option::of(arb_id()),
        mailbox_ids in prop::option::of(prop::collection::btree_map(arb_id(), any::<bool>(), 0..2)),
        keywords in prop::option::of(prop::collection::btree_map("[a-z$]{1,8}", any::<bool>(), 0..2)),
        snoozed in prop::option::of(arb_snooze_details()),
        size in any::<Option<u64>>(),
        received_at in prop::option::of(arb_utc_date()),
        extra in arb_extra(),
    ) -> (
        Option<Id>,
        Option<Id>,
        Option<Id>,
        Option<BTreeMap<Id, bool>>,
        Option<BTreeMap<String, bool>>,
        Option<SnoozeDetails>,
        Option<u64>,
        Option<UtcDate>,
        BTreeMap<String, Value>,
    ) {
        (
            id,
            blob_id,
            thread_id,
            mailbox_ids,
            keywords,
            snoozed,
            size,
            received_at,
            extra,
        )
    }
}

prop_compose! {
    fn arb_email_envelope()(
        message_id in prop::option::of(prop::collection::vec("[a-z0-9@.-]{1,24}", 0..2)),
        from in prop::option::of(prop::collection::vec(arb_email_address(), 0..2)),
        to in prop::option::of(prop::collection::vec(arb_email_address(), 0..2)),
        subject in prop::option::of("[A-Za-z0-9 _-]{0,32}"),
    ) -> (
        Option<Vec<String>>,
        Option<Vec<EmailAddress>>,
        Option<Vec<EmailAddress>>,
        Option<String>,
    ) {
        (message_id, from, to, subject)
    }
}

prop_compose! {
    fn arb_email_content()(
        has_attachment in any::<Option<bool>>(),
        preview in prop::option::of("[A-Za-z0-9 _-]{0,32}"),
        body_structure in prop::option::of(arb_email_body_part()),
        body_values in prop::option::of(prop::collection::btree_map("[0-9]{1,2}", arb_email_body_value(), 0..2)),
        headers in prop::option::of(prop::collection::vec(arb_email_header(), 0..2)),
    ) -> (
        Option<bool>,
        Option<String>,
        Option<EmailBodyPart>,
        Option<BTreeMap<String, EmailBodyValue>>,
        Option<Vec<EmailHeader>>,
    ) {
        (has_attachment, preview, body_structure, body_values, headers)
    }
}

prop_compose! {
    fn arb_email()(
        (id, blob_id, thread_id, mailbox_ids, keywords, snoozed, size, received_at, extra) in arb_email_meta(),
        (message_id, from, to, subject) in arb_email_envelope(),
        (has_attachment, preview, body_structure, body_values, headers) in arb_email_content(),
    ) -> Email {
        Email {
            id,
            blob_id,
            thread_id,
            mailbox_ids,
            keywords,
            snoozed,
            size,
            received_at,
            message_id,
            in_reply_to: None,
            references: None,
            sender: None,
            from,
            to,
            cc: None,
            bcc: None,
            reply_to: None,
            subject,
            sent_at: None,
            has_attachment,
            preview,
            body_structure,
            body_values,
            text_body: None,
            html_body: None,
            headers,
            attachments: None,
            smime_status: None,
            smime_errors: None,
            smime_verified_at: None,
            extra,
        }
    }
}

prop_compose! {
    fn arb_identity()(
        id in prop::option::of(arb_id()),
        name in "[A-Za-z ]{1,24}",
        email in "[a-z0-9]{1,10}@[a-z0-9]{1,10}\\.com",
        reply_to in prop::option::of(prop::collection::vec(arb_email_address(), 0..2)),
        bcc in prop::option::of(prop::collection::vec(arb_email_address(), 0..2)),
        text_signature in prop::option::of("[A-Za-z0-9 \n]{0,32}"),
        html_signature in prop::option::of("<p>signature</p>"),
        draft_mailbox_id in prop::option::of(arb_id()),
        may_delete in any::<Option<bool>>(),
        may_send in any::<Option<bool>>(),
        extra in arb_extra(),
    ) -> Identity {
        let mut identity = Identity::default();
        identity.id = id;
        identity.name = name;
        identity.email = email;
        identity.reply_to = reply_to;
        identity.bcc = bcc;
        identity.text_signature = text_signature;
        identity.html_signature = html_signature;
        identity.draft_mailbox_id = draft_mailbox_id;
        identity.may_delete = may_delete;
        identity.may_send = may_send;
        identity.extra = extra;
        identity
    }
}

prop_compose! {
    fn arb_address_book_rights()(
        may_read in any::<Option<bool>>(),
        may_write in any::<Option<bool>>(),
        may_share in any::<Option<bool>>(),
        may_delete in any::<Option<bool>>(),
        extra in arb_extra(),
    ) -> AddressBookRights {
        AddressBookRights {
            may_read,
            may_write,
            may_share,
            may_delete,
            extra,
        }
    }
}

prop_compose! {
    fn arb_address_book()(
        id in prop::option::of(arb_id()),
        name in "[A-Za-z0-9 _-]{1,24}",
        description in prop::option::of("[A-Za-z0-9 _-]{0,32}"),
        sort_order in any::<Option<u32>>(),
        is_default in any::<Option<bool>>(),
        is_subscribed in any::<Option<bool>>(),
        my_rights in prop::option::of(arb_address_book_rights()),
        share_with in prop::option::of(prop::collection::btree_map(arb_id(), arb_address_book_rights(), 0..2)),
        extra in arb_extra(),
    ) -> AddressBook {
        AddressBook {
            id,
            name,
            description,
            sort_order,
            is_default,
            is_subscribed,
            my_rights,
            share_with,
            may_delete: None,
            extra,
        }
    }
}

prop_compose! {
    fn arb_name_component()(
        value in "[A-Za-z]{1,16}",
        kind in prop_oneof![
            Just("prefix".to_string()),
            Just("given".to_string()),
            Just("surname".to_string()),
            Just("suffix".to_string()),
        ],
    ) -> NameComponent {
        NameComponent::new(value, kind)
    }
}

prop_compose! {
    fn arb_name()(
        full in prop::option::of("[A-Za-z ]{1,32}"),
        components in prop::option::of(prop::collection::vec(arb_name_component(), 0..3)),
        is_ordered in any::<Option<bool>>(),
        default_separator in prop::option::of(" "),
        extra in arb_extra(),
    ) -> Name {
        Name {
            components,
            full,
            is_ordered,
            sort_as: None,
            default_separator,
            extra,
        }
    }
}

prop_compose! {
    fn arb_contact_email()(
        address in "[a-z0-9]{1,8}@[a-z0-9]{1,8}\\.org",
        pref in any::<Option<u32>>(),
        extra in arb_extra(),
    ) -> ContactEmail {
        ContactEmail {
            address,
            contexts: None,
            pref,
            extra,
        }
    }
}

prop_compose! {
    fn arb_contact_phone()(
        number in "\\+1[0-9]{10}",
        pref in any::<Option<u32>>(),
        extra in arb_extra(),
    ) -> ContactPhone {
        ContactPhone {
            number,
            contexts: None,
            features: None,
            pref,
            extra,
        }
    }
}

prop_compose! {
    fn arb_organization()(
        name in "[A-Za-z0-9 ]{1,24}",
        extra in arb_extra(),
    ) -> Organization {
        Organization {
            name: Some(name),
            units: None,
            sort_as: None,
            contexts: None,
            extra,
        }
    }
}

prop_compose! {
    fn arb_title()(
        name in "[A-Za-z ]{1,24}",
        extra in arb_extra(),
    ) -> Title {
        Title {
            name,
            kind: None,
            organization_id: None,
            extra,
        }
    }
}

prop_compose! {
    fn arb_address()(
        full in prop::option::of("[0-9]{1,4} Main St, Anytown CA 90210"),
        country_code in prop::option::of("US"),
        default_separator in prop::option::of(", "),
        extra in arb_extra(),
    ) -> Address {
        Address {
            components: None,
            contexts: None,
            full,
            is_ordered: None,
            country_code,
            coordinates: None,
            time_zone: None,
            pref: None,
            default_separator,
            extra,
        }
    }
}

prop_compose! {
    fn arb_partial_date()(
        y in 2020..2030i32,
        m in 1..=12u32,
        d in 1..=28u32,
    ) -> PartialDate {
        PartialDate::new().with_year(y).with_month(m).with_day(d)
    }
}

prop_compose! {
    fn arb_anniversary()(
        kind in prop_oneof![Just("birth".to_string()), Just("wedding".to_string())],
        date in prop::option::of(arb_partial_date().prop_map(|pd| serde_json::to_value(pd).unwrap())),
        extra in arb_extra(),
    ) -> Anniversary {
        Anniversary { kind, date, extra }
    }
}

prop_compose! {
    fn arb_note()(
        note in "[A-Za-z0-9 .,]{1,32}",
        created in prop::option::of(arb_utc_date()),
        author in prop::option::of(
            "[A-Za-z ]{1,16}".prop_map(|name| serde_json::to_value(Author::new(name)).unwrap())
        ),
        extra in arb_extra(),
    ) -> Note {
        Note {
            note,
            created,
            author,
            extra,
        }
    }
}

prop_compose! {
    fn arb_card_meta()(
        id in prop::option::of(arb_id()),
        address_book_ids in prop::option::of(prop::collection::btree_map(arb_id(), any::<bool>(), 0..2)),
        uid in prop::option::of(arb_id().prop_map(|id| id.as_str().to_string())),
        created in prop::option::of(arb_utc_date()),
        extra in arb_extra(),
    ) -> (
        Option<Id>,
        Option<BTreeMap<Id, bool>>,
        Option<String>,
        Option<UtcDate>,
        BTreeMap<String, Value>,
    ) {
        (id, address_book_ids, uid, created, extra)
    }
}

prop_compose! {
    fn arb_card_names()(
        name in prop::option::of(arb_name()),
        organizations in prop::option::of(prop::collection::btree_map("[a-z0-9]{1,4}", arb_organization(), 0..2)),
        titles in prop::option::of(prop::collection::btree_map("[a-z0-9]{1,4}", arb_title(), 0..2)),
    ) -> (
        Option<Name>,
        Option<BTreeMap<String, Organization>>,
        Option<BTreeMap<String, Title>>,
    ) {
        (name, organizations, titles)
    }
}

prop_compose! {
    fn arb_card_comm()(
        emails in prop::option::of(prop::collection::btree_map("[a-z0-9]{1,4}", arb_contact_email(), 0..2)),
        phones in prop::option::of(prop::collection::btree_map("[a-z0-9]{1,4}", arb_contact_phone(), 0..2)),
        addresses in prop::option::of(prop::collection::btree_map("[a-z0-9]{1,4}", arb_address(), 0..2)),
    ) -> (
        Option<BTreeMap<String, ContactEmail>>,
        Option<BTreeMap<String, ContactPhone>>,
        Option<BTreeMap<String, Address>>,
    ) {
        (emails, phones, addresses)
    }
}

prop_compose! {
    fn arb_card_entries()(
        notes in prop::option::of(prop::collection::btree_map("[a-z0-9]{1,4}", arb_note(), 0..2)),
        anniversaries in prop::option::of(prop::collection::btree_map("[a-z0-9]{1,4}", arb_anniversary(), 0..2)),
    ) -> (
        Option<BTreeMap<String, Note>>,
        Option<BTreeMap<String, Anniversary>>,
    ) {
        (notes, anniversaries)
    }
}

prop_compose! {
    fn arb_contact_card()(
        (id, address_book_ids, uid, created, extra) in arb_card_meta(),
        (name, organizations, titles) in arb_card_names(),
        (emails, phones, addresses) in arb_card_comm(),
        (notes, anniversaries) in arb_card_entries(),
    ) -> ContactCard {
        ContactCard {
            id,
            address_book_ids,
            card_type: Some("Card".to_string()),
            kind: Some("individual".to_string()),
            version: Some("1.0".to_string()),
            language: None,
            members: None,
            prod_id: None,
            uid,
            created,
            updated: None,
            name,
            nicknames: None,
            emails,
            phones,
            organizations,
            titles,
            addresses,
            notes,
            anniversaries,
            links: None,
            calendars: None,
            scheduling_addresses: None,
            media: None,
            online_services: None,
            keywords: None,
            related_to: None,
            crypto_keys: None,
            directories: None,
            personal_info: None,
            speak_to_as: None,
            preferred_languages: None,
            localizations: None,
            extra,
        }
    }
}

prop_compose! {
    fn arb_calendar_rights()(
        may_read_free_busy in any::<Option<bool>>(),
        may_read_items in any::<Option<bool>>(),
        may_write_all in any::<Option<bool>>(),
        may_write_own in any::<Option<bool>>(),
        may_update_private in any::<Option<bool>>(),
        may_rsvp in any::<Option<bool>>(),
        may_share in any::<Option<bool>>(),
        may_delete in any::<Option<bool>>(),
        extra in arb_extra(),
    ) -> CalendarRights {
        CalendarRights {
            may_read_free_busy,
            may_read_items,
            may_write_all,
            may_write_own,
            may_update_private,
            may_rsvp,
            may_share,
            may_delete,
            extra,
        }
    }
}

prop_compose! {
    fn arb_calendar()(
        id in prop::option::of(arb_id()),
        name in "[A-Za-z0-9 _-]{1,24}",
        description in prop::option::of("[A-Za-z0-9 _-]{0,32}"),
        color in prop::option::of("#[0-9a-fA-F]{6}"),
        sort_order in any::<Option<u32>>(),
        is_default in any::<Option<bool>>(),
        is_subscribed in any::<Option<bool>>(),
        is_visible in any::<Option<bool>>(),
        include_in_availability in prop::option::of(Just("all".to_string())),
        time_zone in prop::option::of(Just("Europe/Berlin".to_string())),
        my_rights in prop::option::of(arb_calendar_rights()),
        share_with in prop::option::of(prop::collection::btree_map(arb_id(), arb_calendar_rights(), 0..2)),
        may_delete in any::<Option<bool>>(),
        extra in arb_extra(),
    ) -> Calendar {
        Calendar {
            id,
            name,
            description,
            color,
            sort_order,
            is_default,
            is_subscribed,
            is_visible,
            include_in_availability,
            time_zone,
            my_rights,
            share_with,
            may_delete,
            extra,
        }
    }
}

prop_compose! {
    fn arb_nday()(
        day in prop_oneof![
            Just("mo".to_string()),
            Just("tu".to_string()),
            Just("we".to_string()),
            Just("th".to_string()),
            Just("fr".to_string()),
            Just("sa".to_string()),
            Just("su".to_string()),
        ],
        nth_of_period in any::<Option<i32>>(),
        extra in arb_extra(),
    ) -> NDay {
        NDay {
            day_type: Some("NDay".to_string()),
            day,
            nth_of_period,
            extra,
        }
    }
}

prop_compose! {
    fn arb_recurrence_rule()(
        frequency in prop_oneof![
            Just("daily".to_string()),
            Just("weekly".to_string()),
            Just("monthly".to_string()),
            Just("yearly".to_string()),
        ],
        interval in any::<Option<u32>>(),
        count in any::<Option<u32>>(),
        until in prop::option::of("2026-12-31T23:59:59"),
        by_day in prop::option::of(prop::collection::vec(arb_nday(), 0..2)),
        first_day_of_week in prop::option::of(Just("mo".to_string())),
        extra in arb_extra(),
    ) -> RecurrenceRule {
        RecurrenceRule {
            rule_type: Some("RecurrenceRule".to_string()),
            frequency,
            interval,
            count,
            until,
            by_day,
            by_month: None,
            by_month_day: None,
            by_year_day: None,
            by_week_no: None,
            by_hour: None,
            by_minute: None,
            by_second: None,
            by_set_position: None,
            first_day_of_week,
            rscale: None,
            skip: None,
            extra,
        }
    }
}

prop_compose! {
    fn arb_event_meta()(
        id in prop::option::of(arb_id()),
        calendar_ids in prop::option::of(prop::collection::btree_map(arb_id(), any::<bool>(), 0..2)),
        uid in prop::option::of(arb_id().prop_map(|id| id.as_str().to_string())),
        main_location_id in prop::option::of(arb_id()),
        is_origin in any::<Option<bool>>(),
        extra in arb_extra(),
    ) -> (
        Option<Id>,
        Option<BTreeMap<Id, bool>>,
        Option<String>,
        Option<Id>,
        Option<bool>,
        BTreeMap<String, Value>,
    ) {
        (id, calendar_ids, uid, main_location_id, is_origin, extra)
    }
}

prop_compose! {
    fn arb_event_schedule()(
        start in prop::option::of("2026-06-15T10:00:00"),
        time_zone in prop::option::of(Just("Europe/Berlin".to_string())),
        duration in prop::option::of(Just("PT1H".to_string())),
        show_without_time in any::<Option<bool>>(),
        recurrence_rule in prop::option::of(arb_recurrence_rule()),
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<bool>,
        Option<RecurrenceRule>,
    ) {
        (start, time_zone, duration, show_without_time, recurrence_rule)
    }
}

prop_compose! {
    fn arb_event_details()(
        title in prop::option::of("[A-Za-z0-9 _-]{1,32}"),
        description in prop::option::of("[A-Za-z0-9 _-]{0,32}"),
        status in prop::option::of(prop_oneof![
            Just("confirmed".to_string()),
            Just("tentative".to_string()),
            Just("cancelled".to_string()),
        ]),
        free_busy_status in prop::option::of(Just("busy".to_string())),
        priority in any::<Option<i64>>(),
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
    ) {
        (title, description, status, free_busy_status, priority)
    }
}

prop_compose! {
    fn arb_calendar_event()(
        (id, calendar_ids, uid, main_location_id, is_origin, extra) in arb_event_meta(),
        (start, time_zone, duration, show_without_time, recurrence_rule) in arb_event_schedule(),
        (title, description, status, free_busy_status, priority) in arb_event_details(),
    ) -> CalendarEvent {
        CalendarEvent {
            id,
            calendar_ids,
            event_type: Some("Event".to_string()),
            version: Some("2.0".to_string()),
            uid,
            created: None,
            updated: None,
            title,
            description,
            start,
            time_zone,
            duration,
            show_without_time,
            status,
            free_busy_status,
            priority,
            privacy: None,
            locations: None,
            virtual_locations: None,
            links: None,
            keywords: None,
            alerts: None,
            participants: None,
            organizer_calendar_address: None,
            is_draft: None,
            hide_attendees: None,
            recurrence_rule,
            recurrence_overrides: None,
            time_zones: None,
            use_default_alerts: None,
            color: None,
            locale: None,
            localizations: None,
            is_origin,
            end_time_zone: None,
            main_location_id,
            schedule_sequence: None,
            schedule_updated: None,
            may_invite_self: None,
            may_invite_others: None,
            extra,
        }
    }
}

prop_compose! {
    fn arb_get_request()(
        account_id in arb_id(),
        ids in prop::option::of(prop::collection::vec(arb_id(), 0..3)),
        ids_ref in prop::option::of(arb_result_reference()),
        properties in prop::option::of(prop::collection::vec("[a-z]{1,10}", 0..3)),
    ) -> GetRequest {
        let mut request = GetRequest::all(account_id);
        request.ids = ids;
        request.ids_ref = ids_ref;
        request.properties = properties;
        request
    }
}

fn arb_get_response<T, S>(make_item: fn() -> S) -> impl Strategy<Value = GetResponse<T>>
where
    T: Clone + std::fmt::Debug + 'static,
    S: Strategy<Value = T> + 'static,
{
    (
        arb_id(),
        arb_state(),
        prop::collection::vec(make_item(), 0..3),
        prop::collection::vec(arb_id(), 0..2),
    )
        .prop_map(|(account_id, state, list, not_found)| {
            let mut response = GetResponse::new(account_id, state, list);
            response.not_found = not_found;
            response
        })
}

fn arb_set_request<T, S>(make_item: fn() -> S) -> impl Strategy<Value = SetRequest<T>>
where
    T: Clone + std::fmt::Debug + 'static,
    S: Strategy<Value = T> + 'static,
{
    (
        arb_id(),
        prop::option::of(arb_state()),
        prop::option::of(prop::collection::btree_map(
            "[a-z0-9]{1,8}",
            make_item(),
            0..2,
        )),
        prop::option::of(prop::collection::btree_map(
            arb_id(),
            arb_json_value(),
            0..2,
        )),
        prop::option::of(prop::collection::vec(arb_id(), 0..2)),
    )
        .prop_map(|(account_id, if_in_state, create, update, destroy)| {
            let mut request = SetRequest::new(account_id);
            request.if_in_state = if_in_state;
            request.create = create;
            request.update = update;
            request.destroy = destroy;
            request
        })
}

prop_compose! {
    fn arb_set_error()(
        error_type in prop_oneof![
            Just("notFound".to_string()),
            Just("invalidProperties".to_string()),
            Just("stateMismatch".to_string()),
        ],
        description in prop::option::of("[A-Za-z0-9 ]{1,24}"),
        properties in prop::option::of(prop::collection::vec("[a-z]{1,10}", 0..2)),
        extra in arb_extra(),
    ) -> SetError {
        let mut error = SetError::new(error_type).with_extra(extra);
        if let Some(description) = description {
            error = error.with_description(description);
        }
        if let Some(properties) = properties {
            error = error.with_properties(properties);
        }
        error
    }
}

fn arb_set_response<T, S>(make_item: fn() -> S) -> impl Strategy<Value = SetResponse<T>>
where
    T: Clone + std::fmt::Debug + 'static,
    S: Strategy<Value = T> + 'static,
{
    (
        arb_id(),
        prop::option::of(arb_state()),
        prop::option::of(arb_state()),
        prop::option::of(prop::collection::btree_map(
            "[a-z0-9]{1,8}",
            make_item(),
            0..2,
        )),
        prop::option::of(prop::collection::btree_map(
            arb_id(),
            prop::option::of(make_item()),
            0..2,
        )),
        prop::option::of(prop::collection::vec(arb_id(), 0..2)),
        prop::option::of(prop::collection::btree_map(
            "[a-z0-9]{1,8}",
            arb_set_error(),
            0..2,
        )),
        prop::option::of(prop::collection::btree_map(arb_id(), arb_set_error(), 0..2)),
        prop::option::of(prop::collection::btree_map(arb_id(), arb_set_error(), 0..2)),
    )
        .prop_map(
            |(
                account_id,
                old_state,
                new_state,
                created,
                updated,
                destroyed,
                not_created,
                not_updated,
                not_destroyed,
            )| {
                let mut response =
                    SetResponse::new(account_id, new_state.clone().unwrap_or_default());
                response.old_state = old_state;
                response.new_state = new_state;
                response.created = created;
                response.updated = updated;
                response.destroyed = destroyed;
                response.not_created = not_created;
                response.not_updated = not_updated;
                response.not_destroyed = not_destroyed;
                response
            },
        )
}

prop_compose! {
    fn arb_email_get_request()(
        account_id in prop::option::of(arb_id()),
        ids in prop::option::of(prop::collection::vec(arb_id(), 0..2)),
        ids_ref in prop::option::of(arb_result_reference()),
        properties in prop::option::of(prop::collection::vec("[a-z]{1,10}", 0..2)),
        body_properties in prop::option::of(prop::collection::vec("[a-z]{1,10}", 0..2)),
        fetch_text_body_values in any::<bool>(),
        fetch_html_body_values in any::<bool>(),
        fetch_all_body_values in any::<bool>(),
        max_body_value_bytes in any::<Option<u64>>(),
    ) -> EmailGetRequest {
        let mut request = EmailGetRequest::default();
        request.account_id = account_id;
        request.ids = ids;
        request.ids_ref = ids_ref;
        request.properties = properties;
        request.body_properties = body_properties;
        request.fetch_text_body_values = fetch_text_body_values;
        request.fetch_html_body_values = fetch_html_body_values;
        request.fetch_all_body_values = fetch_all_body_values;
        request.max_body_value_bytes = max_body_value_bytes;
        request
    }
}

fn arb_address_book_set_request() -> impl Strategy<Value = AddressBookSetRequest> {
    (
        arb_set_request(arb_address_book),
        any::<Option<bool>>(),
        prop::option::of(arb_id()),
    )
        .prop_map(
            |(set, on_destroy_remove_contents, on_success_set_is_default)| AddressBookSetRequest {
                set,
                on_destroy_remove_contents,
                on_success_set_is_default,
            },
        )
}

fn arb_calendar_set_request() -> impl Strategy<Value = CalendarSetRequest> {
    (
        arb_id(),
        prop::option::of(arb_state()),
        prop::option::of(prop::collection::btree_map(
            "[a-z0-9]{1,8}",
            arb_calendar(),
            0..2,
        )),
        prop::option::of(prop::collection::btree_map(
            arb_id(),
            arb_json_value(),
            0..2,
        )),
        prop::option::of(prop::collection::vec(arb_id(), 0..2)),
        any::<Option<bool>>(),
        prop::option::of(arb_id()),
    )
        .prop_map(
            |(
                account_id,
                if_in_state,
                create,
                update,
                destroy,
                on_destroy_remove_contents,
                on_success_set_is_default,
            )| {
                CalendarSetRequest {
                    account_id,
                    if_in_state,
                    create,
                    update,
                    destroy,
                    on_destroy_remove_contents,
                    on_success_set_is_default,
                }
            },
        )
}

fn arb_calendar_event_set_request() -> impl Strategy<Value = CalendarEventSetRequest> {
    (arb_set_request(arb_calendar_event), any::<Option<bool>>()).prop_map(
        |(set, send_scheduling_messages)| CalendarEventSetRequest {
            set,
            send_scheduling_messages,
        },
    )
}

fn assert_roundtrip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let serialized = serde_json::to_value(value).expect("serialization to Value must succeed");
    let deserialized: T =
        serde_json::from_value(serialized).expect("deserialization from Value must succeed");
    assert_eq!(&deserialized, value, "round-trip value mismatch");
}

fn assert_unknown_property_survives<T>(
    value: &T,
    key: &str,
    val: &Value,
    get_extra: impl Fn(&T) -> &BTreeMap<String, Value>,
) where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let mut json_obj = serde_json::to_value(value)
        .expect("to_value must succeed")
        .as_object()
        .cloned()
        .unwrap_or_default();
    json_obj.insert(key.to_string(), val.clone());
    let wrapped = Value::Object(json_obj);

    let parsed: T =
        serde_json::from_value(wrapped).expect("from_value with unknown property must succeed");
    assert_eq!(
        get_extra(&parsed).get(key),
        Some(val),
        "unknown property must be preserved in extra"
    );

    let re_serialized = serde_json::to_value(&parsed).expect("re-serialization must succeed");
    assert_eq!(
        re_serialized.get(key),
        Some(val),
        "unknown property must survive re-serialization"
    );

    let parsed2: T = serde_json::from_value(re_serialized).expect("second from_value must succeed");
    assert_eq!(&parsed, &parsed2, "second round-trip must equal first");
}

proptest! {
    // 1. Mailbox
    #[test]
    fn prop_roundtrip_mailbox(mailbox in arb_mailbox()) {
        assert_roundtrip(&mailbox);
    }

    #[test]
    fn prop_unknown_property_mailbox(
        mailbox in arb_mailbox(),
        key in arb_unknown_key(),
        val in arb_json_value(),
    ) {
        assert_unknown_property_survives(&mailbox, &key, &val, |m| &m.extra);
    }

    // 2. Email
    #[test]
    fn prop_roundtrip_email(email in arb_email()) {
        assert_roundtrip(&email);
    }

    #[test]
    fn prop_unknown_property_email(
        email in arb_email(),
        key in arb_unknown_key(),
        val in arb_json_value(),
    ) {
        assert_unknown_property_survives(&email, &key, &val, |e| &e.extra);
    }

    // 3. Identity
    #[test]
    fn prop_roundtrip_identity(identity in arb_identity()) {
        assert_roundtrip(&identity);
    }

    #[test]
    fn prop_unknown_property_identity(
        identity in arb_identity(),
        key in arb_unknown_key(),
        val in arb_json_value(),
    ) {
        assert_unknown_property_survives(&identity, &key, &val, |i| &i.extra);
    }

    // 4. AddressBook
    #[test]
    fn prop_roundtrip_address_book(address_book in arb_address_book()) {
        assert_roundtrip(&address_book);
    }

    #[test]
    fn prop_unknown_property_address_book(
        address_book in arb_address_book(),
        key in arb_unknown_key(),
        val in arb_json_value(),
    ) {
        assert_unknown_property_survives(&address_book, &key, &val, |ab| &ab.extra);
    }

    // 5. ContactCard
    #[test]
    fn prop_roundtrip_contact_card(contact_card in arb_contact_card()) {
        assert_roundtrip(&contact_card);
    }

    #[test]
    fn prop_unknown_property_contact_card(
        contact_card in arb_contact_card(),
        key in arb_unknown_key(),
        val in arb_json_value(),
    ) {
        assert_unknown_property_survives(&contact_card, &key, &val, |cc| &cc.extra);
    }

    // 6. Calendar
    #[test]
    fn prop_roundtrip_calendar(calendar in arb_calendar()) {
        assert_roundtrip(&calendar);
    }

    #[test]
    fn prop_unknown_property_calendar(
        calendar in arb_calendar(),
        key in arb_unknown_key(),
        val in arb_json_value(),
    ) {
        assert_unknown_property_survives(&calendar, &key, &val, |c| &c.extra);
    }

    // 7. CalendarEvent
    #[test]
    fn prop_roundtrip_calendar_event(event in arb_calendar_event()) {
        assert_roundtrip(&event);
    }

    #[test]
    fn prop_unknown_property_calendar_event(
        event in arb_calendar_event(),
        key in arb_unknown_key(),
        val in arb_json_value(),
    ) {
        assert_unknown_property_survives(&event, &key, &val, |ce| &ce.extra);
    }

    // Request and response shapes
    #[test]
    fn prop_roundtrip_get_request(req in arb_get_request()) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_email_get_request(req in arb_email_get_request()) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_address_book_set_request(req in arb_address_book_set_request()) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_calendar_set_request(req in arb_calendar_set_request()) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_calendar_event_set_request(req in arb_calendar_event_set_request()) {
        assert_roundtrip(&req);
    }

    // GetResponse shapes for all 7 record types
    #[test]
    fn prop_roundtrip_mailbox_get_response(resp in arb_get_response(arb_mailbox)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_email_get_response(resp in arb_get_response(arb_email)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_identity_get_response(resp in arb_get_response(arb_identity)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_address_book_get_response(resp in arb_get_response(arb_address_book)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_contact_card_get_response(resp in arb_get_response(arb_contact_card)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_calendar_get_response(resp in arb_get_response(arb_calendar)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_calendar_event_get_response(resp in arb_get_response(arb_calendar_event)) {
        assert_roundtrip(&resp);
    }

    // SetRequest shapes for all 7 record types
    #[test]
    fn prop_roundtrip_mailbox_set_request(req in arb_set_request(arb_mailbox)) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_email_set_request(req in arb_set_request(arb_email)) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_identity_set_request(req in arb_set_request(arb_identity)) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_address_book_set_request_generic(req in arb_set_request(arb_address_book)) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_contact_card_set_request(req in arb_set_request(arb_contact_card)) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_calendar_set_request_generic(req in arb_set_request(arb_calendar)) {
        assert_roundtrip(&req);
    }

    #[test]
    fn prop_roundtrip_calendar_event_set_request_generic(req in arb_set_request(arb_calendar_event)) {
        assert_roundtrip(&req);
    }

    // SetResponse shapes for all 7 record types
    #[test]
    fn prop_roundtrip_mailbox_set_response(resp in arb_set_response(arb_mailbox)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_email_set_response(resp in arb_set_response(arb_email)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_identity_set_response(resp in arb_set_response(arb_identity)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_address_book_set_response(resp in arb_set_response(arb_address_book)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_contact_card_set_response(resp in arb_set_response(arb_contact_card)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_calendar_set_response(resp in arb_set_response(arb_calendar)) {
        assert_roundtrip(&resp);
    }

    #[test]
    fn prop_roundtrip_calendar_event_set_response(resp in arb_set_response(arb_calendar_event)) {
        assert_roundtrip(&resp);
    }
}
