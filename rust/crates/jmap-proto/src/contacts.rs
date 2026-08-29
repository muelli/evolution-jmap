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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_with: Option<BTreeMap<Id, Option<AddressBookRights>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<AddressBookRights>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The permissions the user has for an address book (RFC 9610 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddressBookRights {
    #[serde(default)]
    pub may_read_items: bool,
    #[serde(default)]
    pub may_add_items: bool,
    #[serde(default)]
    pub may_modify_items: bool,
    #[serde(default)]
    pub may_remove_items: bool,
    #[serde(default)]
    pub may_delete: bool,
    #[serde(default)]
    pub may_rename: bool,
    #[serde(default)]
    pub may_admin: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A contact card (RFC 9610 §3): JSContact Card plus JMAP `id` and
/// `addressBookIds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContactCard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_book_ids: Option<BTreeMap<Id, bool>>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub card_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Name>,

    /// The names the contact is also known by (RFC 9553 §2.2.2), keyed like
    /// the other JSContact maps. vCard states each on a `NICKNAME` line of
    /// its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nicknames: Option<BTreeMap<String, Nickname>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emails: Option<BTreeMap<String, ContactEmail>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phones: Option<BTreeMap<String, ContactPhone>>,
    /// The organisations the contact belongs to (RFC 9553 §2.2.3), keyed by
    /// an id of whoever wrote them — the employer and the department within
    /// it, which vCard states in one `ORG` line each.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizations: Option<BTreeMap<String, Organization>>,
    /// The job titles the contact holds and the roles it plays (RFC 9553
    /// §2.2.4), keyed like the other JSContact maps. vCard states each on a
    /// `TITLE` or a `ROLE` line, depending on the entry's `kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titles: Option<BTreeMap<String, Title>>,
    /// The postal addresses the contact can be reached at (RFC 9553 §2.5.1),
    /// keyed like the other JSContact maps. vCard states each as one
    /// structured `ADR` line, whose seven fields have room for only some of
    /// the component kinds RFC 9553 allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<BTreeMap<String, Address>>,
    /// The free-text notes kept about the contact (RFC 9553 §2.8.3), keyed
    /// like the other JSContact maps. vCard states each on a `NOTE` line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<BTreeMap<String, Note>>,
    /// The memorable dates of the contact (RFC 9553 §2.8.1), keyed like the
    /// other JSContact maps. vCard states the birthday on a `BDAY` line and
    /// the wedding day on the line Evolution reads as the anniversary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anniversaries: Option<BTreeMap<String, Anniversary>>,
    /// The resources the contact points at (RFC 9553 §2.6.3), keyed like the
    /// other JSContact maps. vCard states each on a `URL` line, of which
    /// Evolution shows the first as the contact's home page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, Link>>,
    /// The calendaring resources of the contact (RFC 9553 §2.4.1), keyed like
    /// the other JSContact maps. vCard states the calendar itself on a
    /// `CALURI` line and the free/busy data drawn from it on an `FBURL`,
    /// which are the two Evolution shows as the contact's calendar addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendars: Option<BTreeMap<String, Calendar>>,
    /// The media the card carries (RFC 9553 §2.6.4), keyed like the other
    /// JSContact maps. vCard states a photo on a `PHOTO` line, which is what
    /// Evolution shows as the contact's picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<BTreeMap<String, Media>>,
    /// The online services the contact is reachable on (RFC 9553 §2.3.2),
    /// keyed like the other JSContact maps. vCard states each on the `X-` line
    /// EDS keeps that service's handles on — `X-JABBER`, `X-MATRIX`, … — which
    /// is what Evolution shows as an instant-messaging address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_services: Option<BTreeMap<String, OnlineService>>,
    /// The tags the contact is filed under (RFC 9553 §2.8.2), an RFC 9553
    /// §1.4.3 Set: the keys are the keywords and every value is `true`. vCard
    /// states the whole set on one `CATEGORIES` line, which is what Evolution's
    /// Categories field shows.
    ///
    /// The values are left as JSON rather than as `bool` for the reason
    /// [`crate::calendars::CalendarEvent::keywords`] gives: a server that puts
    /// something else there must not cost the user the whole address book, so
    /// the odd entry stays visible as itself and the vCard mapping refuses to
    /// write the property back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, Value>>,
    /// The other entities this one relates to (RFC 9553 §2.1.8), keyed — unlike
    /// every other JSContact map here — by *who the related entity is* rather
    /// than by an id of whoever wrote the entry. vCard 3.0 states none of the
    /// relations; the one Evolution has a field for is the spouse, on the `X-`
    /// line EDS keeps `E_CONTACT_SPOUSE` on.
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

impl ContactCard {
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One name component: kind is `given`, `surname`, `title`, …
///
/// Like [`AddressComponent`] this keeps what it does not model — a component
/// carries a `phonetic` spelling besides its value — because the save path
/// writes the component list back whole, so a member dropped on the way in is
/// a member deleted on the way out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameComponent {
    pub kind: String,
    pub value: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl NameComponent {
    pub fn new(kind: &str, value: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            value: value.to_owned(),
            extra: BTreeMap::new(),
        }
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

/// JSContact Organization (RFC 9553 §2.2.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    /// The organisation's own name, which the `ORG` value states first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The units within it, outermost first — the departments the `ORG` value
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
}

/// JSContact Title (RFC 9553 §2.2.4): a job title the contact holds, or a
/// role it plays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Title {
    #[serde(default)]
    pub name: String,
    /// `title` or `role`. Absent means `title`, RFC 9553 §2.2.4's default,
    /// which this side leaves unsaid rather than writing out — so a card
    /// that never named a kind is not rewritten by a save that changed
    /// nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
    pub fn new(kind: &str, value: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            value: value.to_owned(),
            extra: BTreeMap::new(),
        }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact Anniversary (RFC 9553 §2.8.1): one memorable date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Anniversary {
    /// `birth`, `death` or `wedding`. Mandatory per RFC 9553 §2.8.1, which
    /// gives it no default — an entry that names none says nothing about
    /// what its date is the date *of*, and is treated here as a kind no
    /// vCard property states.
    #[serde(default)]
    pub kind: String,
    /// The date itself, kept as it arrived rather than parsed into a shape
    /// of our own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact Link (RFC 9553 §2.6.3): one resource the contact points at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// Where the resource is. Mandatory per RFC 9553 §2.6.3, and the only
    /// part of a link a `URL` line has room for.
    #[serde(default)]
    pub uri: String,
    /// What kind of link it is — `contact`, a URI for getting in touch, is
    /// the one kind RFC 9553 §2.6.3 defines, and it has no default.
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

/// JSContact Calendar (RFC 9553 §2.4.1): one calendaring resource of the
/// contact — a calendar of theirs, or the free/busy data drawn from one.
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

/// JSContact OnlineService (RFC 9553 §2.3.2): the contact as one online
/// service or protocol knows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OnlineService {
    /// The name of the service or protocol — `Jabber`, `Matrix`, `Skype`.
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

/// JSContact Relation (RFC 9553 §2.1.8): how one related entity relates to
/// the contact.
///
/// The entity itself is the map key over in [`ContactCard::related_to`] — RFC
/// 9553 §2.1.8 makes it the related Card's `uid`, and RFC 9555 §2.9.5 puts free
/// text there for a vCard `RELATED;VALUE=text`, which is the case that holds a
/// name a user could read. So a Relation object carries no identity of its own
/// and there is nothing else on it to model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    /// The relation types the entity stands in, an RFC 9553 §1.4.3 Set: the
    /// keys are the types — `spouse`, `child`, `colleague`, … — and every value
    /// is `true`. An empty set, which RFC 9555 §2.9.5's example produces from a
    /// `RELATED` line carrying no `TYPE`, means the relation is unspecified.
    ///
    /// The values are left as JSON rather than as `bool` for the reason
    /// [`ContactCard::keywords`] gives: a whole `ContactCard/get` response is
    /// deserialized at once, so a server answering `{"spouse": 1}` for one card
    /// must not cost the user the whole address book. The odd entry stays
    /// visible as itself and the vCard mapping refuses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
    pub const GIVEN: &str = "given";
    pub const MIDDLE: &str = "middle";
    pub const SURNAME: &str = "surname";
    pub const SUFFIX: &str = "suffix";
}

/// Standard RFC 9553 §2.5.1 address component kinds.
pub mod address_component_kind {
    pub const NAME: &str = "name";
    pub const UNIT: &str = "unit";
    pub const FLOOR: &str = "floor";
    pub const STREET: &str = "street";
    pub const APPARTMENT: &str = "appartment";
    pub const ROOM: &str = "room";
    pub const BUILDING: &str = "building";
    pub const LOCALITY: &str = "locality";
    pub const REGION: &str = "region";
    pub const POSTCODE: &str = "postcode";
    pub const COUNTRY: &str = "country";
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

/// Standard RFC 9553 §2.6.1 crypto key kinds.
pub mod crypto_key_kind {
    pub const KEY: &str = "key";
    pub const CERT: &str = "cert";
}

/// JSContact Directory (RFC 9553 §2.6.2): a directory service for the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Directory {
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

/// Standard RFC 9553 §2.6.2 directory kinds.
pub mod directory_kind {
    pub const DIRECTORY: &str = "directory";
}

/// JSContact PersonalInfo (RFC 9553 §2.8.4): personal information about the contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PersonalInfo {
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_as: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Standard RFC 9553 §2.8.4 personal info kinds.
pub mod personal_info_kind {
    pub const GENDER: &str = "gender";
    pub const EXPERTISE: &str = "expertise";
    pub const HOBBY: &str = "hobby";
    pub const INTEREST: &str = "interest";
}

/// JSContact CardGroup (RFC 9553 §2.1.2): a group of contact cards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub card_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<BTreeMap<String, bool>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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

/// JSContact SpeakToAs (RFC 9553 §2.2.5): how to address the contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpeakToAs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammatical_gender: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
                    pref: Some(value as u32),
                    ..LanguagePref::default()
                })
            }

            fn visit_i64<E>(self, value: i64) -> Result<LanguagePref, E>
            where
                E: serde::de::Error,
            {
                Ok(LanguagePref {
                    pref: Some(value as u32),
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
