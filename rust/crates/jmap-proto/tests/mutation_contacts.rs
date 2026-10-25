// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::contacts::{
    Address, AddressBook, AddressBookRights, AddressBookSetRequest, AddressComponent, Anniversary,
    Author, Calendar, CardGroup, ContactCard, ContactCardParseRequest, ContactCardParseResponse,
    ContactCardQueryFilter, ContactEmail, ContactPhone, ContactsCapability, CryptoKey, Directory,
    LanguagePref, Link, Media, Name, NameComponent, Nickname, Note, OnlineService, OrgUnit,
    Organization, PartialDate, PersonalInfo, Pronouns, PronounsValue, Relation, SchedulingAddress,
    SpeakToAs, Title, address_book_set_error, address_component_kind, anniversary_kind,
    calendar_kind, card_kind, contact_card_set_error, crypto_key_kind, directory_kind,
    grammatical_gender, link_kind, media_kind, name_component_kind, personal_info_kind, title_kind,
};
use jmap_proto::methods::SetRequest;
use jmap_proto::state::UtcDate;
use serde_json::json;

#[test]
fn address_book_builders_properties_and_extra_preservation() {
    let mut extra = BTreeMap::new();
    extra.insert("customVendorTag".to_string(), json!("addressbook-tag"));
    extra.insert("syncInterval".to_string(), json!(60));

    let mut address_book = AddressBook::new("Work Contacts")
        .with_description("Team contact directory")
        .with_sort_order(20)
        .is_default(true)
        .is_subscribed(true)
        .with_extra(extra);
    address_book.id = Some(Id::from("ab-work-1"));

    assert_eq!(address_book.id, Some(Id::from("ab-work-1")));
    assert_eq!(address_book.name, "Work Contacts");
    assert_eq!(
        address_book.description.as_deref(),
        Some("Team contact directory")
    );
    assert_eq!(address_book.sort_order, Some(20));
    assert_eq!(address_book.is_default, Some(true));
    assert_eq!(address_book.is_subscribed, Some(true));
    assert_eq!(
        address_book.extra.get("customVendorTag"),
        Some(&json!("addressbook-tag"))
    );
    assert_eq!(address_book.extra.get("syncInterval"), Some(&json!(60)));

    let val = serde_json::to_value(&address_book).expect("serialize AddressBook");
    assert_eq!(val["id"], "ab-work-1");
    assert_eq!(val["name"], "Work Contacts");
    assert_eq!(val["description"], "Team contact directory");
    assert_eq!(val["sortOrder"], 20);
    assert_eq!(val["isDefault"], true);
    assert_eq!(val["isSubscribed"], true);
    assert_eq!(val["customVendorTag"], "addressbook-tag");

    let deserialized: AddressBook = serde_json::from_value(val).expect("deserialize AddressBook");
    assert_eq!(deserialized, address_book);
}

#[test]
fn address_book_rights_presets_permissions_logic_and_extra_preservation() {
    let all_rights = AddressBookRights::all();
    assert_eq!(all_rights.may_read, Some(true));
    assert_eq!(all_rights.may_write, Some(true));
    assert_eq!(all_rights.may_share, Some(true));
    assert_eq!(all_rights.may_delete, Some(true));
    assert!(all_rights.is_writable());

    let ro_rights = AddressBookRights::read_only();
    assert_eq!(ro_rights.may_read, Some(true));
    assert_eq!(ro_rights.may_write, None);
    assert_eq!(ro_rights.may_share, None);
    assert_eq!(ro_rights.may_delete, None);
    assert!(!ro_rights.is_writable());

    let write_only = AddressBookRights::default().with_may_write(true);
    assert!(write_only.is_writable());

    let write_false = AddressBookRights::default().with_may_write(false);
    assert!(!write_false.is_writable());

    let mut extra = BTreeMap::new();
    extra.insert("adminDelegated".to_string(), json!(true));
    let custom_rights = AddressBookRights::read_only().with_extra(extra);
    assert_eq!(custom_rights.may_read, Some(true));
    assert_eq!(
        custom_rights.extra.get("adminDelegated"),
        Some(&json!(true))
    );

    let val = serde_json::to_value(&custom_rights).expect("serialize AddressBookRights");
    assert_eq!(val["mayRead"], true);
    assert_eq!(val["adminDelegated"], true);

    let deserialized: AddressBookRights =
        serde_json::from_value(val).expect("deserialize AddressBookRights");
    assert_eq!(deserialized, custom_rights);
}

#[test]
fn address_book_set_request_builders_and_properties() {
    let set_req = SetRequest::<AddressBook>::new("acc-test-1");
    let set_address_book = AddressBookSetRequest::new(set_req)
        .with_on_destroy_remove_contents(true)
        .with_on_success_set_is_default(Id::from("ab-default-target"));

    assert_eq!(set_address_book.on_destroy_remove_contents, Some(true));
    assert_eq!(
        set_address_book.on_success_set_is_default,
        Some(Id::from("ab-default-target"))
    );

    let val = serde_json::to_value(&set_address_book).expect("serialize AddressBookSetRequest");
    assert_eq!(val["onDestroyRemoveContents"], true);
    assert_eq!(val["onSuccessSetIsDefault"], "ab-default-target");

    let deserialized: AddressBookSetRequest =
        serde_json::from_value(val).expect("deserialize AddressBookSetRequest");
    assert_eq!(deserialized, set_address_book);
}

#[test]
fn contact_card_simple_constructor_and_fields() {
    let simple_card = ContactCard::simple("ab-target-1", "Vera Oldenburg", "vera@example.com");

    let ab_ids = simple_card
        .address_book_ids
        .as_ref()
        .expect("addressBookIds");
    assert_eq!(ab_ids.get(&Id::from("ab-target-1")), Some(&true));
    assert_eq!(simple_card.card_type.as_deref(), Some("Card"));
    assert_eq!(simple_card.version.as_deref(), Some("1.0"));
    assert_eq!(
        simple_card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Vera Oldenburg")
    );
    assert_eq!(
        simple_card
            .emails
            .as_ref()
            .and_then(|e| e.get("e0"))
            .map(|em| em.address.as_str()),
        Some("vera@example.com")
    );

    let val = serde_json::to_value(&simple_card).expect("serialize simple ContactCard");
    assert_eq!(val["@type"], "Card");
    assert_eq!(val["version"], "1.0");
    assert_eq!(val["addressBookIds"]["ab-target-1"], true);
    assert_eq!(val["name"]["full"], "Vera Oldenburg");
    assert_eq!(val["emails"]["e0"]["address"], "vera@example.com");
}

#[test]
fn contact_card_builders_and_properties() {
    let mut sched_map = BTreeMap::new();
    sched_map.insert("pref".to_string(), json!(1));
    let sched_addr = SchedulingAddress::new("mailto:sched@example.com")
        .with_type("SchedulingAddress")
        .with_pref(1)
        .with_label("Scheduling endpoint");

    let card = ContactCard::new()
        .with_type("Card")
        .with_id(Id::from("card-test-1"))
        .with_address_book_id(Id::from("ab-test-1"))
        .with_uid("urn:uuid:550e8400-e29b-41d4-a716-446655440000")
        .with_kind("individual")
        .with_name(Name::new("Alice Smith"))
        .with_email("work", ContactEmail::new("alice@work.example.com"))
        .with_phone("mobile", ContactPhone::new("+15551234567"))
        .with_address("office", Address::new().with_full("100 Market St"))
        .with_organization("employer", Organization::new("Acme Corp"))
        .with_title("lead", Title::new("Principal Engineer"))
        .with_note("reminder", Note::new("Met at RustConf"))
        .with_created(UtcDate::from("2026-04-01T10:00:00Z"))
        .with_updated(UtcDate::from("2026-04-02T12:00:00Z"))
        .with_speak_to_as(SpeakToAs::new().with_grammatical_gender("feminine"))
        .with_scheduling_address("primary", sched_addr);

    assert_eq!(card.card_type.as_deref(), Some("Card"));
    assert_eq!(card.id, Some(Id::from("card-test-1")));
    assert_eq!(
        card.address_book_ids
            .as_ref()
            .and_then(|m| m.get(&Id::from("ab-test-1"))),
        Some(&true)
    );
    assert_eq!(
        card.uid.as_deref(),
        Some("urn:uuid:550e8400-e29b-41d4-a716-446655440000")
    );
    assert_eq!(card.kind.as_deref(), Some("individual"));
    assert_eq!(
        card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some("Alice Smith")
    );
    assert_eq!(
        card.emails
            .as_ref()
            .and_then(|e| e.get("work"))
            .map(|em| em.address.as_str()),
        Some("alice@work.example.com")
    );
    assert_eq!(
        card.phones
            .as_ref()
            .and_then(|p| p.get("mobile"))
            .map(|ph| ph.number.as_str()),
        Some("+15551234567")
    );
    assert_eq!(
        card.addresses
            .as_ref()
            .and_then(|a| a.get("office"))
            .and_then(|ad| ad.full.as_deref()),
        Some("100 Market St")
    );
    assert_eq!(
        card.organizations
            .as_ref()
            .and_then(|o| o.get("employer"))
            .and_then(|org| org.name.as_deref()),
        Some("Acme Corp")
    );
    assert_eq!(
        card.titles
            .as_ref()
            .and_then(|t| t.get("lead"))
            .map(|ti| ti.name.as_str()),
        Some("Principal Engineer")
    );
    assert_eq!(
        card.notes
            .as_ref()
            .and_then(|n| n.get("reminder"))
            .map(|no| no.note.as_str()),
        Some("Met at RustConf")
    );
    assert_eq!(card.created, Some(UtcDate::from("2026-04-01T10:00:00Z")));
    assert_eq!(card.updated, Some(UtcDate::from("2026-04-02T12:00:00Z")));
    assert_eq!(
        card.speak_to_as
            .as_ref()
            .and_then(|s| s.grammatical_gender.as_deref()),
        Some("feminine")
    );
    assert_eq!(
        card.scheduling_addresses
            .as_ref()
            .and_then(|sa| sa.get("primary"))
            .map(|s| s.uri.as_str()),
        Some("mailto:sched@example.com")
    );

    let val = serde_json::to_value(&card).expect("serialize ContactCard");
    assert_eq!(val["@type"], "Card");
    assert_eq!(val["id"], "card-test-1");
    assert_eq!(val["addressBookIds"]["ab-test-1"], true);
    assert_eq!(val["uid"], "urn:uuid:550e8400-e29b-41d4-a716-446655440000");

    let deserialized: ContactCard = serde_json::from_value(val).expect("deserialize ContactCard");
    assert_eq!(deserialized, card);
}

#[test]
fn contact_card_type_serialization_and_deserialization_fallbacks() {
    let card_json = json!({
        "@type": "Card",
        "uid": "test-card-type-1"
    });
    let card: ContactCard = serde_json::from_value(card_json).expect("deserialize Card");
    assert_eq!(card.card_type.as_deref(), Some("Card"));

    let contact_card_json = json!({
        "@type": "ContactCard",
        "uid": "test-card-type-2"
    });
    let card2: ContactCard =
        serde_json::from_value(contact_card_json).expect("deserialize ContactCard");
    assert_eq!(card2.card_type.as_deref(), Some("Card"));

    let custom_card_json = json!({
        "@type": "VendorSpecialCard",
        "uid": "test-card-type-3"
    });
    let card3: ContactCard =
        serde_json::from_value(custom_card_json).expect("deserialize VendorSpecialCard");
    assert_eq!(card3.card_type.as_deref(), Some("VendorSpecialCard"));

    let omitted_type_json = json!({
        "uid": "test-card-type-4"
    });
    let card4: ContactCard =
        serde_json::from_value(omitted_type_json).expect("deserialize omitted @type");
    assert_eq!(card4.card_type.as_deref(), Some("Card"));

    let null_type_json = json!({
        "@type": null,
        "uid": "test-card-type-5"
    });
    let card5: ContactCard =
        serde_json::from_value(null_type_json).expect("deserialize null @type");
    assert_eq!(card5.card_type.as_deref(), Some("Card"));

    let serialized = serde_json::to_value(&card3).expect("serialize ContactCard");
    assert_eq!(serialized["@type"], "Card");
}

#[test]
fn name_and_name_component_builders_and_phonetic() {
    let comp1 = NameComponent::new("given", "John").with_phonetic("dʒɒn");
    let comp2 = NameComponent::new("surname", "Doe");
    assert_eq!(comp1.kind, "given");
    assert_eq!(comp1.value, "John");
    assert_eq!(comp1.phonetic(), Some("dʒɒn"));
    assert_eq!(comp2.phonetic(), None);

    let mut sort_as = BTreeMap::new();
    sort_as.insert("ja".to_string(), "ドオ, ジョン".to_string());

    let name = Name::new("John Doe")
        .with_components([comp1.clone(), comp2.clone()])
        .is_ordered(true)
        .with_sort_as(sort_as.clone())
        .with_default_separator(" ");

    assert_eq!(name.full.as_deref(), Some("John Doe"));
    assert_eq!(name.is_ordered, Some(true));
    assert_eq!(name.default_separator.as_deref(), Some(" "));
    assert_eq!(name.sort_as, Some(sort_as));
    assert_eq!(name.components.as_ref().map(|c| c.len()), Some(2));

    let val = serde_json::to_value(&name).expect("serialize Name");
    assert_eq!(val["full"], "John Doe");
    assert_eq!(val["isOrdered"], true);
    assert_eq!(val["defaultSeparator"], " ");
    assert_eq!(val["components"][0]["phonetic"], "dʒɒn");

    let deserialized: Name = serde_json::from_value(val).expect("deserialize Name");
    assert_eq!(deserialized, name);
}

#[test]
fn nickname_and_contact_email_builders_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customNick".to_string(), json!("fav"));
    let mut nickname = Nickname::new("Johnny");
    nickname.contexts = Some(json!({"private": true}));
    nickname.pref = Some(1);
    nickname.extra = extra;
    let val_nick = serde_json::to_value(&nickname).expect("serialize Nickname");
    assert_eq!(val_nick["name"], "Johnny");
    assert_eq!(val_nick["pref"], 1);
    let deserialized_nick: Nickname =
        serde_json::from_value(val_nick).expect("deserialize Nickname");
    assert_eq!(deserialized_nick, nickname);

    let email = ContactEmail::new("john@example.com")
        .with_contexts(json!({"work": true}))
        .with_pref(1)
        .with_label("Work Address");

    assert_eq!(email.address, "john@example.com");
    assert_eq!(email.contexts, Some(json!({"work": true})));
    assert_eq!(email.pref, Some(1));
    assert_eq!(email.label(), Some("Work Address"));

    let email_no_label = ContactEmail::new("plain@example.com");
    assert_eq!(email_no_label.label(), None);

    let val_email = serde_json::to_value(&email).expect("serialize ContactEmail");
    assert_eq!(val_email["address"], "john@example.com");
    assert_eq!(val_email["pref"], 1);
    assert_eq!(val_email["label"], "Work Address");

    let deserialized_email: ContactEmail =
        serde_json::from_value(val_email).expect("deserialize ContactEmail");
    assert_eq!(deserialized_email, email);
}

#[test]
fn contact_phone_builders_and_properties() {
    let phone = ContactPhone::new("+15559876543")
        .with_contexts(json!({"work": true}))
        .with_features(json!({"voice": true, "cell": true}))
        .with_pref(2)
        .with_label("Emergency Mobile");

    assert_eq!(phone.number, "+15559876543");
    assert_eq!(phone.contexts, Some(json!({"work": true})));
    assert_eq!(phone.features, Some(json!({"voice": true, "cell": true})));
    assert_eq!(phone.pref, Some(2));
    assert_eq!(phone.label(), Some("Emergency Mobile"));

    let phone_no_label = ContactPhone::new("+10000000000");
    assert_eq!(phone_no_label.label(), None);

    let val = serde_json::to_value(&phone).expect("serialize ContactPhone");
    assert_eq!(val["number"], "+15559876543");
    assert_eq!(val["pref"], 2);
    assert_eq!(val["label"], "Emergency Mobile");

    let deserialized: ContactPhone = serde_json::from_value(val).expect("deserialize ContactPhone");
    assert_eq!(deserialized, phone);
}

#[test]
fn organization_and_org_unit_builders_and_sorting() {
    let unit = OrgUnit::new("Core Platforms").with_sort_as("Platforms");
    assert_eq!(unit.name, "Core Platforms");
    assert_eq!(unit.sort_as.as_deref(), Some("Platforms"));

    let org = Organization::new("Global Tech Inc")
        .with_units([unit.clone()])
        .with_sort_as("GlobalTech");

    assert_eq!(org.name.as_deref(), Some("Global Tech Inc"));
    assert_eq!(org.sort_as.as_deref(), Some("GlobalTech"));
    assert_eq!(org.units.as_ref().map(|u| u.len()), Some(1));
    assert_eq!(org.units.as_ref().unwrap()[0].name, "Core Platforms");

    let val = serde_json::to_value(&org).expect("serialize Organization");
    assert_eq!(val["name"], "Global Tech Inc");
    assert_eq!(val["sortAs"], "GlobalTech");
    assert_eq!(val["units"][0]["name"], "Core Platforms");
    assert_eq!(val["units"][0]["sortAs"], "Platforms");

    let deserialized: Organization = serde_json::from_value(val).expect("deserialize Organization");
    assert_eq!(deserialized, org);
}

#[test]
fn title_builders_and_roles() {
    let title = Title::new("Principal Architect")
        .with_kind("role")
        .with_organization_id("org-enterprise-42");

    assert_eq!(title.name, "Principal Architect");
    assert_eq!(title.kind.as_deref(), Some("role"));
    assert_eq!(title.organization_id.as_deref(), Some("org-enterprise-42"));

    let val = serde_json::to_value(&title).expect("serialize Title");
    assert_eq!(val["name"], "Principal Architect");
    assert_eq!(val["kind"], "role");
    assert_eq!(val["organizationId"], "org-enterprise-42");

    let deserialized: Title = serde_json::from_value(val).expect("deserialize Title");
    assert_eq!(deserialized, title);
}

#[test]
fn address_and_address_component_builders_and_phonetic() {
    let comp = AddressComponent::new("name", "Friedrichstraße").with_phonetic("ˈfʁiːdʁɪçˌʃtʁaːsə");
    assert_eq!(comp.kind, "name");
    assert_eq!(comp.value, "Friedrichstraße");
    assert_eq!(comp.phonetic(), Some("ˈfʁiːdʁɪçˌʃtʁaːsə"));

    let comp_plain = AddressComponent::new("postcode", "10117");
    assert_eq!(comp_plain.phonetic(), None);

    let address = Address::new()
        .with_components([comp.clone(), comp_plain.clone()])
        .with_full("Friedrichstraße 43\n10117 Berlin\nGermany")
        .is_ordered(true)
        .with_country_code("DE")
        .with_coordinates("geo:52.5162,13.3888")
        .with_time_zone("Europe/Berlin")
        .with_pref(1)
        .with_contexts(json!({"billing": true}))
        .with_default_separator("\n");

    assert_eq!(
        address.full.as_deref(),
        Some("Friedrichstraße 43\n10117 Berlin\nGermany")
    );
    assert_eq!(address.is_ordered, Some(true));
    assert_eq!(address.country_code.as_deref(), Some("DE"));
    assert_eq!(address.coordinates.as_deref(), Some("geo:52.5162,13.3888"));
    assert_eq!(address.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(address.pref, Some(1));
    assert_eq!(address.contexts, Some(json!({"billing": true})));
    assert_eq!(address.default_separator.as_deref(), Some("\n"));
    assert_eq!(address.components.as_ref().map(|c| c.len()), Some(2));

    let val = serde_json::to_value(&address).expect("serialize Address");
    assert_eq!(val["countryCode"], "DE");
    assert_eq!(val["isOrdered"], true);
    assert_eq!(val["components"][0]["phonetic"], "ˈfʁiːdʁɪçˌʃtʁaːsə");

    let deserialized: Address = serde_json::from_value(val).expect("deserialize Address");
    assert_eq!(deserialized, address);
}

#[test]
fn author_and_note_builders_and_serialization() {
    let author = Author::new("Code Reviewer").with_uri("https://review.example.com/bot");
    assert_eq!(author.author_type.as_deref(), Some("Author"));
    assert_eq!(author.name.as_deref(), Some("Code Reviewer"));
    assert_eq!(
        author.uri.as_deref(),
        Some("https://review.example.com/bot")
    );

    let val_author = serde_json::to_value(&author).expect("serialize Author");
    assert_eq!(val_author["@type"], "Author");
    assert_eq!(val_author["name"], "Code Reviewer");
    assert_eq!(val_author["uri"], "https://review.example.com/bot");

    let note_with_author = Note::new("Approved architecture design")
        .with_created(UtcDate::from("2026-05-15T08:30:00Z"))
        .with_author(author.clone());

    assert_eq!(note_with_author.note, "Approved architecture design");
    assert_eq!(
        note_with_author.created,
        Some(UtcDate::from("2026-05-15T08:30:00Z"))
    );
    assert_eq!(note_with_author.author_name(), Some("Code Reviewer"));
    assert_eq!(note_with_author.author(), Some(author));

    let note_with_name = Note::new("Pending verification").with_author_name("QA Engineer");
    assert_eq!(note_with_name.author_name(), Some("QA Engineer"));
    assert_eq!(
        note_with_name.author().and_then(|a| a.name),
        Some("QA Engineer".to_string())
    );

    let mut note_string_author = Note::new("Legacy note format");
    note_string_author.author = Some(json!("Legacy Auditor"));
    assert_eq!(note_string_author.author_name(), Some("Legacy Auditor"));
    assert_eq!(
        note_string_author.author().and_then(|a| a.name),
        Some("Legacy Auditor".to_string())
    );

    let val_note_str =
        serde_json::to_value(&note_string_author).expect("serialize note string author");
    assert_eq!(val_note_str["author"]["@type"], "Author");
    assert_eq!(val_note_str["author"]["name"], "Legacy Auditor");

    let note_none_author = Note::new("Anonymous note");
    assert_eq!(note_none_author.author_name(), None);
    assert_eq!(note_none_author.author(), None);
    let val_none = serde_json::to_value(&note_none_author).expect("serialize note none author");
    assert!(val_none.get("author").is_none());
}

#[test]
fn partial_date_and_anniversary_builders_and_iso_parsing() {
    let pd = PartialDate::new()
        .with_year(1995)
        .with_month(8)
        .with_day(24)
        .with_calendar_scale("gregorian");

    assert_eq!(pd.date_type, "PartialDate");
    assert_eq!(pd.year, Some(1995));
    assert_eq!(pd.month, Some(8));
    assert_eq!(pd.day, Some(24));
    assert_eq!(pd.calendar_scale.as_deref(), Some("gregorian"));

    let val_pd = serde_json::to_value(&pd).expect("serialize PartialDate");
    assert_eq!(val_pd["@type"], "PartialDate");
    assert_eq!(val_pd["year"], 1995);
    assert_eq!(val_pd["month"], 8);
    assert_eq!(val_pd["day"], 24);

    let deserialized_pd: PartialDate =
        serde_json::from_value(val_pd).expect("deserialize PartialDate");
    assert_eq!(deserialized_pd, pd);

    let ann = Anniversary::new(anniversary_kind::BIRTH)
        .with_partial_date(Some(1995), Some(8), Some(24))
        .with_label("Release date")
        .with_pref(1)
        .with_place(Address::new().with_full("Redmond, WA"));

    assert_eq!(ann.kind, "birth");
    assert_eq!(ann.label(), Some("Release date"));
    assert_eq!(ann.pref(), Some(1));
    assert_eq!(
        ann.partial_date(),
        Some(
            PartialDate::new()
                .with_year(1995)
                .with_month(8)
                .with_day(24)
        )
    );

    let ann_no_label = Anniversary::new(anniversary_kind::WEDDING);
    assert_eq!(ann_no_label.label(), None);
    assert_eq!(ann_no_label.pref(), None);
    assert_eq!(ann_no_label.partial_date(), None);

    let ann_ym = Anniversary::new("celebration").with_date(json!("2020-12"));
    assert_eq!(
        ann_ym.partial_date(),
        Some(PartialDate::new().with_year(2020).with_month(12))
    );

    let ann_y = Anniversary::new("milestone").with_date(json!("2010"));
    assert_eq!(
        ann_y.partial_date(),
        Some(PartialDate::new().with_year(2010))
    );

    let ann_invalid = Anniversary::new("other").with_date(json!("bad-format-string"));
    assert_eq!(ann_invalid.partial_date(), None);

    let ann_bad_parts = Anniversary::new("bad3").with_date(json!("1995-xx-24"));
    assert_eq!(ann_bad_parts.partial_date(), None);

    let ann_bad_month = Anniversary::new("bad2").with_date(json!("1995-xx"));
    assert_eq!(ann_bad_month.partial_date(), None);

    let ann_bad_year = Anniversary::new("bad1").with_date(json!("xxxx"));
    assert_eq!(ann_bad_year.partial_date(), None);

    let ann_short = Anniversary::new("short").with_date(json!("123"));
    assert_eq!(ann_short.partial_date(), None);

    let ann_obj = Anniversary::new("custom_obj").with_date(json!({
        "@type": "PartialDate",
        "year": 1999,
        "month": 12,
        "day": 31
    }));
    assert_eq!(
        ann_obj.partial_date(),
        Some(
            PartialDate::new()
                .with_year(1999)
                .with_month(12)
                .with_day(31)
        )
    );

    let ann_iso_str = Anniversary::new("born").with_date(json!("1980-05-12"));
    let val_ann_iso = serde_json::to_value(&ann_iso_str).expect("serialize anniversary iso string");
    assert_eq!(val_ann_iso["date"]["@type"], "PartialDate");
    assert_eq!(val_ann_iso["date"]["year"], 1980);
    assert_eq!(val_ann_iso["date"]["month"], 5);
    assert_eq!(val_ann_iso["date"]["day"], 12);
}

#[test]
fn link_and_calendar_builders_and_properties() {
    let link = Link::new("https://social.example.com/@user")
        .with_kind(link_kind::CONTACT)
        .with_pref(1)
        .with_label("Fediverse profile");

    assert_eq!(link.uri, "https://social.example.com/@user");
    assert_eq!(link.kind.as_deref(), Some("contact"));
    assert_eq!(link.pref, Some(1));
    assert_eq!(link.label.as_deref(), Some("Fediverse profile"));

    let val_link = serde_json::to_value(&link).expect("serialize Link");
    assert_eq!(val_link["uri"], "https://social.example.com/@user");
    assert_eq!(val_link["kind"], "contact");
    assert_eq!(val_link["pref"], 1);

    let deserialized_link: Link = serde_json::from_value(val_link).expect("deserialize Link");
    assert_eq!(deserialized_link, link);

    let calendar = Calendar::new("https://calendar.example.com/cal.ics")
        .with_kind(calendar_kind::FREE_BUSY)
        .with_pref(2)
        .with_label("Free/Busy feed");

    assert_eq!(calendar.uri, "https://calendar.example.com/cal.ics");
    assert_eq!(calendar.kind.as_deref(), Some("freeBusy"));
    assert_eq!(calendar.pref, Some(2));
    assert_eq!(calendar.label.as_deref(), Some("Free/Busy feed"));

    let val_cal = serde_json::to_value(&calendar).expect("serialize Calendar");
    assert_eq!(val_cal["uri"], "https://calendar.example.com/cal.ics");
    assert_eq!(val_cal["kind"], "freeBusy");
    assert_eq!(val_cal["pref"], 2);

    let deserialized_cal: Calendar = serde_json::from_value(val_cal).expect("deserialize Calendar");
    assert_eq!(deserialized_cal, calendar);
}

#[test]
fn scheduling_address_builders_and_type_deserialization() {
    let sched = SchedulingAddress::new("mailto:sched.user@example.com")
        .with_type("SchedulingAddress")
        .with_pref(1)
        .with_label("Primary IMIP endpoint");

    assert_eq!(
        sched.scheduling_address_type.as_deref(),
        Some("SchedulingAddress")
    );
    assert_eq!(sched.uri, "mailto:sched.user@example.com");
    assert_eq!(sched.pref, Some(1));
    assert_eq!(sched.label.as_deref(), Some("Primary IMIP endpoint"));

    let val = serde_json::to_value(&sched).expect("serialize SchedulingAddress");
    assert_eq!(val["@type"], "SchedulingAddress");
    assert_eq!(val["uri"], "mailto:sched.user@example.com");

    let deserialized: SchedulingAddress =
        serde_json::from_value(val).expect("deserialize SchedulingAddress");
    assert_eq!(deserialized, sched);

    let custom_type_json = json!({
        "@type": "CustomSchedulingFormat",
        "uri": "sip:user@example.com"
    });
    let custom_sched: SchedulingAddress =
        serde_json::from_value(custom_type_json).expect("deserialize CustomSchedulingFormat");
    assert_eq!(
        custom_sched.scheduling_address_type.as_deref(),
        Some("CustomSchedulingFormat")
    );

    let omitted_type_json = json!({
        "uri": "mailto:default@example.com"
    });
    let default_sched: SchedulingAddress =
        serde_json::from_value(omitted_type_json).expect("deserialize omitted @type");
    assert_eq!(
        default_sched.scheduling_address_type.as_deref(),
        Some("SchedulingAddress")
    );

    let null_sched_type_json = json!({
        "@type": null,
        "uri": "mailto:null-type@example.com"
    });
    let null_sched: SchedulingAddress =
        serde_json::from_value(null_sched_type_json).expect("deserialize null @type");
    assert_eq!(
        null_sched.scheduling_address_type.as_deref(),
        Some("SchedulingAddress")
    );
}

#[test]
fn media_and_online_service_builders_and_properties() {
    let media = Media::new("https://media.example.com/avatar.jpg")
        .with_kind(media_kind::PHOTO)
        .with_media_type("image/jpeg")
        .with_pref(1)
        .with_label("High-res profile photo");

    assert_eq!(media.uri, "https://media.example.com/avatar.jpg");
    assert_eq!(media.kind.as_deref(), Some("photo"));
    assert_eq!(media.media_type.as_deref(), Some("image/jpeg"));
    assert_eq!(media.pref, Some(1));
    assert_eq!(media.label.as_deref(), Some("High-res profile photo"));

    let val_media = serde_json::to_value(&media).expect("serialize Media");
    assert_eq!(val_media["uri"], "https://media.example.com/avatar.jpg");
    assert_eq!(val_media["kind"], "photo");

    let deserialized_media: Media = serde_json::from_value(val_media).expect("deserialize Media");
    assert_eq!(deserialized_media, media);

    let online = OnlineService::new()
        .with_service("Matrix")
        .with_user("@developer:matrix.org")
        .with_uri("matrix:u/developer:matrix.org")
        .with_pref(1)
        .with_label("Dev Matrix Chat");

    assert_eq!(online.service.as_deref(), Some("Matrix"));
    assert_eq!(online.user.as_deref(), Some("@developer:matrix.org"));
    assert_eq!(online.uri.as_deref(), Some("matrix:u/developer:matrix.org"));
    assert_eq!(online.pref, Some(1));
    assert_eq!(online.label.as_deref(), Some("Dev Matrix Chat"));

    let val_online = serde_json::to_value(&online).expect("serialize OnlineService");
    assert_eq!(val_online["service"], "Matrix");
    assert_eq!(val_online["user"], "@developer:matrix.org");

    let deserialized_online: OnlineService =
        serde_json::from_value(val_online).expect("deserialize OnlineService");
    assert_eq!(deserialized_online, online);
}

#[test]
fn relation_and_contact_card_query_filter_builders() {
    let mut rel_map = BTreeMap::new();
    rel_map.insert("colleague".to_string(), true);
    rel_map.insert("emergency".to_string(), false);

    let relation = Relation::new().with_relation(rel_map);
    assert_eq!(
        relation.relation.as_ref().and_then(|r| r.get("colleague")),
        Some(&json!(true))
    );
    assert_eq!(
        relation.relation.as_ref().and_then(|r| r.get("emergency")),
        Some(&json!(false))
    );

    let val_rel = serde_json::to_value(&relation).expect("serialize Relation");
    assert_eq!(val_rel["relation"]["colleague"], true);
    assert_eq!(val_rel["relation"]["emergency"], false);

    let deserialized_rel: Relation = serde_json::from_value(val_rel).expect("deserialize Relation");
    assert_eq!(deserialized_rel, relation);

    let filter = ContactCardQueryFilter::in_address_book("ab-search-1")
        .uid("uid-match-1")
        .text("Rust")
        .name("Tobias")
        .email("muelli@")
        .phone("+49")
        .online_service("Matrix")
        .address("Berlin")
        .kind("individual");

    assert_eq!(filter.in_address_book, Some(Id::from("ab-search-1")));
    assert_eq!(filter.uid.as_deref(), Some("uid-match-1"));
    assert_eq!(filter.text.as_deref(), Some("Rust"));
    assert_eq!(filter.name.as_deref(), Some("Tobias"));
    assert_eq!(filter.email.as_deref(), Some("muelli@"));
    assert_eq!(filter.phone.as_deref(), Some("+49"));
    assert_eq!(filter.online_service.as_deref(), Some("Matrix"));
    assert_eq!(filter.address.as_deref(), Some("Berlin"));
    assert_eq!(filter.kind.as_deref(), Some("individual"));

    let val_filter = serde_json::to_value(&filter).expect("serialize ContactCardQueryFilter");
    assert_eq!(val_filter["inAddressBook"], "ab-search-1");
    assert_eq!(val_filter["uid"], "uid-match-1");
    assert_eq!(val_filter["text"], "Rust");
    assert_eq!(val_filter["name"], "Tobias");
    assert_eq!(val_filter["email"], "muelli@");

    let deserialized_filter: ContactCardQueryFilter =
        serde_json::from_value(val_filter).expect("deserialize ContactCardQueryFilter");
    assert_eq!(deserialized_filter, filter);
}

#[test]
fn crypto_key_and_directory_builders_and_deserialization() {
    let key = CryptoKey::new(crypto_key_kind::KEY, "https://keys.example.com/alice.gpg")
        .with_media_type("application/pgp-keys")
        .with_pref(1);

    assert_eq!(key.kind.as_deref(), Some("key"));
    assert_eq!(key.uri, "https://keys.example.com/alice.gpg");
    assert_eq!(key.media_type.as_deref(), Some("application/pgp-keys"));
    assert_eq!(key.pref, Some(1));

    let val_key = serde_json::to_value(&key).expect("serialize CryptoKey");
    assert_eq!(val_key["kind"], "key");
    assert_eq!(val_key["uri"], "https://keys.example.com/alice.gpg");

    let deserialized_key: CryptoKey =
        serde_json::from_value(val_key).expect("deserialize CryptoKey");
    assert_eq!(deserialized_key, key);

    let dir = Directory::new(directory_kind::ENTRY, "ldap://ldap.corp.com/cn=Alice")
        .with_type("Directory")
        .with_media_type("text/directory")
        .with_list_as(15)
        .with_pref(1)
        .with_label("Active Directory Entry")
        .with_contexts(json!({"work": true}));

    assert_eq!(dir.directory_type.as_deref(), Some("Directory"));
    assert_eq!(dir.kind.as_deref(), Some("entry"));
    assert_eq!(dir.uri, "ldap://ldap.corp.com/cn=Alice");
    assert_eq!(dir.media_type.as_deref(), Some("text/directory"));
    assert_eq!(dir.list_as, Some(15));
    assert_eq!(dir.pref, Some(1));
    assert_eq!(dir.label.as_deref(), Some("Active Directory Entry"));
    assert_eq!(dir.contexts, Some(json!({"work": true})));

    let val_dir = serde_json::to_value(&dir).expect("serialize Directory");
    assert_eq!(val_dir["@type"], "Directory");
    assert_eq!(val_dir["listAs"], 15);

    let deserialized_dir: Directory =
        serde_json::from_value(val_dir).expect("deserialize Directory");
    assert_eq!(deserialized_dir, dir);

    let custom_dir_json = json!({
        "@type": "CustomDirectoryFormat",
        "uri": "ldap://example.com"
    });
    let custom_dir: Directory =
        serde_json::from_value(custom_dir_json).expect("deserialize CustomDirectoryFormat");
    assert_eq!(
        custom_dir.directory_type.as_deref(),
        Some("CustomDirectoryFormat")
    );

    let omitted_dir_json = json!({
        "uri": "ldap://default.example.com"
    });
    let default_dir: Directory =
        serde_json::from_value(omitted_dir_json).expect("deserialize omitted @type");
    assert_eq!(default_dir.directory_type.as_deref(), Some("Directory"));

    let null_dir_type_json = json!({
        "@type": null,
        "uri": "ldap://null-type.example.com"
    });
    let null_dir_type: Directory =
        serde_json::from_value(null_dir_type_json).expect("deserialize null @type");
    assert_eq!(null_dir_type.directory_type.as_deref(), Some("Directory"));

    let zero_list_as = json!({
        "uri": "ldap://example.com",
        "listAs": 0
    });
    let zero_dir: Directory =
        serde_json::from_value(zero_list_as).expect("deserialize zero listAs");
    assert_eq!(zero_dir.list_as, Some(0));

    let string_list_as = json!({
        "uri": "ldap://example.com",
        "listAs": "42"
    });
    let str_dir: Directory =
        serde_json::from_value(string_list_as).expect("deserialize string listAs");
    assert_eq!(str_dir.list_as, Some(42));

    let negative_list_as = json!({
        "uri": "ldap://example.com",
        "listAs": -1
    });
    assert!(serde_json::from_value::<Directory>(negative_list_as).is_err());

    let invalid_str_list_as = json!({
        "uri": "ldap://example.com",
        "listAs": "not-a-number"
    });
    assert!(serde_json::from_value::<Directory>(invalid_str_list_as).is_err());

    let null_list_as = json!({
        "uri": "ldap://example.com",
        "listAs": null
    });
    let null_dir: Directory =
        serde_json::from_value(null_list_as).expect("deserialize null listAs");
    assert_eq!(null_dir.list_as, None);
}

#[test]
fn personal_info_and_card_group_builders_and_deserialization() {
    let info = PersonalInfo::new(personal_info_kind::EXPERTISE, "Distributed Systems")
        .with_type("professional")
        .with_list_as(5);

    assert_eq!(info.kind, "expertise");
    assert_eq!(info.value.as_deref(), Some("Distributed Systems"));
    assert_eq!(info.personal_info_type.as_deref(), Some("professional"));
    assert_eq!(info.list_as, Some(5));

    let val_info = serde_json::to_value(&info).expect("serialize PersonalInfo");
    assert_eq!(val_info["kind"], "expertise");
    assert_eq!(val_info["type"], "professional");
    assert_eq!(val_info["listAs"], 5);

    let deserialized_info: PersonalInfo =
        serde_json::from_value(val_info).expect("deserialize PersonalInfo");
    assert_eq!(deserialized_info, info);

    let mut members = BTreeMap::new();
    members.insert("card-dev-1".to_string(), true);
    members.insert("card-dev-2".to_string(), true);

    let mut extra = BTreeMap::new();
    extra.insert("department".to_string(), json!("Core Infrastructure"));

    let card_group = CardGroup::new("Engineering Leads")
        .with_type("CardGroup")
        .with_members(members.clone())
        .with_extra(extra);

    assert_eq!(card_group.card_type.as_deref(), Some("CardGroup"));
    assert_eq!(card_group.name.as_deref(), Some("Engineering Leads"));
    assert_eq!(card_group.members, Some(members));
    assert_eq!(
        card_group.extra.get("department"),
        Some(&json!("Core Infrastructure"))
    );

    let val_group = serde_json::to_value(&card_group).expect("serialize CardGroup");
    assert_eq!(val_group["@type"], "CardGroup");
    assert_eq!(val_group["name"], "Engineering Leads");
    assert_eq!(val_group["department"], "Core Infrastructure");

    let deserialized_group: CardGroup =
        serde_json::from_value(val_group).expect("deserialize CardGroup");
    assert_eq!(deserialized_group, card_group);

    for alias in ["CardGroup", "Group", "Card"] {
        let group_alias_json = json!({
            "@type": alias,
            "name": "Alias Group"
        });
        let g: CardGroup =
            serde_json::from_value(group_alias_json).expect("deserialize alias CardGroup");
        assert_eq!(g.card_type.as_deref(), Some("CardGroup"));
    }

    let custom_group_json = json!({
        "@type": "SpecialGroup",
        "name": "Custom Team"
    });
    let custom_g: CardGroup =
        serde_json::from_value(custom_group_json).expect("deserialize SpecialGroup");
    assert_eq!(custom_g.card_type.as_deref(), Some("SpecialGroup"));

    let omitted_group_json = json!({
        "name": "Default Type Group"
    });
    let default_g: CardGroup =
        serde_json::from_value(omitted_group_json).expect("deserialize omitted @type group");
    assert_eq!(default_g.card_type.as_deref(), Some("CardGroup"));

    let null_group_json = json!({
        "@type": null,
        "name": "Null Type Group"
    });
    let null_g: CardGroup =
        serde_json::from_value(null_group_json).expect("deserialize null @type group");
    assert_eq!(null_g.card_type.as_deref(), Some("CardGroup"));
}

#[test]
fn contacts_capability_and_pronouns_builders_and_deref() {
    let mut extra = BTreeMap::new();
    extra.insert("maxVCardExportSize".to_string(), json!(5242880));

    let cap = ContactsCapability::new()
        .with_max_size_attachments_per_card(10_000_000)
        .with_max_number_of_cards_in_set(500)
        .with_extra(extra);

    assert_eq!(cap.max_size_attachments_per_card, 10_000_000);
    assert_eq!(cap.max_number_of_cards_in_set, 500);
    assert_eq!(cap.extra.get("maxVCardExportSize"), Some(&json!(5242880)));

    let val_cap = serde_json::to_value(&cap).expect("serialize ContactsCapability");
    assert_eq!(val_cap["maxSizeAttachmentsPerCard"], 10_000_000);
    assert_eq!(val_cap["maxNumberOfCardsInSet"], 500);
    assert_eq!(val_cap["maxVCardExportSize"], 5242880);

    let deserialized_cap: ContactsCapability =
        serde_json::from_value(val_cap).expect("deserialize ContactsCapability");
    assert_eq!(deserialized_cap, cap);

    let pronouns = Pronouns::new("they/them").with_pref(1);
    assert_eq!(pronouns.pronouns, "they/them");
    assert_eq!(pronouns.pref, Some(1));

    let pv_str: PronounsValue = PronounsValue::from("she/her");
    assert_eq!(&*pv_str, "she/her");

    let pv_owned: PronounsValue = PronounsValue::from(String::from("he/him"));
    assert_eq!(&*pv_owned, "he/him");

    let mut map_p = BTreeMap::new();
    map_p.insert("work".to_string(), Pronouns::new("they/them"));
    let pv_map: PronounsValue = PronounsValue::from(map_p);
    assert_eq!(&*pv_map, "they/them");

    let empty_map: BTreeMap<String, Pronouns> = BTreeMap::new();
    let pv_empty: PronounsValue = PronounsValue::from(empty_map);
    assert_eq!(&*pv_empty, "");
}

#[test]
fn speak_to_as_builders_and_pronouns_entry() {
    let speak = SpeakToAs::new()
        .with_grammatical_gender(grammatical_gender::FEMININE)
        .with_pronouns(PronounsValue::from("she/her"));

    assert_eq!(speak.grammatical_gender.as_deref(), Some("feminine"));
    assert_eq!(speak.pronouns.as_deref(), Some("she/her"));

    let with_entry = speak.with_pronouns_entry("informal", Pronouns::new("she/they"));
    match with_entry.pronouns.as_ref() {
        Some(PronounsValue::Map(m)) => {
            assert_eq!(
                m.get("default").map(|p| p.pronouns.as_str()),
                Some("she/her")
            );
            assert_eq!(
                m.get("informal").map(|p| p.pronouns.as_str()),
                Some("she/they")
            );
        }
        _ => panic!("expected PronounsValue::Map"),
    }

    let speak_empty = SpeakToAs::new().with_pronouns_entry("primary", Pronouns::new("he/him"));
    match speak_empty.pronouns.as_ref() {
        Some(PronounsValue::Map(m)) => {
            assert_eq!(
                m.get("primary").map(|p| p.pronouns.as_str()),
                Some("he/him")
            );
        }
        _ => panic!("expected PronounsValue::Map"),
    }
}

#[test]
fn language_pref_builders_and_custom_deserialization() {
    let pref = LanguagePref::new("de")
        .with_contexts(json!({"work": true}))
        .with_pref(1);

    assert_eq!(pref.language, "de");
    assert_eq!(pref.contexts, Some(json!({"work": true})));
    assert_eq!(pref.pref, Some(1));

    let val = serde_json::to_value(&pref).expect("serialize LanguagePref");
    assert_eq!(val["language"], "de");
    assert_eq!(val["pref"], 1);

    let deserialized_str: LanguagePref =
        serde_json::from_value(json!("en-US")).expect("deserialize string LanguagePref");
    assert_eq!(deserialized_str.language, "en-US");
    assert_eq!(deserialized_str.pref, None);

    let deserialized_u64: LanguagePref =
        serde_json::from_value(json!(3)).expect("deserialize u64 LanguagePref");
    assert_eq!(deserialized_u64.language, "");
    assert_eq!(deserialized_u64.pref, Some(3));

    assert!(serde_json::from_value::<LanguagePref>(json!(-4)).is_err());

    let deserialized_map: LanguagePref = serde_json::from_value(json!({
        "language": "fr",
        "pref": 2,
        "contexts": {"home": true}
    }))
    .expect("deserialize map LanguagePref");
    assert_eq!(deserialized_map.language, "fr");
    assert_eq!(deserialized_map.pref, Some(2));
    assert_eq!(deserialized_map.contexts, Some(json!({"home": true})));
}

#[test]
fn contact_card_parse_request_and_response_builders() {
    let req = ContactCardParseRequest::new(
        Id::from("acc-contacts-1"),
        [Id::from("blob-card-1"), Id::from("blob-card-2")],
    )
    .properties(["name", "emails", "addresses"]);

    assert_eq!(req.account_id, Id::from("acc-contacts-1"));
    assert_eq!(
        req.blob_ids,
        vec![Id::from("blob-card-1"), Id::from("blob-card-2")]
    );
    assert_eq!(
        req.properties,
        Some(vec![
            "name".to_string(),
            "emails".to_string(),
            "addresses".to_string()
        ])
    );

    let val_req = serde_json::to_value(&req).expect("serialize ContactCardParseRequest");
    assert_eq!(val_req["accountId"], "acc-contacts-1");
    assert_eq!(val_req["blobIds"][0], "blob-card-1");
    assert_eq!(val_req["properties"][0], "name");

    let deserialized_req: ContactCardParseRequest =
        serde_json::from_value(val_req).expect("deserialize ContactCardParseRequest");
    assert_eq!(deserialized_req, req);

    let mut parsed_cards = BTreeMap::new();
    parsed_cards.insert(
        Id::from("blob-card-1"),
        ContactCard::new().with_uid("card-parsed-1"),
    );

    let resp = ContactCardParseResponse::new(Id::from("acc-contacts-1"))
        .with_parsed(parsed_cards)
        .with_not_parsable([Id::from("blob-corrupt")])
        .with_not_found([Id::from("blob-missing")]);

    assert_eq!(resp.account_id, Id::from("acc-contacts-1"));
    assert!(
        resp.parsed
            .as_ref()
            .unwrap()
            .contains_key(&Id::from("blob-card-1"))
    );
    assert_eq!(resp.not_parsable, Some(vec![Id::from("blob-corrupt")]));
    assert_eq!(resp.not_found, Some(vec![Id::from("blob-missing")]));

    let val_resp = serde_json::to_value(&resp).expect("serialize ContactCardParseResponse");
    assert_eq!(val_resp["accountId"], "acc-contacts-1");
    assert_eq!(val_resp["notParsable"][0], "blob-corrupt");
    assert_eq!(val_resp["notFound"][0], "blob-missing");

    let deserialized_resp: ContactCardParseResponse =
        serde_json::from_value(val_resp).expect("deserialize ContactCardParseResponse");
    assert_eq!(deserialized_resp, resp);
}

#[test]
fn constants_and_error_codes_verification() {
    assert_eq!(address_book_set_error::HAS_CARD, "addressBookHasCard");
    assert_eq!(
        address_book_set_error::ADDRESS_BOOK_HAS_CONTENTS,
        "addressBookHasContents"
    );

    assert_eq!(anniversary_kind::BIRTH, "birth");
    assert_eq!(anniversary_kind::DEATH, "death");
    assert_eq!(anniversary_kind::WEDDING, "wedding");

    assert_eq!(title_kind::TITLE, "title");
    assert_eq!(title_kind::ROLE, "role");

    assert_eq!(calendar_kind::CALENDAR, "calendar");
    assert_eq!(calendar_kind::FREE_BUSY, "freeBusy");

    assert_eq!(media_kind::PHOTO, "photo");
    assert_eq!(media_kind::SOUND, "sound");
    assert_eq!(media_kind::LOGO, "logo");

    assert_eq!(link_kind::CONTACT, "contact");

    assert_eq!(name_component_kind::PREFIX, "prefix");
    assert_eq!(name_component_kind::TITLE, "title");
    assert_eq!(name_component_kind::GIVEN, "given");
    assert_eq!(name_component_kind::GIVEN2, "given2");
    assert_eq!(name_component_kind::MIDDLE, "middle");
    assert_eq!(name_component_kind::SURNAME, "surname");
    assert_eq!(name_component_kind::SURNAME2, "surname2");
    assert_eq!(name_component_kind::SUFFIX, "suffix");
    assert_eq!(name_component_kind::GENERATION, "generation");
    assert_eq!(name_component_kind::CREDENTIAL, "credential");
    assert_eq!(name_component_kind::SEPARATOR, "separator");

    assert_eq!(address_component_kind::NAME, "name");
    assert_eq!(address_component_kind::UNIT, "unit");
    assert_eq!(address_component_kind::FLOOR, "floor");
    assert_eq!(address_component_kind::STREET, "street");
    assert_eq!(address_component_kind::APPARTMENT, "appartment");
    assert_eq!(address_component_kind::APARTMENT, "apartment");
    assert_eq!(address_component_kind::ROOM, "room");
    assert_eq!(address_component_kind::BUILDING, "building");
    assert_eq!(address_component_kind::LOCALITY, "locality");
    assert_eq!(address_component_kind::DISTRICT, "district");
    assert_eq!(address_component_kind::SUBDISTRICT, "subdistrict");
    assert_eq!(address_component_kind::REGION, "region");
    assert_eq!(address_component_kind::POSTCODE, "postcode");
    assert_eq!(address_component_kind::POST_OFFICE_BOX, "postOfficeBox");
    assert_eq!(address_component_kind::COUNTRY, "country");
    assert_eq!(address_component_kind::COUNTRY_CODE, "countryCode");

    assert_eq!(crypto_key_kind::KEY, "key");
    assert_eq!(crypto_key_kind::CERT, "cert");

    assert_eq!(directory_kind::DIRECTORY, "directory");
    assert_eq!(directory_kind::ENTRY, "entry");

    assert_eq!(personal_info_kind::GENDER, "gender");
    assert_eq!(personal_info_kind::EXPERTISE, "expertise");
    assert_eq!(personal_info_kind::HOBBY, "hobby");
    assert_eq!(personal_info_kind::INTEREST, "interest");

    assert_eq!(card_kind::INDIVIDUAL, "individual");
    assert_eq!(card_kind::GROUP, "group");
    assert_eq!(card_kind::ORG, "org");
    assert_eq!(card_kind::LOCATION, "location");
    assert_eq!(card_kind::DEVICE, "device");
    assert_eq!(card_kind::APPLICATION, "application");

    assert_eq!(grammatical_gender::ANIMATE, "animate");
    assert_eq!(grammatical_gender::INANIMATE, "inanimate");
    assert_eq!(grammatical_gender::FEMININE, "feminine");
    assert_eq!(grammatical_gender::MASCULINE, "masculine");
    assert_eq!(grammatical_gender::NEUTER, "neuter");
    assert_eq!(grammatical_gender::COMMON, "common");

    assert_eq!(contact_card_set_error::BLOB_NOT_FOUND, "blobNotFound");
}
