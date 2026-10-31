// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tells Evolution's Contact List Editor this backend supports contact lists.
//!
//! Without the `contact-lists` capability, Evolution disables the editor's
//! name field and member controls for every list, even one with its members
//! already loaded (`e_contact_list_editor_set_client`, checked through
//! `e_client_check_capability`).

use std::ffi::CStr;

/// Static capability indicating the backend supports contact lists.
pub const CONTACT_LISTS: &str = "contact-lists";

/// Backend capabilities property name.
pub const CAPABILITIES_PROPERTY: &CStr = c"capabilities";

/// Returns the inherited capability list with `contact-lists` added.
pub fn capabilities(inherited: Option<&str>) -> String {
    let mut caps: Vec<&str> = inherited
        .unwrap_or_default()
        .split(',')
        .filter(|cap| !cap.is_empty())
        .collect();
    if !caps.contains(&CONTACT_LISTS) {
        caps.push(CONTACT_LISTS);
    }
    caps.join(",")
}
