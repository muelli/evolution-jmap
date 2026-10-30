// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! Map JSContact contacts to Evolution vCard text.
//!
//! Evolution stores contacts as vCard text. JMAP stores contacts as JSContact
//! `ContactCard` values from RFC 9553 and RFC 9610. This crate keeps mapping
//! logic free of GLib. Tests can then cover mapping behavior.
//!
//! See [`contact`] for field-preservation rules.

pub mod contact;
pub mod error;

pub use contact::{
    DataUri, GROUP_KIND, VENDOR_BARE_MEMBERS, VENDOR_LIST_SHOW_ADDRESSES, VENDOR_MEMBER_DETAILS,
    X_EVOLUTION_DEST_CONTACT_UID, X_EVOLUTION_DEST_EMAIL, X_EVOLUTION_DEST_NAME, X_EVOLUTION_LIST,
    X_EVOLUTION_LIST_SHOW_ADDRESSES, address_label, anniversary_date, card_to_vcard, data_uri,
    maps_context, maps_phone_feature, online_service_handle, online_service_uri,
    restore_address_components, restore_name_components, same_photo, same_service,
    states_a_point_in_time, states_address, states_address_component, states_anniversary,
    states_assistant, states_calendar, states_context, states_email, states_file_as,
    states_group_kind, states_keyword, states_link, states_manager, states_media, states_member,
    states_name, states_name_component, states_nickname, states_note,
    states_nothing_but_the_marriage, states_online_service, states_org_unit, states_organization,
    states_phone, states_phone_feature, states_spouse, states_title, title_kind, vcard_to_card,
};
pub use error::VCardError;
