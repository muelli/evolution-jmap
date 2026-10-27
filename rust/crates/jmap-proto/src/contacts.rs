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
#[non_exhaustive]
pub struct AddressBook {
    /// The server-assigned id. Absent on an `AddressBook/set create` argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The user-visible name of the address book.
    #[serde(default)]
    pub name: String,
    /// A longer, free text description of the address book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A number the client may use to order address books for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u32>,
    /// True if this is the account's default address book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// True if the user has subscribed to this shared address book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    /// Server-computed permissions on this address book (RFC 9610 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<AddressBookRights>,
    /// Rights granted to other principals (RFC 9610 §2), keyed by principal id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_with: Option<BTreeMap<Id, AddressBookRights>>,
    /// True if the requesting account may delete this address book.
    #[serde(default, skip_serializing)]
    pub may_delete: Option<bool>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AddressBook {
    /// Create an address book with the given name and no other properties set.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the longer, free text description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the display sort order.
    pub fn with_sort_order(mut self, sort_order: u32) -> Self {
        self.sort_order = Some(sort_order);
        self
    }

    /// Set whether this is the account's default address book.
    pub fn is_default(mut self, is_default: bool) -> Self {
        self.is_default = Some(is_default);
        self
    }

    /// Set whether the user has subscribed to this shared address book.
    pub fn is_subscribed(mut self, is_subscribed: bool) -> Self {
        self.is_subscribed = Some(is_subscribed);
        self
    }

    /// Set the server-computed permissions on this address book.
    pub fn with_my_rights(mut self, my_rights: AddressBookRights) -> Self {
        self.my_rights = Some(my_rights);
        self
    }

    /// Set the rights granted to other principals, keyed by principal id.
    pub fn with_share_with(mut self, share_with: BTreeMap<Id, AddressBookRights>) -> Self {
        self.share_with = Some(share_with);
        self
    }

    /// Set whether the requesting account may delete this address book.
    pub fn with_may_delete(mut self, may_delete: bool) -> Self {
        self.may_delete = Some(may_delete);
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `AddressBook/set` request arguments (RFC 9610 §2.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AddressBookSetRequest {
    /// The underlying `AddressBook/set` arguments: create, update and destroy.
    #[serde(flatten)]
    pub set: crate::methods::SetRequest<AddressBook>,
    /// If true, destroying an address book also destroys the cards in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_destroy_remove_contents: Option<bool>,
    /// If the destroyed default address book had this id, make it the new default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_set_is_default: Option<Id>,
}

impl AddressBookSetRequest {
    /// Create a request from the underlying `AddressBook/set` arguments.
    pub fn new(set: crate::methods::SetRequest<AddressBook>) -> Self {
        Self {
            set,
            on_destroy_remove_contents: None,
            on_success_set_is_default: None,
        }
    }

    /// Set whether destroying an address book also destroys the cards in it.
    pub fn with_on_destroy_remove_contents(mut self, on_destroy: bool) -> Self {
        self.on_destroy_remove_contents = Some(on_destroy);
        self
    }

    /// Set the address book to make the new default if the current default is destroyed.
    pub fn with_on_success_set_is_default(mut self, default_id: impl Into<Id>) -> Self {
        self.on_success_set_is_default = Some(default_id.into());
        self
    }
}

/// `AddressBook.myRights`/a `shareWith` entry, RFC 9610 §2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AddressBookRights {
    /// True if the account may see the cards in this address book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read: Option<bool>,
    /// True if the account may create, update and destroy cards in this address book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write: Option<bool>,
    /// True if the account may change the sharing settings of this address book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_share: Option<bool>,
    /// True if the account may delete this address book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AddressBookRights {
    /// Rights that grant every permission.
    pub fn all() -> Self {
        Self {
            may_read: Some(true),
            may_write: Some(true),
            may_share: Some(true),
            may_delete: Some(true),
            extra: BTreeMap::new(),
        }
    }

    /// Rights that grant read access only.
    pub fn read_only() -> Self {
        Self {
            may_read: Some(true),
            ..Self::default()
        }
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// Set whether the account may see the cards in this address book.
    pub fn with_may_read(mut self, may_read: bool) -> Self {
        self.may_read = Some(may_read);
        self
    }

    /// Set whether the account may create, update and destroy cards here.
    pub fn with_may_write(mut self, may_write: bool) -> Self {
        self.may_write = Some(may_write);
        self
    }

    /// Set whether the account may change the sharing settings.
    pub fn with_may_share(mut self, may_share: bool) -> Self {
        self.may_share = Some(may_share);
        self
    }

    /// Set whether the account may delete this address book.
    pub fn with_may_delete(mut self, may_delete: bool) -> Self {
        self.may_delete = Some(may_delete);
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
#[non_exhaustive]
pub struct ContactCard {
    /// The server-assigned id. Absent on a `ContactCard/set create` argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The address books the card belongs to, keyed by address book id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_book_ids: Option<BTreeMap<Id, bool>>,
    /// The JSContact object type. Always `"Card"` for a contact card.
    #[serde(
        rename = "@type",
        default = "default_contact_card_type",
        serialize_with = "serialize_contact_card_type",
        deserialize_with = "deserialize_contact_card_type"
    )]
    pub card_type: Option<String>,
    /// What the card represents: `individual`, `group`, `org`, one of the
    /// [`card_kind`] values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The JSContact version the card conforms to (RFC 9553 §2.1.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The language tag of the card's language-dependent properties, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// For a `group` card, the other cards that are members, keyed by uid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<BTreeMap<String, bool>>,
    /// The identifier of the product that last created or updated the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prod_id: Option<String>,
    /// A globally unique identifier for the card, stable across updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// The date and time the card was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    /// The date and time the card was last updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<UtcDate>,
    /// The name of the entity the card represents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Name>,

    /// Contact nicknames (RFC 9553 §2.2.2), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nicknames: Option<BTreeMap<String, Nickname>>,

    /// Email addresses for the contact (RFC 9553 §2.3.1), keyed by entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emails: Option<BTreeMap<String, ContactEmail>>,
    /// Phone numbers for the contact (RFC 9553 §2.3.3), keyed by entry ID.
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
    /// Extra properties the server sent that this struct does not name.
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
    /// Create an empty card with no properties set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the JSContact object type.
    pub fn with_type(mut self, card_type: impl Into<String>) -> Self {
        self.card_type = Some(card_type.into());
        self
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add the card to the given address book.
    pub fn with_address_book_id(mut self, id: impl Into<Id>) -> Self {
        let mut map = self.address_book_ids.unwrap_or_default();
        map.insert(id.into(), true);
        self.address_book_ids = Some(map);
        self
    }

    /// Set the globally unique identifier.
    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    /// Set the name of the entity the card represents.
    pub fn with_name(mut self, name: Name) -> Self {
        self.name = Some(name);
        self
    }

    /// Set what the card represents, one of the [`card_kind`] values.
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Add one email address, keyed by an id of the caller's choosing.
    pub fn with_email(mut self, key: impl Into<String>, email: ContactEmail) -> Self {
        let mut map = self.emails.unwrap_or_default();
        map.insert(key.into(), email);
        self.emails = Some(map);
        self
    }

    /// Add one phone number, keyed by an id of the caller's choosing.
    pub fn with_phone(mut self, key: impl Into<String>, phone: ContactPhone) -> Self {
        let mut map = self.phones.unwrap_or_default();
        map.insert(key.into(), phone);
        self.phones = Some(map);
        self
    }

    /// Add one postal address, keyed by an id of the caller's choosing.
    pub fn with_address(mut self, key: impl Into<String>, address: Address) -> Self {
        let mut map = self.addresses.unwrap_or_default();
        map.insert(key.into(), address);
        self.addresses = Some(map);
        self
    }

    /// Add one organization, keyed by an id of the caller's choosing.
    pub fn with_organization(mut self, key: impl Into<String>, org: Organization) -> Self {
        let mut map = self.organizations.unwrap_or_default();
        map.insert(key.into(), org);
        self.organizations = Some(map);
        self
    }

    /// Add one job title or role, keyed by an id of the caller's choosing.
    pub fn with_title(mut self, key: impl Into<String>, title: Title) -> Self {
        let mut map = self.titles.unwrap_or_default();
        map.insert(key.into(), title);
        self.titles = Some(map);
        self
    }

    /// Add one free-text note, keyed by an id of the caller's choosing.
    pub fn with_note(mut self, key: impl Into<String>, note: Note) -> Self {
        let mut map = self.notes.unwrap_or_default();
        map.insert(key.into(), note);
        self.notes = Some(map);
        self
    }

    /// Set the date and time the card was created.
    pub fn with_created(mut self, created: impl Into<UtcDate>) -> Self {
        self.created = Some(created.into());
        self
    }

    /// Set the date and time the card was last updated.
    pub fn with_updated(mut self, updated: impl Into<UtcDate>) -> Self {
        self.updated = Some(updated.into());
        self
    }

    /// Set how to address the contact.
    pub fn with_speak_to_as(mut self, speak_to_as: SpeakToAs) -> Self {
        self.speak_to_as = Some(speak_to_as);
        self
    }

    /// Add one scheduling address, keyed by an id of the caller's choosing.
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
#[non_exhaustive]
pub struct Name {
    /// The parts the name is built from, each naming what it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<NameComponent>>,
    /// The name written out as it should be displayed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
    /// True if `components` lists its entries in display order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_ordered: Option<bool>,
    /// Transliterated or phonetic forms to sort by, keyed by language tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<BTreeMap<String, String>>,
    /// The separator to join unordered `components` with when displaying the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_separator: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Name {
    /// Create a name with the given display form and no other properties set.
    pub fn new(full: impl Into<String>) -> Self {
        Self {
            full: Some(full.into()),
            ..Self::default()
        }
    }

    /// Set the parts the name is built from.
    pub fn with_components(mut self, components: impl IntoIterator<Item = NameComponent>) -> Self {
        self.components = Some(components.into_iter().collect());
        self
    }

    /// Set whether `components` lists its entries in display order.
    pub fn is_ordered(mut self, is_ordered: bool) -> Self {
        self.is_ordered = Some(is_ordered);
        self
    }

    /// Set the transliterated or phonetic forms to sort by.
    pub fn with_sort_as(mut self, sort_as: BTreeMap<String, String>) -> Self {
        self.sort_as = Some(sort_as);
        self
    }

    /// Set the separator to join unordered components with.
    pub fn with_default_separator(mut self, separator: impl Into<String>) -> Self {
        self.default_separator = Some(separator.into());
        self
    }
}

/// One name component: kind is `given`, `surname`, `title`, etc.
/// Unmodeled properties (such as phonetic spellings) are preserved in `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct NameComponent {
    /// One of the [`name_component_kind`] values.
    pub kind: String,
    /// The text of this part of the name.
    pub value: String,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl NameComponent {
    /// Create a name component of the given kind and text.
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: value.into(),
            extra: BTreeMap::new(),
        }
    }

    /// The phonetic spelling of this component, if the server sent one.
    pub fn phonetic(&self) -> Option<&str> {
        self.extra.get("phonetic").and_then(|v| v.as_str())
    }

    /// Set the phonetic spelling of this component.
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
#[non_exhaustive]
pub struct Nickname {
    /// The nickname itself. Mandatory per RFC 9553 §2.2.2, and the only part
    /// of it a `NICKNAME` line has room for.
    #[serde(default)]
    pub name: String,
    /// The contexts the nickname applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The preference order among nicknames, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Nickname {
    /// Create a nickname with the given name and no other properties set.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Set the contexts the nickname applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    /// Set the preference order among nicknames.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }
}

/// JSContact EmailAddress (RFC 9553 §2.3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContactEmail {
    /// The email address itself.
    #[serde(default)]
    pub address: String,
    /// The contexts the email address applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The preference order among email addresses, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ContactEmail {
    /// Create an email entry with the given address and no other properties set.
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            ..Self::default()
        }
    }

    /// Set the contexts the email address applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    /// Set the preference order among email addresses.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// A free-text label for the email address, if the server sent one.
    pub fn label(&self) -> Option<&str> {
        self.extra.get("label").and_then(|v| v.as_str())
    }

    /// Set a free-text label for the email address.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.extra
            .insert("label".to_owned(), Value::String(label.into()));
        self
    }
}

/// JSContact Phone (RFC 9553 §2.3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContactPhone {
    /// The phone number itself, in any format the contact provided.
    #[serde(default)]
    pub number: String,
    /// What the number can be used for: `voice`, `fax`, `text`, per RFC 9553 §2.3.3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Value>,
    /// The contexts the phone number applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The preference order among phone numbers, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ContactPhone {
    /// Create a phone entry with the given number and no other properties set.
    pub fn new(number: impl Into<String>) -> Self {
        Self {
            number: number.into(),
            ..Self::default()
        }
    }

    /// Set the contexts the phone number applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    /// Set what the number can be used for.
    pub fn with_features(mut self, features: Value) -> Self {
        self.features = Some(features);
        self
    }

    /// Set the preference order among phone numbers.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// A free-text label for the phone number, if the server sent one.
    pub fn label(&self) -> Option<&str> {
        self.extra.get("label").and_then(|v| v.as_str())
    }

    /// Set a free-text label for the phone number.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.extra
            .insert("label".to_owned(), Value::String(label.into()));
        self
    }
}

/// JSContact Organization (RFC 9553 §2.2.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Organization {
    /// The organisation's own name, which the `ORG` value states first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The units within it, outermost first: the departments the `ORG` value
    /// lists after the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<Vec<OrgUnit>>,
    /// Transliterated or phonetic forms to sort by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<String>,
    /// The contexts the organization applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Organization {
    /// Create an organization with the given name and no other properties set.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }

    /// Set the units within the organization, outermost first.
    pub fn with_units(mut self, units: impl IntoIterator<Item = OrgUnit>) -> Self {
        self.units = Some(units.into_iter().collect());
        self
    }

    /// Set the transliterated or phonetic forms to sort by.
    pub fn with_sort_as(mut self, sort_as: impl Into<String>) -> Self {
        self.sort_as = Some(sort_as.into());
        self
    }

    /// Set the contexts the organization applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }
}

/// JSContact OrgUnit (RFC 9553 §2.2.3): one unit of an [`Organization`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct OrgUnit {
    /// The unit's name.
    #[serde(default)]
    pub name: String,
    /// Transliterated or phonetic forms to sort by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl OrgUnit {
    /// Create a unit with the given name and no other properties set.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            sort_as: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the transliterated or phonetic forms to sort by.
    pub fn with_sort_as(mut self, sort_as: impl Into<String>) -> Self {
        self.sort_as = Some(sort_as.into());
        self
    }
}

/// JSContact Title (RFC 9553 §2.2.4): a job title the contact holds, or a
/// role it plays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Title {
    /// The title or role itself.
    #[serde(default)]
    pub name: String,
    /// `title` or `role`. Absent means `title` (RFC 9553 §2.2.4 default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The id of the [`Organization`] entry this title applies to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Title {
    /// Create a title with the given name and no other properties set.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Set whether this is a `title` or a `role`.
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Set the id of the organization entry this title applies to.
    pub fn with_organization_id(mut self, organization_id: impl Into<String>) -> Self {
        self.organization_id = Some(organization_id.into());
        self
    }
}

/// JSContact Address (RFC 9553 §2.5.1): one postal address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Address {
    /// The parts the address is built from, each naming what it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<AddressComponent>>,
    /// The contexts the address applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The address written out as it should be printed, line breaks and all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
    /// True if `components` lists its entries in display order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_ordered: Option<bool>,
    /// The ISO 3166-1 country code of the address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    /// The geographic coordinates of the address, as a `geo:` URI (RFC 5870).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<String>,
    /// The time zone the address is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// The preference order among addresses, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// The separator to join unordered `components` with when displaying the address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_separator: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Address {
    /// Create an empty address with no properties set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the parts the address is built from.
    pub fn with_components(
        mut self,
        components: impl IntoIterator<Item = AddressComponent>,
    ) -> Self {
        self.components = Some(components.into_iter().collect());
        self
    }

    /// Set the address written out as it should be printed.
    pub fn with_full(mut self, full: impl Into<String>) -> Self {
        self.full = Some(full.into());
        self
    }

    /// Set whether `components` lists its entries in display order.
    pub fn is_ordered(mut self, is_ordered: bool) -> Self {
        self.is_ordered = Some(is_ordered);
        self
    }

    /// Set the ISO 3166-1 country code.
    pub fn with_country_code(mut self, country_code: impl Into<String>) -> Self {
        self.country_code = Some(country_code.into());
        self
    }

    /// Set the geographic coordinates, as a `geo:` URI.
    pub fn with_coordinates(mut self, coordinates: impl Into<String>) -> Self {
        self.coordinates = Some(coordinates.into());
        self
    }

    /// Set the time zone the address is in.
    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    /// Set the preference order among addresses.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// Set the contexts the address applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    /// Set the separator to join unordered components with.
    pub fn with_default_separator(mut self, separator: impl Into<String>) -> Self {
        self.default_separator = Some(separator.into());
        self
    }
}

/// One part of an [`Address`]: `kind` is `name` (the street), `locality`,
/// `postcode`, `floor`, …
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AddressComponent {
    /// One of the [`address_component_kind`] values.
    #[serde(default)]
    pub kind: String,
    /// The text of this part of the address.
    #[serde(default)]
    pub value: String,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl AddressComponent {
    /// Create an address component of the given kind and text.
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: value.into(),
            extra: BTreeMap::new(),
        }
    }

    /// The phonetic spelling of this component, if the server sent one.
    pub fn phonetic(&self) -> Option<&str> {
        self.extra.get("phonetic").and_then(|v| v.as_str())
    }

    /// Set the phonetic spelling of this component.
    pub fn with_phonetic(mut self, phonetic: impl Into<String>) -> Self {
        self.extra
            .insert("phonetic".to_string(), Value::String(phonetic.into()));
        self
    }
}

/// An author of a note (RFC 9553 §2.8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Author {
    /// The JSContact object type. Always `"Author"` for an author.
    #[serde(
        rename = "@type",
        default = "default_author_type",
        skip_serializing_if = "Option::is_none"
    )]
    pub author_type: Option<String>,
    /// The author's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A URI identifying the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_author_type() -> Option<String> {
    Some("Author".to_owned())
}

impl Author {
    /// Create an author with the given name and no other properties set.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            author_type: Some("Author".to_owned()),
            name: Some(name.into()),
            uri: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the URI identifying the author.
    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }
}

/// JSContact Note (RFC 9553 §2.8.3): one free-text note about the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Note {
    /// The note's text. Mandatory per RFC 9553 §2.8.3, and the only part of
    /// a note a `NOTE` line has room for.
    #[serde(default)]
    pub note: String,
    /// The date and time the note was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    /// The author of the note, as an [`Author`] object or a plain name string.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_note_author"
    )]
    pub author: Option<Value>,
    /// Extra properties the server sent that this struct does not name.
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
    /// Create a note with the given text and no other properties set.
    pub fn new(note: impl Into<String>) -> Self {
        Self {
            note: note.into(),
            ..Self::default()
        }
    }

    /// Set the date and time the note was created.
    pub fn with_created(mut self, created: impl Into<UtcDate>) -> Self {
        self.created = Some(created.into());
        self
    }

    /// Set the author as a full [`Author`] object.
    pub fn with_author(mut self, author: Author) -> Self {
        self.author = Some(serde_json::to_value(author).unwrap_or(Value::Null));
        self
    }

    /// Set the author to just a name, with no other author properties.
    pub fn with_author_name(mut self, name: impl Into<String>) -> Self {
        self.author = Some(serde_json::json!({
            "@type": "Author",
            "name": name.into(),
        }));
        self
    }

    /// The author's name, from either form `author` can take.
    pub fn author_name(&self) -> Option<&str> {
        let val = self.author.as_ref()?;
        if let Some(s) = val.as_str() {
            Some(s)
        } else {
            val.get("name").and_then(Value::as_str)
        }
    }

    /// The author as a full [`Author`] object, from either form `author` can take.
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
#[non_exhaustive]
pub struct PartialDate {
    /// The JSContact object type. Always `"PartialDate"`.
    #[serde(rename = "@type", default = "default_partial_date_type")]
    pub date_type: String,
    /// The year, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// The month, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u32>,
    /// The day of the month, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<u32>,
    /// The calendar system the date is in, if not Gregorian.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_scale: Option<String>,
    /// Extra properties the server sent that this struct does not name.
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
    /// Create an empty partial date with no fields set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the year.
    pub fn with_year(mut self, year: i32) -> Self {
        self.year = Some(year);
        self
    }

    /// Set the month.
    pub fn with_month(mut self, month: u32) -> Self {
        self.month = Some(month);
        self
    }

    /// Set the day of the month.
    pub fn with_day(mut self, day: u32) -> Self {
        self.day = Some(day);
        self
    }

    /// Set the calendar system the date is in.
    pub fn with_calendar_scale(mut self, scale: impl Into<String>) -> Self {
        self.calendar_scale = Some(scale.into());
        self
    }
}

/// JSContact Anniversary (RFC 9553 §2.8.1): one memorable date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
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
    /// Extra properties the server sent that this struct does not name.
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
    /// Create an anniversary of the given kind and no date set.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ..Self::default()
        }
    }

    /// Set the date as a raw JSON value.
    pub fn with_date(mut self, date: Value) -> Self {
        self.date = Some(date);
        self
    }

    /// Set the date from its year, month and day parts, any of which may be unknown.
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

    /// A free-text label for the anniversary, if the server sent one.
    pub fn label(&self) -> Option<&str> {
        self.extra.get("label").and_then(|v| v.as_str())
    }

    /// Set a free-text label for the anniversary.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.extra
            .insert("label".to_owned(), Value::String(label.into()));
        self
    }

    /// The preference order among anniversaries of the same kind, if the server sent one.
    pub fn pref(&self) -> Option<u32> {
        self.extra
            .get("pref")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    }

    /// Set the preference order among anniversaries of the same kind.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.extra
            .insert("pref".to_owned(), Value::Number(pref.into()));
        self
    }

    /// Set the place associated with the anniversary.
    pub fn with_place(mut self, place: Address) -> Self {
        self.extra.insert(
            "place".to_owned(),
            serde_json::to_value(place).unwrap_or(Value::Null),
        );
        self
    }

    /// The date as a [`PartialDate`], parsed from either form `date` can take.
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
#[non_exhaustive]
pub struct Link {
    /// Where the resource is. Mandatory per RFC 9553 §2.6.3, and the only
    /// part of a link a `URL` line has room for.
    #[serde(default)]
    pub uri: String,
    /// What kind of link it is: `contact` (a URI for getting in touch) is
    /// the defined kind in RFC 9553 §2.6.3, with no default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The contexts the link applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The media type of the resource at the link, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The preference order among links, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// A free-text label for the link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Link {
    /// Create a link with the given URI and no other properties set.
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            ..Self::default()
        }
    }

    /// Set what kind of link it is.
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Set the preference order among links.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// Set a free-text label for the link.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the contexts the link applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    /// Set the media type of the resource at the link.
    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }
}

/// JSContact Calendar (RFC 9553 §2.4.1): one calendaring resource of the
/// contact: a calendar of theirs, or the free/busy data drawn from one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Calendar {
    /// What the resource is: `calendar` or `freeBusy` per RFC 9553 §2.4.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Where the resource is. Mandatory per RFC 9553 §2.4.1, and the only
    /// part of a calendar either line has room for.
    #[serde(default)]
    pub uri: String,
    /// The contexts the calendar resource applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The media type of the resource at the URI, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The preference order among calendar resources, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// A free-text label for the calendar resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Calendar {
    /// Create a calendar resource with the given URI and no other properties set.
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            ..Self::default()
        }
    }

    /// Set what the resource is.
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Set the preference order among calendar resources.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// Set a free-text label for the calendar resource.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// JSContact SchedulingAddress (RFC 9553 §2.4.2): one scheduling address of the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SchedulingAddress {
    /// The JSContact object type. Always `"SchedulingAddress"`.
    #[serde(
        rename = "@type",
        default = "default_scheduling_address_type",
        serialize_with = "serialize_scheduling_address_type",
        deserialize_with = "deserialize_scheduling_address_type"
    )]
    pub scheduling_address_type: Option<String>,
    /// The scheduling address itself, typically a `mailto:` URI.
    #[serde(default)]
    pub uri: String,
    /// The contexts the scheduling address applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The preference order among scheduling addresses, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// A free-text label for the scheduling address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Extra properties the server sent that this struct does not name.
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
    /// Create a scheduling address with the given URI and no other properties set.
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            ..Self::default()
        }
    }

    /// Set the JSContact object type.
    pub fn with_type(mut self, address_type: impl Into<String>) -> Self {
        self.scheduling_address_type = Some(address_type.into());
        self
    }

    /// Set the preference order among scheduling addresses.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// Set a free-text label for the scheduling address.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// JSContact Media (RFC 9553 §2.6.4): one media resource the card carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
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
    /// The contexts the media resource applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The preference order among media resources of the same kind, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// A free-text label for the media resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Media {
    /// Create a media resource with the given URI and no other properties set.
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            ..Self::default()
        }
    }

    /// Set what the resource is.
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Set the media type of the resource.
    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    /// Set the preference order among media resources of the same kind.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// Set a free-text label for the media resource.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the contexts the media resource applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }
}

/// JSContact OnlineService (RFC 9553 §2.3.2): the contact as one online
/// service or protocol knows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
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
    /// The contexts the online service applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The preference order among online services, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// A free-text label for the online service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl OnlineService {
    /// Create an empty online service entry with no properties set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the name of the service or protocol.
    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    /// Set the name the contact is known by at the service.
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Set the contact's identifier at the service as a URI.
    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }

    /// Set the contexts the online service applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    /// Set the preference order among online services.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// Set a free-text label for the online service.
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
#[non_exhaustive]
pub struct Relation {
    /// Relation types the entity stands in (RFC 9553 §1.4.3 Set mapping types
    /// such as `spouse`, `child`, `colleague` to boolean values).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<BTreeMap<String, Value>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Relation {
    /// Create an empty relation with no relation types set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the relation types the entity stands in.
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
#[non_exhaustive]
pub struct ContactCardQueryFilter {
    /// Match only cards in this address book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_address_book: Option<Id>,
    /// Match only the card with this uid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// Match only cards whose name, nicknames, organization, email, phone,
    /// online service, note or address contain this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Match only cards whose name or nicknames contain this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Match only cards with an email address containing this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Match only cards with a phone number containing this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    /// Match only cards with an online service identifier containing this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_service: Option<String>,
    /// Match only cards with a postal address containing this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Match only cards of this [`card_kind`] value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl ContactCardQueryFilter {
    /// Create a filter that matches every card in the given address book.
    pub fn in_address_book(address_book_id: impl Into<Id>) -> Self {
        Self {
            in_address_book: Some(address_book_id.into()),
            ..Self::default()
        }
    }

    /// Match only the card with the given uid.
    pub fn uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    /// Match only cards containing the given text.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Match only cards whose name or nicknames contain the given text.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Match only cards with an email address containing the given text.
    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Match only cards with a phone number containing the given text.
    pub fn phone(mut self, phone: impl Into<String>) -> Self {
        self.phone = Some(phone.into());
        self
    }

    /// Match only cards with an online service identifier containing the given text.
    pub fn online_service(mut self, service: impl Into<String>) -> Self {
        self.online_service = Some(service.into());
        self
    }

    /// Match only cards with a postal address containing the given text.
    pub fn address(mut self, address: impl Into<String>) -> Self {
        self.address = Some(address.into());
        self
    }

    /// Match only cards of the given kind.
    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }
}

/// The `SetError` type RFC 9610 §2.4 adds for `AddressBook/set`.
pub mod address_book_set_error {
    /// Returned when an `AddressBook/set destroy` targets one that still has cards.
    pub const HAS_CARD: &str = "addressBookHasCard";
    /// Returned when an `AddressBook/set destroy` targets one with contents
    /// and `onDestroyRemoveContents` was not set.
    pub const ADDRESS_BOOK_HAS_CONTENTS: &str = "addressBookHasContents";
}

/// Standard RFC 9553 §2.8.1 anniversary kinds.
pub mod anniversary_kind {
    /// A birth date.
    pub const BIRTH: &str = "birth";
    /// A date of death.
    pub const DEATH: &str = "death";
    /// A wedding date.
    pub const WEDDING: &str = "wedding";
}

/// Standard RFC 9553 §2.2.4 job title kinds.
pub mod title_kind {
    /// A job title.
    pub const TITLE: &str = "title";
    /// A role the contact plays.
    pub const ROLE: &str = "role";
}

/// Standard RFC 9553 §2.4.1 calendar resource kinds.
pub mod calendar_kind {
    /// A calendar of the contact's.
    pub const CALENDAR: &str = "calendar";
    /// Free/busy data drawn from a calendar of the contact's.
    pub const FREE_BUSY: &str = "freeBusy";
}

/// Standard RFC 9553 §2.6.4 media resource kinds.
pub mod media_kind {
    /// A photo of the contact.
    pub const PHOTO: &str = "photo";
    /// A sound clip, such as a name pronunciation.
    pub const SOUND: &str = "sound";
    /// A logo associated with the contact.
    pub const LOGO: &str = "logo";
}

/// Standard RFC 9553 §2.6.3 link kinds.
pub mod link_kind {
    /// A URI for getting in touch with the contact.
    pub const CONTACT: &str = "contact";
}

/// Standard RFC 9553 §2.2.1 name component kinds.
pub mod name_component_kind {
    /// A prefix such as "Mr." or "Dr.".
    pub const PREFIX: &str = "prefix";
    /// A title such as "Dr." when it is itself a component, not a prefix.
    pub const TITLE: &str = "title";
    /// The given (first) name.
    pub const GIVEN: &str = "given";
    /// A second given name.
    pub const GIVEN2: &str = "given2";
    /// A middle name.
    pub const MIDDLE: &str = "middle";
    /// The surname (last name).
    pub const SURNAME: &str = "surname";
    /// A second surname.
    pub const SURNAME2: &str = "surname2";
    /// A suffix such as "Jr." or "III".
    pub const SUFFIX: &str = "suffix";
    /// A generation marker such as "Sr." or "Jr.".
    pub const GENERATION: &str = "generation";
    /// A credential such as "PhD" or "MD".
    pub const CREDENTIAL: &str = "credential";
    /// A literal separator between other components, for display order.
    pub const SEPARATOR: &str = "separator";
}

/// Standard RFC 9553 §2.5.1 address component kinds.
pub mod address_component_kind {
    /// The street name.
    pub const NAME: &str = "name";
    /// The unit number within a building.
    pub const UNIT: &str = "unit";
    /// The floor within a building.
    pub const FLOOR: &str = "floor";
    /// The street number.
    pub const STREET: &str = "street";
    /// The apartment number, in the RFC's misspelled constant name.
    pub const APPARTMENT: &str = "appartment";
    /// The apartment number.
    pub const APARTMENT: &str = "apartment";
    /// The room number.
    pub const ROOM: &str = "room";
    /// The building name or number.
    pub const BUILDING: &str = "building";
    /// The city or town.
    pub const LOCALITY: &str = "locality";
    /// The district or neighbourhood.
    pub const DISTRICT: &str = "district";
    /// A subdivision of the district.
    pub const SUBDISTRICT: &str = "subdistrict";
    /// The state, province or region.
    pub const REGION: &str = "region";
    /// The postal or ZIP code.
    pub const POSTCODE: &str = "postcode";
    /// The post office box.
    pub const POST_OFFICE_BOX: &str = "postOfficeBox";
    /// The country name.
    pub const COUNTRY: &str = "country";
    /// The ISO 3166-1 country code.
    pub const COUNTRY_CODE: &str = "countryCode";
}

/// JSContact CryptoKey (RFC 9553 §2.6.1): a cryptographic key for the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CryptoKey {
    /// What kind of key or certificate this is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Where the key is. Mandatory per RFC 9553 §2.6.1, and the only part of
    /// a `KEY` line it has room for.
    #[serde(default)]
    pub uri: String,
    /// The media type of the key, if not stated by the URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The preference order among crypto keys, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CryptoKey {
    /// Create a crypto key of the given kind at the given URI.
    pub fn new(kind: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            kind: Some(kind.into()),
            uri: uri.into(),
            media_type: None,
            pref: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the media type of the key.
    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    /// Set the preference order among crypto keys.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }
}

/// Standard RFC 9553 §2.6.1 crypto key kinds.
pub mod crypto_key_kind {
    /// A public key.
    pub const KEY: &str = "key";
    /// A certificate.
    pub const CERT: &str = "cert";
}

/// JSContact Directory (RFC 9553 §2.6.2): a directory service for the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Directory {
    /// The JSContact object type. Always `"Directory"`.
    #[serde(
        rename = "@type",
        default = "default_directory_type",
        serialize_with = "serialize_directory_type",
        deserialize_with = "deserialize_directory_type"
    )]
    pub directory_type: Option<String>,
    /// What kind of directory resource this is: `directory` or `entry`, one
    /// of the [`directory_kind`] values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Where the directory resource is. Mandatory per RFC 9553 §2.6.2.
    #[serde(default)]
    pub uri: String,
    /// The media type of the resource, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The position to list the contact at in a directory listing, if a
    /// single entry represents more than one person.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_u32"
    )]
    pub list_as: Option<u32>,
    /// The preference order among directory resources, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// A free-text label for the directory resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The contexts the directory resource applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// Extra properties the server sent that this struct does not name.
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
    /// Create a directory resource of the given kind at the given URI.
    pub fn new(kind: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            kind: Some(kind.into()),
            uri: uri.into(),
            ..Self::default()
        }
    }

    /// Set the JSContact object type.
    pub fn with_type(mut self, directory_type: impl Into<String>) -> Self {
        self.directory_type = Some(directory_type.into());
        self
    }

    /// Set the media type of the resource.
    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    /// Set the position to list the contact at in a directory listing.
    pub fn with_list_as(mut self, list_as: u32) -> Self {
        self.list_as = Some(list_as);
        self
    }

    /// Set the preference order among directory resources.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }

    /// Set a free-text label for the directory resource.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the contexts the directory resource applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }
}

/// Standard RFC 9553 §2.6.2 directory kinds.
pub mod directory_kind {
    /// A general directory for the contact.
    pub const DIRECTORY: &str = "directory";
    /// A directory entry specific to the contact.
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
#[non_exhaustive]
pub struct PersonalInfo {
    /// One of the [`personal_info_kind`] values.
    #[serde(default)]
    pub kind: String,
    /// The text of the personal information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// For `expertise`, the level: `low`, `medium` or `high` (RFC 9553 §2.8.4).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub personal_info_type: Option<String>,
    /// The position to list this entry at among others of the same kind.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_u32"
    )]
    pub list_as: Option<u32>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PersonalInfo {
    /// Create a personal info entry of the given kind and text.
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: Some(value.into()),
            personal_info_type: None,
            list_as: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the expertise level.
    pub fn with_type(mut self, info_type: impl Into<String>) -> Self {
        self.personal_info_type = Some(info_type.into());
        self
    }

    /// Set the position to list this entry at among others of the same kind.
    pub fn with_list_as(mut self, list_as: u32) -> Self {
        self.list_as = Some(list_as);
        self
    }
}

/// Standard RFC 9553 §2.8.4 personal info kinds.
pub mod personal_info_kind {
    /// The contact's gender identity.
    pub const GENDER: &str = "gender";
    /// An area of expertise.
    pub const EXPERTISE: &str = "expertise";
    /// A hobby.
    pub const HOBBY: &str = "hobby";
    /// An interest.
    pub const INTEREST: &str = "interest";
}

/// JSContact CardGroup (RFC 9553 §2.1.2): a group of contact cards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CardGroup {
    /// The server-assigned id. Absent on a `ContactCard/set create` argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The JSContact object type. Always `"CardGroup"`.
    #[serde(
        rename = "@type",
        default = "default_card_group_type",
        serialize_with = "serialize_card_group_type",
        deserialize_with = "deserialize_card_group_type"
    )]
    pub card_type: Option<String>,
    /// The user-visible name of the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The member cards, keyed by uid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<BTreeMap<String, bool>>,
    /// Extra properties the server sent that this struct does not name.
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
    /// Create a group with the given name and no members.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the JSContact object type.
    pub fn with_type(mut self, card_type: impl Into<String>) -> Self {
        self.card_type = Some(card_type.into());
        self
    }

    /// Set the member cards, keyed by uid.
    pub fn with_members(mut self, members: BTreeMap<String, bool>) -> Self {
        self.members = Some(members);
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Contacts capability properties (RFC 9610 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContactsCapability {
    /// The largest total size, in octets, of attachments a single card may carry.
    #[serde(default)]
    pub max_size_attachments_per_card: u64,
    /// The largest number of cards the server lets a single `ContactCard/set` change.
    #[serde(default)]
    pub max_number_of_cards_in_set: u64,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ContactsCapability {
    /// Create an empty capability with no limits set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the largest total size of attachments a single card may carry.
    pub fn with_max_size_attachments_per_card(mut self, max: u64) -> Self {
        self.max_size_attachments_per_card = max;
        self
    }

    /// Set the largest number of cards a single `ContactCard/set` changes.
    pub fn with_max_number_of_cards_in_set(mut self, max: u64) -> Self {
        self.max_number_of_cards_in_set = max;
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard RFC 9553 §2.1.1 JSContact card kinds.
pub mod card_kind {
    /// A single person.
    pub const INDIVIDUAL: &str = "individual";
    /// A group of individuals or entities.
    pub const GROUP: &str = "group";
    /// An organisation.
    pub const ORG: &str = "org";
    /// A named location.
    pub const LOCATION: &str = "location";
    /// A device.
    pub const DEVICE: &str = "device";
    /// A software application.
    pub const APPLICATION: &str = "application";
}

/// Standard RFC 9553 §2.2.5 grammatical genders.
pub mod grammatical_gender {
    /// Animate gender.
    pub const ANIMATE: &str = "animate";
    /// Inanimate gender.
    pub const INANIMATE: &str = "inanimate";
    /// Feminine gender.
    pub const FEMININE: &str = "feminine";
    /// Masculine gender.
    pub const MASCULINE: &str = "masculine";
    /// Neuter gender.
    pub const NEUTER: &str = "neuter";
    /// Common gender, used where masculine and feminine are not distinguished.
    pub const COMMON: &str = "common";
}

/// JSContact Pronouns (RFC 9553 §2.2.4): pronouns to use when addressing or speaking about the contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Pronouns {
    /// The JSContact object type. Always `"Pronouns"`.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub pronouns_type: Option<String>,
    /// The pronouns themselves, such as "they/them".
    #[serde(default)]
    pub pronouns: String,
    /// The contexts the pronouns apply to, keyed by context name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<BTreeMap<String, bool>>,
    /// The preference order among pronoun entries, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Pronouns {
    /// Create a pronouns entry with the given text and no other properties set.
    pub fn new(pronouns: impl Into<String>) -> Self {
        Self {
            pronouns: pronouns.into(),
            ..Self::default()
        }
    }

    /// Set the preference order among pronoun entries.
    pub fn with_pref(mut self, pref: u32) -> Self {
        self.pref = Some(pref);
        self
    }
}

/// JSContact pronouns value, supporting either an RFC 9553 §2.2.4 map of
/// `Pronouns` or a shorthand plain string.
/// Stays exhaustive: an untagged two-case type fixed by the RFC, not a type a server extends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PronounsValue {
    /// The full RFC 9553 §2.2.4 form: one or more `Pronouns` entries, keyed by id.
    Map(BTreeMap<String, Pronouns>),
    /// The shorthand form: a plain pronouns string with no other properties.
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
#[non_exhaustive]
pub struct SpeakToAs {
    /// The grammatical gender to use, one of the [`grammatical_gender`] values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammatical_gender: Option<String>,
    /// The pronouns to use when speaking about the contact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<PronounsValue>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SpeakToAs {
    /// Create an empty SpeakToAs with no properties set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the grammatical gender to use.
    pub fn with_grammatical_gender(mut self, gender: impl Into<String>) -> Self {
        self.grammatical_gender = Some(gender.into());
        self
    }

    /// Set the pronouns to use, as a full map or a shorthand string.
    pub fn with_pronouns(mut self, pronouns: impl Into<PronounsValue>) -> Self {
        self.pronouns = Some(pronouns.into());
        self
    }

    /// Add one pronouns entry, keyed by an id of the caller's choosing.
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
#[non_exhaustive]
pub struct LanguagePref {
    /// The preferred language, as a language tag.
    #[serde(default)]
    pub language: String,
    /// The contexts the preference applies to, such as `private` or `work`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The preference order among languages, 1 (most preferred) to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pref: Option<u32>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl LanguagePref {
    /// Create a language preference with the given language and no other properties set.
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
            ..Self::default()
        }
    }

    /// Set the contexts the preference applies to.
    pub fn with_contexts(mut self, contexts: Value) -> Self {
        self.contexts = Some(contexts);
        self
    }

    /// Set the preference order among languages.
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
#[non_exhaustive]
pub struct ContactCardParseRequest {
    /// The id of the account the blobs belong to.
    pub account_id: Id,
    /// The ids of the blobs to parse as contact cards.
    pub blob_ids: Vec<Id>,
    /// The properties to return for each parsed card. All properties if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
}

impl ContactCardParseRequest {
    /// Create a request for the given account and blob ids.
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

    /// Set the properties to return for each parsed card.
    pub fn properties(mut self, properties: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }
}

/// `ContactCard/parse` response (RFC 9610 §3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContactCardParseResponse {
    /// The id of the account the blobs belong to.
    pub account_id: Id,
    /// The successfully parsed cards, keyed by the blob id they came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<BTreeMap<Id, ContactCard>>,
    /// The ids of blobs that could not be parsed as a contact card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_parsable: Option<Vec<Id>>,
    /// The ids of blobs that do not exist or the account cannot read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_found: Option<Vec<Id>>,
}

impl ContactCardParseResponse {
    /// Create an empty response for the given account with nothing parsed yet.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            parsed: None,
            not_parsable: None,
            not_found: None,
        }
    }

    /// Set the successfully parsed cards.
    pub fn with_parsed(mut self, parsed: BTreeMap<Id, ContactCard>) -> Self {
        self.parsed = Some(parsed);
        self
    }

    /// Set the ids of blobs that could not be parsed.
    pub fn with_not_parsable(
        mut self,
        not_parsable: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.not_parsable = Some(not_parsable.into_iter().map(Into::into).collect());
        self
    }

    /// Set the ids of blobs that do not exist or the account cannot read.
    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = Some(not_found.into_iter().map(Into::into).collect());
        self
    }
}

/// The `SetError` types RFC 9610 §3.2 adds for `ContactCard/set`.
pub mod contact_card_set_error {
    /// Returned when a `blobId` on a `Media` property does not exist or the account cannot read it.
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
}
