// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 9610 contact types. A `ContactCard` is a JSContact Card (RFC 9553)
//! carrying the JMAP-side `id` and `addressBookIds` properties.
//!
//! Only the card properties Evolution needs are modeled; everything else
//! rides in `extra` and survives round-trips.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;
use crate::state::UtcDate;

/// An address book (RFC 9610 §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddressBook {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    /// Server-computed permissions on this address book (RFC 9610 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<AddressBookRights>,
    /// Rights granted to other principals (RFC 9610 §2), keyed by principal id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_with: Option<BTreeMap<Id, AddressBookRights>>,
    #[serde(default, skip_serializing)]
    pub may_delete: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AddressBook {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_sort_order(mut self, sort_order: u32) -> Self {
        self.sort_order = Some(sort_order);
        self
    }

    pub fn is_default(mut self, is_default: bool) -> Self {
        self.is_default = Some(is_default);
        self
    }

    pub fn is_subscribed(mut self, is_subscribed: bool) -> Self {
        self.is_subscribed = Some(is_subscribed);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `AddressBook/set` request arguments (RFC 9610 §2.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressBookSetRequest {
    #[serde(flatten)]
    pub set: crate::methods::SetRequest<AddressBook>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_destroy_remove_contents: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_set_is_default: Option<Id>,
}

impl AddressBookSetRequest {
    pub fn new(set: crate::methods::SetRequest<AddressBook>) -> Self {
        Self {
            set,
            on_destroy_remove_contents: None,
            on_success_set_is_default: None,
        }
    }

    pub fn with_on_destroy_remove_contents(mut self, on_destroy: bool) -> Self {
        self.on_destroy_remove_contents = Some(on_destroy);
        self
    }

    pub fn with_on_success_set_is_default(mut self, default_id: impl Into<Id>) -> Self {
        self.on_success_set_is_default = Some(default_id.into());
        self
    }
}

/// `AddressBook.myRights`/a `shareWith` entry, RFC 9610 §2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddressBookRights {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_share: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AddressBookRights {
    pub fn all() -> Self {
        Self {
            may_read: Some(true),
            may_write: Some(true),
            may_share: Some(true),
            may_delete: Some(true),
            extra: BTreeMap::new(),
        }
    }

    pub fn read_only() -> Self {
        Self {
            may_read: Some(true),
            ..Self::default()
        }
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// Whether these rights let the holder write to the address book.
    /// Absent `mayWrite` defaults to `false` (fail closed).
    pub fn is_writable(&self) -> bool {
        self.may_write.unwrap_or(false)
    }
}

/// A contact card (RFC 9610 §3): JSContact Card plus JMAP `id` and
/// `addressBookIds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactCard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_book_ids: Option<BTreeMap<Id, bool>>,
    #[serde(
        rename = "@type",
        default = "default_contact_card_type",
        serialize_with = "serialize_contact_card_type",
        deserialize_with = "deserialize_contact_card_type"
    )]
    pub card_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prod_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Name>,

    /// Contact nicknames (RFC 9553 §2.2.2), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nicknames: Option<BTreeMap<String, Nickname>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emails: Option<BTreeMap<String, ContactEmail>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phones: Option<BTreeMap<String, ContactPhone>>,
    /// Organizations the contact belongs to (RFC 9553 §2.2.3), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizations: Option<BTreeMap<String, Organization>>,
    /// Job titles and roles (RFC 9553 §2.2.4), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titles: Option<BTreeMap<String, Title>>,
    /// Postal addresses (RFC 9553 §2.5.1), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<BTreeMap<String, Address>>,
    /// Free-text notes (RFC 9553 §2.8.3), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<BTreeMap<String, Note>>,
    /// Memorable dates such as birthdays (RFC 9553 §2.8.1), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anniversaries: Option<BTreeMap<String, Anniversary>>,
    /// External web resources (RFC 9553 §2.6.3), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, Link>>,
    /// Calendaring resources (RFC 9553 §2.4.1), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendars: Option<BTreeMap<String, Calendar>>,
    /// The scheduling addresses of the contact (RFC 9553 §2.4.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduling_addresses: Option<BTreeMap<String, SchedulingAddress>>,
    /// Media resources such as photos (RFC 9553 §2.6.4), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<BTreeMap<String, Media>>,
    /// Online services and protocols (RFC 9553 §2.3.2), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_services: Option<BTreeMap<String, OnlineService>>,
    /// Contact tags (RFC 9553 §2.8.2, RFC 9553 §1.4.3 Set).
    /// Values stay as JSON to preserve unrecognised server values safely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, Value>>,
    /// Related entities (RFC 9553 §2.1.8), keyed by related entity identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_to: Option<BTreeMap<String, Relation>>,
    /// The cryptographic keys for the contact (RFC 9553 §2.6.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crypto_keys: Option<BTreeMap<String, CryptoKey>>,
    /// The directory services that may be searched for more info on the contact (RFC 9553 §2.6.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directories: Option<BTreeMap<String, Directory>>,
    /// Personal information such as gender, expertise, hobbies (RFC 9553 §2.8.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_info: Option<BTreeMap<String, PersonalInfo>>,
    /// How to address the contact (RFC 9553 §2.2.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speak_to_as: Option<SpeakToAs>,
    /// Preferred languages for communication (RFC 9553 §2.8.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_languages: Option<BTreeMap<String, LanguagePref>>,
    /// Localized property values (RFC 9553 §2.7.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub localizations: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for ContactCard {
    fn default() -> Self {
        Self {
            id: None,
            address_book_ids: None,
            card_type: Some("Card".to_owned()),
            kind: None,
            version: None,
            language: None,
            members: None,
            prod_id: None,
            uid: None,
            created: None,
            updated: None,
            name: None,
            nicknames: None,
            emails: None,
            phones: None,
            organizations: None,
            titles: None,
            addresses: None,
            notes: None,
            anniversaries: None,
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
            extra: BTreeMap::new(),
        }
    }
}

impl ContactCard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_type(mut self, card_type: impl Into<String>) -> Self {
        self.card_type = Some(card_type.into());
        self
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_address_book_id(mut self, id: impl Into<Id>) -> Self {
        let mut map = self.address_book_ids.unwrap_or_default();
        map.insert(id.into(), true);
        self.address_book_ids = Some(map);
        self
    }

    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    pub fn with_name(mut self, name: Name) -> Self {
        self.name = Some(name);
        self
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    pub fn with_email(mut self, key: impl Into<String>, email: ContactEmail) -> Self {
        let mut map = self.emails.unwrap_or_default();
        map.insert(key.into(), email);
        self.emails = Some(map);
        self
    }

    pub fn with_phone(mut self, key: impl Into<String>, phone: ContactPhone) -> Self {
        let mut map = self.phones.unwrap_or_default();
        map.insert(key.into(), phone);
        self.phones = Some(map);
        self
    }

    pub fn with_address(mut self, key: impl Into<String>, address: Address) -> Self {
        let mut map = self.addresses.unwrap_or_default();
        map.insert(key.into(), address);
        self.addresses = Some(map);
        self
    }

    pub fn with_organization(mut self, key: impl Into<String>, org: Organization) -> Self {
        let mut map = self.organizations.unwrap_or_default();
        map.insert(key.into(), org);
        self.organizations = Some(map);
        self
    }

    pub fn with_title(mut self, key: impl Into<String>, title: Title) -> Self {
        let mut map = self.titles.unwrap_or_default();
        map.insert(key.into(), title);
        self.titles = Some(map);
        self
    }

    pub fn with_note(mut self, key: impl Into<String>, note: Note) -> Self {
        let mut map = self.notes.unwrap_or_default();
        map.insert(key.into(), note);
        self.notes = Some(map);
        self
    }

    pub fn with_created(mut self, created: impl Into<UtcDate>) -> Self {
        self.created = Some(created.into());
        self
    }

    pub fn with_updated(mut self, updated: impl Into<UtcDate>) -> Self {
        self.updated = Some(updated.into());
        self
    }

    pub fn with_speak_to_as(mut self, speak_to_as: SpeakToAs) -> Self {
        self.speak_to_as = Some(speak_to_as);
        self
    }

    pub fn with_scheduling_address(
        mut self,
        key: impl Into<String>,
        address: SchedulingAddress,
    ) -> Self {
        let mut map = self.scheduling_addresses.unwrap_or_default();
        map.insert(key.into(), address);
        self.scheduling_addresses = Some(map);
        self
    }

    /// A minimal card with a full name and one email address, ready for
    /// `ContactCard/set` create.
    pub fn simple(address_book_id: impl Into<Id>, full_name: &str, email: &str) -> Self {
        Self {
            address_book_ids: Some([(address_book_id.into(), true)].into()),
            card_type: Some("Card".to_owned()),
            version: Some("1.0".to_owned()),
            name: Some(Name {
                full: Some(full_name.to_owned()),
                ..Name::default()
            }),
            emails: Some(
                [(
                    "e0".to_owned(),
                    ContactEmail {
                        address: email.to_owned(),
                        ..ContactEmail::default()
                    },
                )]
                .into(),
            ),
            ..Self::default()
        }
    }
}

fn default_contact_card_type() -> Option<String> {
    Some("Card".to_owned())
}

fn serialize_contact_card_type<S>(_value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str("Card")
}

fn deserialize_contact_card_type<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt.as_deref() {
        Some("Card") | Some("ContactCard") => Ok(Some("Card".to_owned())),
        Some(other) => Ok(Some(other.to_owned())),
        None => Ok(Some("Card".to_owned())),
    }
}

/// JSContact Name (RFC 9553 §2.2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Name {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<NameComponent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_ordered: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_separator: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Name {
    pub fn new(full: impl Into<String>) -> Self {
        Self {
            full: Some(full.into()),
            ..Self::default()
        }
    }

    pub fn with_components(mut self, components: impl IntoIterator<Item = NameComponent>) -> Self {
        self.components = Some(components.into_iter().collect());
        self
    }

    pub fn is_ordered(mut self, is_ordered: bool) -> Self {
        self.is_ordered = Some(is_ordered);
        self
    }

    pub fn with_sort_as(mut self, sort_as: BTreeMap<String, String>) -> Self {
        self.sort_as = Some(sort_as);
        self
    }

    pub fn with_default_separator(mut self, separator: impl Into<String>) -> Self {
        self.default_separator = Some(separator.into());
        self
    }
}

/// One name component: kind is `given`, `surname`, `title`, etc.
/// Unmodeled properties (such as phonetic spellings) are preserved in `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameComponent {
    pub kind: String,
    pub value: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl NameComponent {
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: value.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn phonetic(&self) -> Option<&str> {
        self.extra.get("phonetic").and_then(|v| v.as_str())
    }

    pub fn with_phonetic(mut self, phonetic: impl Into<String>) -> Self {
        self.extra
            .insert("phonetic".to_string(), Value::String(phonetic.into()));
        self
    }
}

/// JSContact Nickname (RFC 9553 §2.2.2): one name the contact is also known
/// by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Nickname {
    /// The nickname itself. Mandatory per RFC 9553 §2.2.2, and the only part
    /// of it a `NICKNAME` line has room for.
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact EmailAddress (RFC 9553 §2.3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContactEmail {
    #[serde(default)]
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ContactEmail {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            ..Self::default()
        }
    }

    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn label(&self) -> Option<&str> {
        self.extra.get("label").and_then(|v| v.as_str())
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.extra
            .insert("label".to_owned(), Value::String(label.into()));
        self
    }
}

/// JSContact Phone (RFC 9553 §2.3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContactPhone {
    #[serde(default)]
    pub number: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ContactPhone {
    pub fn new(number: impl Into<String>) -> Self {
        Self {
            number: number.into(),
            ..Self::default()
        }
    }

    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    pub fn with_features(mut self, features: Value) -> Self {
        self.features = Some(features);
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn label(&self) -> Option<&str> {
        self.extra.get("label").and_then(|v| v.as_str())
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.extra
            .insert("label".to_owned(), Value::String(label.into()));
        self
    }
}

/// JSContact Organization (RFC 9553 §2.2.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    /// The organisation's own name, which the `ORG` value states first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The units within it, outermost first: the departments the `ORG` value
    /// lists after the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<Vec<OrgUnit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Organization {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }

    pub fn with_units(mut self, units: impl IntoIterator<Item = OrgUnit>) -> Self {
        self.units = Some(units.into_iter().collect());
        self
    }

    pub fn with_sort_as(mut self, sort_as: impl Into<String>) -> Self {
        self.sort_as = Some(sort_as.into());
        self
    }
}

/// JSContact OrgUnit (RFC 9553 §2.2.3): one unit of an [`Organization`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OrgUnit {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl OrgUnit {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            sort_as: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_sort_as(mut self, sort_as: impl Into<String>) -> Self {
        self.sort_as = Some(sort_as.into());
        self
    }
}

/// JSContact Title (RFC 9553 §2.2.4): a job title the contact holds, or a
/// role it plays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Title {
    #[serde(default)]
    pub name: String,
    /// `title` or `role`. Absent means `title` (RFC 9553 §2.2.4 default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Title {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    pub fn with_organization_id(mut self, organization_id: impl Into<String>) -> Self {
        self.organization_id = Some(organization_id.into());
        self
    }
}

/// JSContact Address (RFC 9553 §2.5.1): one postal address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    /// The parts the address is built from, each naming what it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<AddressComponent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The address written out as it should be printed, line breaks and all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_ordered: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_separator: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Address {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_components(
        mut self,
        components: impl IntoIterator<Item = AddressComponent>,
    ) -> Self {
        self.components = Some(components.into_iter().collect());
        self
    }

    pub fn with_full(mut self, full: impl Into<String>) -> Self {
        self.full = Some(full.into());
        self
    }

    pub fn is_ordered(mut self, is_ordered: bool) -> Self {
        self.is_ordered = Some(is_ordered);
        self
    }

    pub fn with_country_code(mut self, country_code: impl Into<String>) -> Self {
        self.country_code = Some(country_code.into());
        self
    }

    pub fn with_coordinates(mut self, coordinates: impl Into<String>) -> Self {
        self.coordinates = Some(coordinates.into());
        self
    }

    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    pub fn with_default_separator(mut self, separator: impl Into<String>) -> Self {
        self.default_separator = Some(separator.into());
        self
    }
}

/// One part of an [`Address`]: `kind` is `name` (the street), `locality`,
/// `postcode`, `floor`, …
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddressComponent {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub value: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AddressComponent {
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: value.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn phonetic(&self) -> Option<&str> {
        self.extra.get("phonetic").and_then(|v| v.as_str())
    }

    pub fn with_phonetic(mut self, phonetic: impl Into<String>) -> Self {
        self.extra
            .insert("phonetic".to_string(), Value::String(phonetic.into()));
        self
    }
}

/// An author of a note (RFC 9553 §2.8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Author {
    #[serde(
        rename = "@type",
        default = "default_author_type",
        skip_serializing_if = "Option::is_none"
    )]
    pub author_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_author_type() -> Option<String> {
    Some("Author".to_owned())
}

impl Author {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            author_type: Some("Author".to_owned()),
            name: Some(name.into()),
            uri: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }
}

/// JSContact Note (RFC 9553 §2.8.3): one free-text note about the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// The note's text. Mandatory per RFC 9553 §2.8.3, and the only part of
    /// a note a `NOTE` line has room for.
    #[serde(default)]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_note_author"
    )]
    pub author: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn serialize_note_author<S>(value: &Option<Value>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(Value::String(s)) => {
            let author = Author::new(s);
            author.serialize(serializer)
        }
        Some(v) => v.serialize(serializer),
        None => serializer.serialize_none(),
    }
}

impl Note {
    pub fn new(note: impl Into<String>) -> Self {
        Self {
            note: note.into(),
            ..Self::default()
        }
    }

    pub fn with_created(mut self, created: impl Into<UtcDate>) -> Self {
        self.created = Some(created.into());
        self
    }

    pub fn with_author(mut self, author: Author) -> Self {
        self.author = Some(serde_json::to_value(author).unwrap_or(Value::Null));
        self
    }

    pub fn with_author_name(mut self, name: impl Into<String>) -> Self {
        self.author = Some(serde_json::json!({
            "@type": "Author",
            "name": name.into(),
        }));
        self
    }

    pub fn author_name(&self) -> Option<&str> {
        let val = self.author.as_ref()?;
        if let Some(s) = val.as_str() {
            Some(s)
        } else {
            val.get("name").and_then(Value::as_str)
        }
    }

    pub fn author(&self) -> Option<Author> {
        let val = self.author.as_ref()?;
        if val.is_object() {
            serde_json::from_value(val.clone()).ok()
        } else {
            val.as_str().map(Author::new)
        }
    }
}

/// A partial date (RFC 9553 §2.8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialDate {
    #[serde(rename = "@type", default = "default_partial_date_type")]
    pub date_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_scale: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_partial_date_type() -> String {
    "PartialDate".to_owned()
}

impl Default for PartialDate {
    fn default() -> Self {
        Self {
            date_type: "PartialDate".to_owned(),
            year: None,
            month: None,
            day: None,
            calendar_scale: None,
            extra: BTreeMap::new(),
        }
    }
}

impl PartialDate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_year(mut self, year: i32) -> Self {
        self.year = Some(year);
        self
    }

    pub fn with_month(mut self, month: u32) -> Self {
        self.month = Some(month);
        self
    }

    pub fn with_day(mut self, day: u32) -> Self {
        self.day = Some(day);
        self
    }

    pub fn with_calendar_scale(mut self, scale: impl Into<String>) -> Self {
        self.calendar_scale = Some(scale.into());
        self
    }
}

/// JSContact Anniversary (RFC 9553 §2.8.1): one memorable date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Anniversary {
    /// `birth`, `death` or `wedding`. Mandatory per RFC 9553 §2.8.1, which gives it no default.
    #[serde(default)]
    pub kind: String,
    /// Raw date string as received from the server.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_anniversary_date"
    )]
    pub date: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn serialize_anniversary_date<S>(value: &Option<Value>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(Value::String(s)) => {
            if let Some(partial) = parse_iso_partial_date(s) {
                partial.serialize(serializer)
            } else {
                value.serialize(serializer)
            }
        }
        Some(v) => v.serialize(serializer),
        None => serializer.serialize_none(),
    }
}

fn parse_iso_partial_date(s: &str) -> Option<PartialDate> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() == 3 {
        let year: i32 = parts[0].parse().ok()?;
        let month: u32 = parts[1].parse().ok()?;
        let day: u32 = parts[2].parse().ok()?;
        Some(
            PartialDate::new()
                .with_year(year)
                .with_month(month)
                .with_day(day),
        )
    } else if parts.len() == 2 {
        let year: i32 = parts[0].parse().ok()?;
        let month: u32 = parts[1].parse().ok()?;
        Some(PartialDate::new().with_year(year).with_month(month))
    } else if parts.len() == 1 && s.len() == 4 {
        let year: i32 = s.parse().ok()?;
        Some(PartialDate::new().with_year(year))
    } else {
        None
    }
}

impl Anniversary {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ..Self::default()
        }
    }

    pub fn with_date(mut self, date: Value) -> Self {
        self.date = Some(date);
        self
    }

    pub fn with_partial_date(
        mut self,
        year: Option<i32>,
        month: Option<u32>,
        day: Option<u32>,
    ) -> Self {
        let mut pd = PartialDate::new();
        pd.year = year;
        pd.month = month;
        pd.day = day;
        self.date = Some(serde_json::to_value(pd).unwrap_or(Value::Null));
        self
    }

    pub fn label(&self) -> Option<&str> {
        self.extra.get("label").and_then(|v| v.as_str())
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.extra
            .insert("label".to_owned(), Value::String(label.into()));
        self
    }

    pub fn pref(&self) -> Option<u32> {
        self.extra
            .get("pref")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.extra
            .insert("pref".to_owned(), Value::Number(pref.into()));
        self
    }

    pub fn with_place(mut self, place: Address) -> Self {
        self.extra.insert(
            "place".to_owned(),
            serde_json::to_value(place).unwrap_or(Value::Null),
        );
        self
    }

    pub fn partial_date(&self) -> Option<PartialDate> {
        let val = self.date.as_ref()?;
        if val.is_object() {
            serde_json::from_value(val.clone()).ok()
        } else if let Some(s) = val.as_str() {
            parse_iso_partial_date(s)
        } else {
            None
        }
    }
}

/// JSContact Link (RFC 9553 §2.6.3): one resource the contact points at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// Where the resource is. Mandatory per RFC 9553 §2.6.3, and the only
    /// part of a link a `URL` line has room for.
    #[serde(default)]
    pub uri: String,
    /// What kind of link it is: `contact` (a URI for getting in touch) is
    /// the defined kind in RFC 9553 §2.6.3, with no default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Link {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            ..Self::default()
        }
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// JSContact Calendar (RFC 9553 §2.4.1): one calendaring resource of the
/// contact: a calendar of theirs, or the free/busy data drawn from one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    /// What the resource is: `calendar` or `freeBusy` per RFC 9553 §2.4.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Where the resource is. Mandatory per RFC 9553 §2.4.1, and the only
    /// part of a calendar either line has room for.
    #[serde(default)]
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Calendar {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            ..Self::default()
        }
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// JSContact SchedulingAddress (RFC 9553 §2.4.2): one scheduling address of the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulingAddress {
    #[serde(
        rename = "@type",
        default = "default_scheduling_address_type",
        serialize_with = "serialize_scheduling_address_type",
        deserialize_with = "deserialize_scheduling_address_type"
    )]
    pub scheduling_address_type: Option<String>,
    #[serde(default)]
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_scheduling_address_type() -> Option<String> {
    Some("SchedulingAddress".to_owned())
}

fn serialize_scheduling_address_type<S>(
    _value: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str("SchedulingAddress")
}

fn deserialize_scheduling_address_type<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt.as_deref() {
        Some("SchedulingAddress") => Ok(Some("SchedulingAddress".to_owned())),
        Some(other) => Ok(Some(other.to_owned())),
        None => Ok(Some("SchedulingAddress".to_owned())),
    }
}

impl Default for SchedulingAddress {
    fn default() -> Self {
        Self {
            scheduling_address_type: Some("SchedulingAddress".to_owned()),
            uri: String::new(),
            contexts: None,
            pref: None,
            label: None,
            extra: BTreeMap::new(),
        }
    }
}

impl SchedulingAddress {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            ..Self::default()
        }
    }

    pub fn with_type(mut self, address_type: impl Into<String>) -> Self {
        self.scheduling_address_type = Some(address_type.into());
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// JSContact Media (RFC 9553 §2.6.4): one media resource the card carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    /// What the resource is: `photo`, `sound` or `logo` per RFC 9553 §2.6.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Where the resource is. Mandatory per RFC 9553 §2.6.4, and for a picture
    /// the card carries rather than points at, a `data:` URI (RFC 2397).
    #[serde(default)]
    pub uri: String,
    /// The media type of the resource, which RFC 9553 §2.6.4 asks for when the
    /// URI does not state one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Media {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            ..Self::default()
        }
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// JSContact OnlineService (RFC 9553 §2.3.2): the contact as one online
/// service or protocol knows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OnlineService {
    /// The name of the service or protocol: `Jabber`, `Matrix`, `Skype`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// The name the contact is known by at the service. Free text per RFC 9553
    /// §2.3.2, and what the vCard line states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// The contact's identifier at the service as a URI (RFC 9553 §2.3.2
    /// requires RFC 3986 §3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl OnlineService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// JSContact Relation (RFC 9553 §2.1.8).
/// Describes how an entity relates to the contact.
/// The entity key is a Card `uid` or free text (RFC 9555 §2.9.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    /// Relation types the entity stands in (RFC 9553 §1.4.3 Set mapping types
    /// such as `spouse`, `child`, `colleague` to boolean values).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Relation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_relation(mut self, relation: BTreeMap<String, bool>) -> Self {
        self.relation = Some(
            relation
                .into_iter()
                .map(|(k, v)| (k, Value::Bool(v)))
                .collect(),
        );
        self
    }
}

/// `ContactCard/query` filter conditions (RFC 9610 §3.3). Flat conditions
/// only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContactCardQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_address_book: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl ContactCardQueryFilter {
    pub fn in_address_book(address_book_id: impl Into<Id>) -> Self {
        Self {
            in_address_book: Some(address_book_id.into()),
            ..Self::default()
        }
    }

    pub fn uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    pub fn phone(mut self, phone: impl Into<String>) -> Self {
        self.phone = Some(phone.into());
        self
    }

    pub fn online_service(mut self, service: impl Into<String>) -> Self {
        self.online_service = Some(service.into());
        self
    }

    pub fn address(mut self, address: impl Into<String>) -> Self {
        self.address = Some(address.into());
        self
    }

    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }
}

/// The `SetError` type RFC 9610 §2.4 adds for `AddressBook/set`.
pub mod address_book_set_error {
    pub const HAS_CARD: &str = "addressBookHasCard";
    pub const ADDRESS_BOOK_HAS_CONTENTS: &str = "addressBookHasContents";
}

/// Standard RFC 9553 §2.8.1 anniversary kinds.
pub mod anniversary_kind {
    pub const BIRTH: &str = "birth";
    pub const DEATH: &str = "death";
    pub const WEDDING: &str = "wedding";
}

/// Standard RFC 9553 §2.2.4 job title kinds.
pub mod title_kind {
    pub const TITLE: &str = "title";
    pub const ROLE: &str = "role";
}

/// Standard RFC 9553 §2.4.1 calendar resource kinds.
pub mod calendar_kind {
    pub const CALENDAR: &str = "calendar";
    pub const FREE_BUSY: &str = "freeBusy";
}

/// Standard RFC 9553 §2.6.4 media resource kinds.
pub mod media_kind {
    pub const PHOTO: &str = "photo";
    pub const SOUND: &str = "sound";
    pub const LOGO: &str = "logo";
}

/// Standard RFC 9553 §2.6.3 link kinds.
pub mod link_kind {
    pub const CONTACT: &str = "contact";
}

/// Standard RFC 9553 §2.2.1 name component kinds.
pub mod name_component_kind {
    pub const PREFIX: &str = "prefix";
    pub const TITLE: &str = "title";
    pub const GIVEN: &str = "given";
    pub const GIVEN2: &str = "given2";
    pub const MIDDLE: &str = "middle";
    pub const SURNAME: &str = "surname";
    pub const SURNAME2: &str = "surname2";
    pub const SUFFIX: &str = "suffix";
    pub const GENERATION: &str = "generation";
    pub const CREDENTIAL: &str = "credential";
    pub const SEPARATOR: &str = "separator";
}

/// Standard RFC 9553 §2.5.1 address component kinds.
pub mod address_component_kind {
    pub const NAME: &str = "name";
    pub const UNIT: &str = "unit";
    pub const FLOOR: &str = "floor";
    pub const STREET: &str = "street";
    pub const APPARTMENT: &str = "appartment";
    pub const APARTMENT: &str = "apartment";
    pub const ROOM: &str = "room";
    pub const BUILDING: &str = "building";
    pub const LOCALITY: &str = "locality";
    pub const DISTRICT: &str = "district";
    pub const SUBDISTRICT: &str = "subdistrict";
    pub const REGION: &str = "region";
    pub const POSTCODE: &str = "postcode";
    pub const POST_OFFICE_BOX: &str = "postOfficeBox";
    pub const COUNTRY: &str = "country";
    pub const COUNTRY_CODE: &str = "countryCode";
}

/// JSContact CryptoKey (RFC 9553 §2.6.1): a cryptographic key for the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CryptoKey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default)]
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CryptoKey {
    pub fn new(kind: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            kind: Some(kind.into()),
            uri: uri.into(),
            media_type: None,
            pref: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }
}

/// Standard RFC 9553 §2.6.1 crypto key kinds.
pub mod crypto_key_kind {
    pub const KEY: &str = "key";
    pub const CERT: &str = "cert";
}

/// JSContact Directory (RFC 9553 §2.6.2): a directory service for the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Directory {
    #[serde(
        rename = "@type",
        default = "default_directory_type",
        serialize_with = "serialize_directory_type",
        deserialize_with = "deserialize_directory_type"
    )]
    pub directory_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default)]
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_u32"
    )]
    pub list_as: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_directory_type() -> Option<String> {
    Some("Directory".to_owned())
}

fn serialize_directory_type<S>(_value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str("Directory")
}

fn deserialize_directory_type<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt.as_deref() {
        Some("Directory") => Ok(Some("Directory".to_owned())),
        Some(other) => Ok(Some(other.to_owned())),
        None => Ok(Some("Directory".to_owned())),
    }
}

impl Default for Directory {
    fn default() -> Self {
        Self {
            directory_type: Some("Directory".to_owned()),
            kind: None,
            uri: String::new(),
            media_type: None,
            list_as: None,
            pref: None,
            label: None,
            contexts: None,
            extra: BTreeMap::new(),
        }
    }
}

impl Directory {
    pub fn new(kind: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            kind: Some(kind.into()),
            uri: uri.into(),
            ..Self::default()
        }
    }

    pub fn with_type(mut self, directory_type: impl Into<String>) -> Self {
        self.directory_type = Some(directory_type.into());
        self
    }

    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    pub fn with_list_as(mut self, list_as: u32) -> Self {
        self.list_as = Some(list_as);
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }
}

/// Standard RFC 9553 §2.6.2 directory kinds.
pub mod directory_kind {
    pub const DIRECTORY: &str = "directory";
    pub const ENTRY: &str = "entry";
}

fn deserialize_optional_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct OptionalU32Visitor;

    impl<'de> serde::de::Visitor<'de> for OptionalU32Visitor {
        type Value = Option<u32>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an unsigned integer or string containing an unsigned integer")
        }

        fn visit_none<E>(self) -> Result<Option<u32>, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Option<u32>, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(OptionalU32Visitor)
        }

        fn visit_u64<E>(self, value: u64) -> Result<Option<u32>, E>
        where
            E: serde::de::Error,
        {
            u32::try_from(value).map(Some).map_err(E::custom)
        }

        fn visit_i64<E>(self, value: i64) -> Result<Option<u32>, E>
        where
            E: serde::de::Error,
        {
            u32::try_from(value).map(Some).map_err(E::custom)
        }

        fn visit_str<E>(self, value: &str) -> Result<Option<u32>, E>
        where
            E: serde::de::Error,
        {
            value.parse::<u32>().map(Some).map_err(E::custom)
        }
    }

    deserializer.deserialize_option(OptionalU32Visitor)
}

/// JSContact PersonalInfo (RFC 9553 §2.8.4): personal information about the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PersonalInfo {
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub personal_info_type: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_u32"
    )]
    pub list_as: Option<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PersonalInfo {
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: Some(value.into()),
            personal_info_type: None,
            list_as: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_type(mut self, info_type: impl Into<String>) -> Self {
        self.personal_info_type = Some(info_type.into());
        self
    }

    pub fn with_list_as(mut self, list_as: u32) -> Self {
        self.list_as = Some(list_as);
        self
    }
}

/// Standard RFC 9553 §2.8.4 personal info kinds.
pub mod personal_info_kind {
    pub const GENDER: &str = "gender";
    pub const EXPERTISE: &str = "expertise";
    pub const HOBBY: &str = "hobby";
    pub const INTEREST: &str = "interest";
}

/// JSContact CardGroup (RFC 9553 §2.1.2): a group of contact cards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(
        rename = "@type",
        default = "default_card_group_type",
        serialize_with = "serialize_card_group_type",
        deserialize_with = "deserialize_card_group_type"
    )]
    pub card_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<BTreeMap<String, bool>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_card_group_type() -> Option<String> {
    Some("CardGroup".to_owned())
}

fn serialize_card_group_type<S>(_value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str("CardGroup")
}

fn deserialize_card_group_type<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt.as_deref() {
        Some("CardGroup") | Some("Group") | Some("Card") => Ok(Some("CardGroup".to_owned())),
        Some(other) => Ok(Some(other.to_owned())),
        None => Ok(Some("CardGroup".to_owned())),
    }
}

impl Default for CardGroup {
    fn default() -> Self {
        Self {
            id: None,
            card_type: Some("CardGroup".to_owned()),
            name: None,
            members: None,
            extra: BTreeMap::new(),
        }
    }
}

impl CardGroup {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }

    pub fn with_type(mut self, card_type: impl Into<String>) -> Self {
        self.card_type = Some(card_type.into());
        self
    }

    pub fn with_members(mut self, members: BTreeMap<String, bool>) -> Self {
        self.members = Some(members);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Contacts capability properties (RFC 9610 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContactsCapability {
    #[serde(default)]
    pub max_size_attachments_per_card: u64,
    #[serde(default)]
    pub max_number_of_cards_in_set: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ContactsCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_size_attachments_per_card(mut self, max: u64) -> Self {
        self.max_size_attachments_per_card = max;
        self
    }

    pub fn with_max_number_of_cards_in_set(mut self, max: u64) -> Self {
        self.max_number_of_cards_in_set = max;
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard RFC 9553 §2.1.1 JSContact card kinds.
pub mod card_kind {
    pub const INDIVIDUAL: &str = "individual";
    pub const GROUP: &str = "group";
    pub const ORG: &str = "org";
    pub const LOCATION: &str = "location";
    pub const DEVICE: &str = "device";
    pub const APPLICATION: &str = "application";
}

/// Standard RFC 9553 §2.2.5 grammatical genders.
pub mod grammatical_gender {
    pub const ANIMATE: &str = "animate";
    pub const INANIMATE: &str = "inanimate";
    pub const FEMININE: &str = "feminine";
    pub const MASCULINE: &str = "masculine";
    pub const NEUTER: &str = "neuter";
    pub const COMMON: &str = "common";
}

/// JSContact Pronouns (RFC 9553 §2.2.4): pronouns to use when addressing or speaking about the contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Pronouns {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub pronouns_type: Option<String>,
    #[serde(default)]
    pub pronouns: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Pronouns {
    pub fn new(pronouns: impl Into<String>) -> Self {
        Self {
            pronouns: pronouns.into(),
            ..Self::default()
        }
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }
}

/// JSContact pronouns value, supporting either an RFC 9553 §2.2.4 map of
/// `Pronouns` or a shorthand plain string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PronounsValue {
    Map(BTreeMap<String, Pronouns>),
    Text(String),
}

impl std::ops::Deref for PronounsValue {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Text(s) => s.as_str(),
            Self::Map(m) => m.values().next().map(|p| p.pronouns.as_str()).unwrap_or(""),
        }
    }
}

impl From<String> for PronounsValue {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for PronounsValue {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl From<BTreeMap<String, Pronouns>> for PronounsValue {
    fn from(m: BTreeMap<String, Pronouns>) -> Self {
        Self::Map(m)
    }
}

/// JSContact SpeakToAs (RFC 9553 §2.2.4): how to address the contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpeakToAs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammatical_gender: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<PronounsValue>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SpeakToAs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_grammatical_gender(mut self, gender: impl Into<String>) -> Self {
        self.grammatical_gender = Some(gender.into());
        self
    }

    pub fn with_pronouns(mut self, pronouns: impl Into<PronounsValue>) -> Self {
        self.pronouns = Some(pronouns.into());
        self
    }

    pub fn with_pronouns_entry(mut self, key: impl Into<String>, pronouns: Pronouns) -> Self {
        let mut map = match self.pronouns.take() {
            Some(PronounsValue::Map(m)) => m,
            Some(PronounsValue::Text(s)) => {
                let mut m = BTreeMap::new();
                m.insert("default".to_string(), Pronouns::new(s));
                m
            }
            None => BTreeMap::new(),
        };
        map.insert(key.into(), pronouns);
        self.pronouns = Some(PronounsValue::Map(map));
        self
    }
}

/// JSContact LanguagePref (RFC 9553 §2.8.5): preferred language for communication.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LanguagePref {
    #[serde(default)]
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl LanguagePref {
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
            ..Self::default()
        }
    }

    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }
}

impl<'de> serde::Deserialize<'de> for LanguagePref {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct LanguagePrefVisitor;

        impl<'de> serde::de::Visitor<'de> for LanguagePrefVisitor {
            type Value = LanguagePref;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a LanguagePref object, number, or string")
            }

            fn visit_u64<E>(self, value: u64) -> Result<LanguagePref, E>
            where
                E: serde::de::Error,
            {
                Ok(LanguagePref {
                    pref: Some(u32::try_from(value).map_err(E::custom)?),
                    ..LanguagePref::default()
                })
            }

            fn visit_i64<E>(self, value: i64) -> Result<LanguagePref, E>
            where
                E: serde::de::Error,
            {
                Ok(LanguagePref {
                    pref: Some(u32::try_from(value).map_err(E::custom)?),
                    ..LanguagePref::default()
                })
            }

            fn visit_str<E>(self, value: &str) -> Result<LanguagePref, E>
            where
                E: serde::de::Error,
            {
                Ok(LanguagePref {
                    language: value.to_owned(),
                    ..LanguagePref::default()
                })
            }

            fn visit_map<M>(self, map: M) -> Result<LanguagePref, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct RawLanguagePref {
                    #[serde(default)]
                    language: String,
                    #[serde(default)]
                    contexts: Option<Value>,
                    #[serde(default)]
                    pref: Option<u32>,
                    #[serde(flatten)]
                    extra: BTreeMap<String, Value>,
                }

                let raw = RawLanguagePref::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                Ok(LanguagePref {
                    language: raw.language,
                    contexts: raw.contexts,
                    pref: raw.pref,
                    extra: raw.extra,
                })
            }
        }

        deserializer.deserialize_any(LanguagePrefVisitor)
    }
}

/// `ContactCard/parse` arguments (RFC 9610 §3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactCardParseRequest {
    pub account_id: Id,
    pub blob_ids: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
}

impl ContactCardParseRequest {
    pub fn new(
        account_id: impl Into<Id>,
        blob_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            blob_ids: blob_ids.into_iter().map(Into::into).collect(),
            properties: None,
        }
    }

    pub fn properties(mut self, properties: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }
}

/// `ContactCard/parse` response (RFC 9610 §3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactCardParseResponse {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<BTreeMap<Id, ContactCard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_parsable: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_found: Option<Vec<Id>>,
}

impl ContactCardParseResponse {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            parsed: None,
            not_parsable: None,
            not_found: None,
        }
    }

    pub fn with_parsed(mut self, parsed: BTreeMap<Id, ContactCard>) -> Self {
        self.parsed = Some(parsed);
        self
    }

    pub fn with_not_parsable(
        mut self,
        not_parsable: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.not_parsable = Some(not_parsable.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = Some(not_found.into_iter().map(Into::into).collect());
        self
    }
}

/// The `SetError` types RFC 9610 §3.2 adds for `ContactCard/set`.
pub mod contact_card_set_error {
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
}
