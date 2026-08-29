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
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
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
///
/// `contexts` and `pref` are not modeled: RFC 2426 §3.1.3's `NICKNAME` takes
/// none of the parameters that could state either — it has no `TYPE` — and
/// Evolution's contact editor shows a nickname without a context or a
/// ranking. Both therefore ride in [`Self::extra`], where the save path can
/// see the members it is refusing to touch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Nickname {
    /// The nickname itself. Mandatory per RFC 9553 §2.2.2, and the only part
    /// of it a `NICKNAME` line has room for.
    #[serde(default)]
    pub name: String,
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
///
/// `sortAs` and `contexts` are not modeled: vCard 3.0's `ORG` (RFC 2426
/// §3.5.5) has no component and no parameter for either, so they ride in
/// [`Self::extra`] — where the save path can see them and leave them alone,
/// which is the whole reason this is a struct and not a `Value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    /// The organisation's own name, which the `ORG` value states first.
    ///
    /// Optional, as RFC 9553 §2.2.3 has it: a card may name only the units,
    /// and answering `Some("")` for that would put an empty employer on the
    /// server where it never had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The units within it, outermost first — the departments the `ORG` value
    /// lists after the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<Vec<OrgUnit>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact OrgUnit (RFC 9553 §2.2.3): one unit of an [`Organization`].
///
/// A unit holds a `sortAs` besides its name, which is why the vCard mapping
/// cannot rebuild the list from the `ORG` components alone — see the save
/// path, which carries a unit's unmapped members across a rename of its
/// siblings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OrgUnit {
    #[serde(default)]
    pub name: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl OrgUnit {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            extra: BTreeMap::new(),
        }
    }
}

/// JSContact Title (RFC 9553 §2.2.4): a job title the contact holds, or a
/// role it plays.
///
/// `organizationId` — which of the card's `organizations` the title is held
/// at — is not modeled: vCard 3.0's `TITLE` and `ROLE` (RFC 2426 §§3.5.1,
/// 3.5.2) are plain text with no component and no parameter naming an
/// organisation, so it rides in [`Self::extra`], where the save path can see
/// the member it is refusing to touch.
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact Address (RFC 9553 §2.5.1): one postal address.
///
/// Only the three members a vCard can carry are modeled — the `ADR` line's
/// components and its `TYPE`, and the `LABEL` line's text. `coordinates`,
/// `countryCode`, `timeZone`, `pref` and the rest ride in [`Self::extra`] —
/// where the save path can see the members it is refusing to touch, which is
/// the whole reason this is a struct and not a `Value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    /// The parts the address is built from, each naming what it is.
    ///
    /// RFC 9553 §2.5.1 leaves the order meaningful only when `isOrdered` is
    /// set, so this is a list of named parts rather than a fixed shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<AddressComponent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Value>,
    /// The address written out as it should be printed, line breaks and all.
    ///
    /// RFC 9553 §2.5.1 has this stand on its own — an address may be stated
    /// here and nowhere else, "even if the individual address components are
    /// not known" — which is the same thing RFC 2426 §3.2.2's `LABEL` says,
    /// and what EDS keeps in its three synthetic address-label fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One part of an [`Address`]: `kind` is `name` (the street), `locality`,
/// `postcode`, `floor`, …
///
/// Like [`NameComponent`], this keeps what it does not model: a component
/// carries a `phonetic` spelling besides its value, and the save path writes
/// the component list back whole, so a member dropped on the way in is a
/// member deleted on the way out.
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
///
/// `created` and `author` are not modeled: vCard 3.0's `NOTE` (RFC 2426
/// §3.6.2) is plain text with no component and no parameter for when a note
/// was written or by whom, so both ride in [`Self::extra`] — where the save
/// path can see the members it is refusing to touch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// The note's text. Mandatory per RFC 9553 §2.8.3, and the only part of
    /// a note a `NOTE` line has room for.
    #[serde(default)]
    pub note: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact Anniversary (RFC 9553 §2.8.1): one memorable date.
///
/// `place` — where the birth or the wedding happened — is not modeled: a
/// vCard date line is a date and nothing else, so it rides in [`Self::extra`]
/// where the save path can see the member it is refusing to touch.
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
    ///
    /// RFC 9553 §2.8.1 allows two: a `PartialDate`, which may state only a
    /// year or only a month and day, and a `Timestamp`, which states a point
    /// in time. A vCard line can carry neither shape whole — it states one
    /// calendar day — so the save patches *into* whichever object the server
    /// sent rather than replacing it, and that is only possible while its
    /// unmapped members (`calendarScale`, the time of day) are still here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact Link (RFC 9553 §2.6.3): one resource the contact points at.
///
/// `mediaType`, `contexts`, `pref` and `label` are not modeled: vCard 3.0's
/// `URL` (RFC 2426 §3.6.8) is a bare URI with no parameter for what the
/// resource is, where it is used, how strongly it is preferred or what to call
/// it, so all four ride in [`Self::extra`] — where the save path can see the
/// members it is refusing to touch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// Where the resource is. Mandatory per RFC 9553 §2.6.3, and the only
    /// part of a link a `URL` line has room for.
    #[serde(default)]
    pub uri: String,
    /// What kind of link it is — `contact`, a URI for getting in touch, is
    /// the one kind RFC 9553 §2.6.3 defines, and it has no default.
    ///
    /// A link that names no kind is the plain website vCard 3.0's `URL` means
    /// (RFC 9555 §2.6.3 pairs the two), which is why this is modeled rather
    /// than carried: the mapping has to be able to tell those apart from the
    /// kinds it must leave alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact Calendar (RFC 9553 §2.4.1): one calendaring resource of the
/// contact — a calendar of theirs, or the free/busy data drawn from one.
///
/// `mediaType`, `contexts`, `pref` and `label` are not modeled, for the reason
/// [`Link`]'s are not: the `CALURI` and `FBURL` lines EDS keeps these on are
/// bare URIs with no parameter for what the resource is, where it is used, how
/// strongly it is preferred or what to call it, so all four ride in
/// [`Self::extra`] — where the save path can see the members it is refusing to
/// touch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    /// What the resource is: `calendar` or `freeBusy` per RFC 9553 §2.4.1,
    /// which makes it mandatory and gives it no default — an entry stating
    /// none is malformed, and is modeled as `None` rather than refused so that
    /// one bad entry does not cost the user the whole address book.
    ///
    /// Modeled rather than carried because it is the mapping's filter, as
    /// [`Media`]'s is: it says which of the two lines the URI goes on, and
    /// there is no third line to put an entry that names neither on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Where the resource is. Mandatory per RFC 9553 §2.4.1, and the only
    /// part of a calendar either line has room for.
    #[serde(default)]
    pub uri: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact Media (RFC 9553 §2.6.4): one media resource the card carries.
///
/// `pref` and `label` are not modeled, for the reason [`Nickname`]'s are not:
/// RFC 2426 §3.1.4's `PHOTO` has no parameter for a ranking or for what to call
/// the picture. Both ride in [`Self::extra`], where the save path can see the
/// members it is refusing to touch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    /// What the resource is: `photo`, `sound` or `logo` per RFC 9553 §2.6.4,
    /// which makes it mandatory — an entry stating none is malformed, and is
    /// modeled as `None` rather than refused so that one bad entry does not
    /// cost the user the whole address book.
    ///
    /// Modeled rather than carried because it is the mapping's filter: of the
    /// three kinds, only a photo is the picture Evolution shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Where the resource is. Mandatory per RFC 9553 §2.6.4, and for a picture
    /// the card carries rather than points at, a `data:` URI (RFC 2397).
    #[serde(default)]
    pub uri: String,
    /// The media type of the resource, which RFC 9553 §2.6.4 asks for when the
    /// URI does not state one. Modeled because a `PHOTO` line's `TYPE` is what
    /// tells EDS what the bytes are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// JSContact OnlineService (RFC 9553 §2.3.2): the contact as one online
/// service or protocol knows them.
///
/// `contexts`, `pref` and `label` are not modeled, for the reason
/// [`Nickname`]'s are not. The `X-` line EDS keeps a handle on does take a
/// `TYPE`, but that parameter is the *slot* EDS files the handle in rather than
/// the entry's contexts — a line without one reaches no field the user can see,
/// measured against libebook-contacts 3.52 — so the vCard mapping writes it and
/// reads nothing back off it. All three therefore ride in [`Self::extra`],
/// where the save path can see the members it is refusing to touch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OnlineService {
    /// The name of the service or protocol — `Jabber`, `Matrix`, `Skype`.
    ///
    /// RFC 9553 §2.3.2 lets it be capitalised as the service itself
    /// capitalises it and has two names be equal when they match
    /// case-insensitively, so the mapping compares rather than rewrites the
    /// spelling the server chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// The name the contact is known by at the service. Free text per RFC 9553
    /// §2.3.2, and what the vCard line states: Evolution's instant-messaging
    /// field holds a handle rather than a URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// The contact's identifier at the service as a URI (RFC 9553 §2.3.2
    /// requires RFC 3986 §3).
    ///
    /// Drawn only where the service's scheme is known to state the handle and
    /// nothing besides, which is a short list; elsewhere it is modeled so that
    /// the save path can tell an entry that states one from an entry that does
    /// not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
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
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl ContactCardQueryFilter {
    pub fn in_address_book(address_book_id: impl Into<Id>) -> Self {
        Self {
            in_address_book: Some(address_book_id.into()),
            ..Self::default()
        }
    }
}

/// The `SetError` type RFC 9610 §2.4 adds for `AddressBook/set`.
pub mod address_book_set_error {
    pub const HAS_CARD: &str = "addressBookHasCard";
}
