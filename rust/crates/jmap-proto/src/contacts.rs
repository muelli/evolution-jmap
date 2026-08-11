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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameComponent {
    pub kind: String,
    pub value: String,
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
/// Only the two members a vCard `ADR` line can carry are modeled. `full`,
/// `coordinates`, `countryCode`, `timeZone`, `pref` and the rest ride in
/// [`Self::extra`] — where the save path can see the members it is refusing
/// to touch, which is the whole reason this is a struct and not a `Value`.
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One part of an [`Address`]: `kind` is `name` (the street), `locality`,
/// `postcode`, `floor`, …
///
/// Unlike [`NameComponent`], this keeps what it does not model: a component
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
