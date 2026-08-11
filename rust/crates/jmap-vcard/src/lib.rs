// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact ↔ vCard for the Evolution address book backend.
//!
//! `EBookMetaBackend` speaks `EContact`, which is built from and rendered to
//! vCard text; JMAP speaks JSContact Cards ([RFC 9553]) wrapped in
//! `ContactCard` ([RFC 9610]). This crate is the translation between the
//! two, and nothing else — it has no dependency on GLib or the Evolution
//! headers, so the mapping stays testable everywhere the workspace builds.
//!
//! The mapped property set is the minimal useful one: UID, FN, N, NICKNAME,
//! EMAIL, TEL, ADR, LABEL, ORG, TITLE, ROLE, NOTE, BDAY, URL, CALURI, FBURL,
//! PHOTO, CATEGORIES
//! and the `X-` lines EDS keeps instant-messaging handles on. See [`contact`]
//! for what that costs and why it is safe.
//!
//! [RFC 9553]: https://www.rfc-editor.org/rfc/rfc9553
//! [RFC 9610]: https://www.rfc-editor.org/rfc/rfc9610

pub mod contact;
pub mod error;
pub mod syntax;

pub use contact::{
    address_label, anniversary_date, card_to_vcard, maps_address_component, maps_context,
    maps_name_component, maps_phone_feature, online_service_handle, online_service_uri,
    restore_address_components, restore_name_components, same_photo, same_service,
    states_a_point_in_time, states_address, states_anniversary, states_calendar, states_email,
    states_keyword, states_link, states_media, states_nickname, states_note, states_online_service,
    states_organization, states_phone, states_title, title_kind, vcard_to_card,
};
pub use error::VCardError;
