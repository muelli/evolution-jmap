// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `contact-lists` capability, without which Evolution's Contact List
//! Editor disables its name field and member controls entirely
//! (`e_contact_list_editor_set_client`, `e_client_check_capability`).

use jmap_backend_book::capabilities::{CONTACT_LISTS, capabilities};

fn split(caps: &str) -> Vec<&str> {
    caps.split(',').filter(|cap| !cap.is_empty()).collect()
}

#[test]
fn the_capability_name_is_the_one_contact_list_editor_checks() {
    // `e_client_check_capability` matches this exact string. A typo here
    // leaves the list editor permanently disabled, not just missing a flag.
    assert_eq!(CONTACT_LISTS, "contact-lists");
}

#[test]
fn the_inherited_capabilities_gain_contact_lists() {
    let caps = capabilities(Some("refresh-supported,bulk-adds"));
    assert_eq!(
        split(&caps),
        ["refresh-supported", "bulk-adds", CONTACT_LISTS]
    );
}

#[test]
fn with_nothing_inherited_the_answer_has_no_empty_entry() {
    assert_eq!(split(&capabilities(None)), [CONTACT_LISTS]);
    assert_eq!(capabilities(Some("")), capabilities(None));
}

#[test]
fn contact_lists_already_inherited_is_not_listed_twice() {
    let caps = capabilities(Some("contact-lists,refresh-supported"));
    assert_eq!(
        split(&caps)
            .iter()
            .filter(|cap| **cap == CONTACT_LISTS)
            .count(),
        1
    );
    assert!(split(&caps).contains(&"refresh-supported"));
}
