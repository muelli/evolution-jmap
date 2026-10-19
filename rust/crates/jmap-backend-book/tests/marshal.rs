// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! C boundary tests for address book operations.
//!
//! Verifies list walking as `GSList` and freeing via EDS deallocators.

use std::ffi::CStr;

use eds_sys::{
    E_CONTACT_EMAIL_1, E_CONTACT_FULL_NAME, E_CONTACT_UID, EBookMetaBackendInfo,
    e_book_meta_backend_info_free, e_contact_get_const,
};
use glib_sys::{GSList, g_slist_free_full, g_slist_length, g_slist_nth_data};
use jmap_backend_book::marshal;
use jmap_book_sync::ContactInfo;

fn info(uid: &str, revision: &str, vcard: &str) -> ContactInfo {
    ContactInfo {
        uid: uid.to_owned(),
        revision: revision.to_owned(),
        vcard: vcard.to_owned(),
    }
}

/// Reads a `GSList` node as an `EBookMetaBackendInfo`, the way
/// `e_book_meta_backend_process_changes_sync` does.
unsafe fn nth_info(list: *mut GSList, n: u32) -> (String, String, String, Option<String>) {
    unsafe {
        let node = g_slist_nth_data(list, n).cast::<EBookMetaBackendInfo>();
        assert!(!node.is_null(), "no node {n}");
        let text = |p: *mut i8| CStr::from_ptr(p).to_string_lossy().into_owned();
        (
            text((*node).uid),
            text((*node).revision),
            text((*node).object),
            (!(*node).extra.is_null()).then(|| text((*node).extra)),
        )
    }
}

#[test]
fn an_info_list_carries_one_node_per_contact_in_order() {
    let infos = [
        info("K1", "r1", "BEGIN:VCARD\r\nEND:VCARD\r\n"),
        info("K2", "r2", "BEGIN:VCARD\r\nUID:K2\r\nEND:VCARD\r\n"),
    ];
    let list = marshal::info_list(&infos);

    unsafe {
        assert_eq!(g_slist_length(list), 2);
        assert_eq!(
            nth_info(list, 0),
            (
                "K1".to_owned(),
                "r1".to_owned(),
                "BEGIN:VCARD\r\nEND:VCARD\r\n".to_owned(),
                None
            )
        );
        assert_eq!(nth_info(list, 1).0, "K2");
        g_slist_free_full(list, Some(e_book_meta_backend_info_free));
    }
}

/// EDS reads "no objects" as a NULL list, not as an empty allocation.
#[test]
fn an_empty_info_list_is_null() {
    assert!(marshal::info_list(&[]).is_null());
    assert!(marshal::removed_info_list(&[]).is_null());
}

/// Removals are represented as `EBookMetaBackendInfo` nodes rather than bare strings.
#[test]
fn removals_are_infos_and_not_bare_strings() {
    let uids = ["K1".to_owned(), "K2".to_owned()];
    let list = marshal::removed_info_list(&uids);

    unsafe {
        assert_eq!(g_slist_length(list), 2);
        for (index, expected) in ["K1", "K2"].into_iter().enumerate() {
            let node = g_slist_nth_data(list, index as u32).cast::<EBookMetaBackendInfo>();
            assert!(!node.is_null(), "no node {index}");
            assert_eq!(CStr::from_ptr((*node).uid).to_str().unwrap(), expected);
            // The other three are nullable and unused for removals.
            assert!((*node).object.is_null(), "a removal described an object");
            assert!((*node).extra.is_null(), "a removal invented extra state");
        }
        g_slist_free_full(list, Some(e_book_meta_backend_info_free));
    }
}

/// `X-JMAP-KEY` parameters survive round-tripping through `EContact`.
#[test]
fn a_vcard_round_trips_through_econtact_keeping_its_jmap_parameters() {
    let vcard = "BEGIN:VCARD\r\n\
                 VERSION:3.0\r\n\
                 UID:K1\r\n\
                 FN:Vera Olden\r\n\
                 X-JMAP-UID:urn:uuid:1234\r\n\
                 EMAIL;TYPE=WORK;X-JMAP-KEY=e7:vera@example.com\r\n\
                 END:VCARD\r\n";

    let contact = marshal::contact_from_vcard(vcard);
    assert!(!contact.is_null());

    unsafe {
        let back = marshal::vcard_from_contact(contact).expect("rendered");
        assert!(back.contains("X-JMAP-KEY=e7"), "key lost: {back}");
        assert!(
            back.contains("X-JMAP-UID:urn:uuid:1234"),
            "uid lost: {back}"
        );
        assert!(back.contains("TYPE=WORK"), "type lost: {back}");

        let full_name = e_contact_get_const(contact, E_CONTACT_FULL_NAME).cast::<i8>();
        assert_eq!(CStr::from_ptr(full_name).to_str().unwrap(), "Vera Olden");
        let email = e_contact_get_const(contact, E_CONTACT_EMAIL_1).cast::<i8>();
        assert_eq!(CStr::from_ptr(email).to_str().unwrap(), "vera@example.com");
        let uid = e_contact_get_const(contact, E_CONTACT_UID).cast::<i8>();
        assert_eq!(CStr::from_ptr(uid).to_str().unwrap(), "K1");

        assert_eq!(marshal::contact_uid(contact).as_deref(), Some("K1"));
        marshal::contact_unref(contact);
    }
}

/// Verifies contact without a UID reports None.
#[test]
fn a_contact_without_a_uid_reports_none() {
    for vcard in [
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Nobody\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:\r\nFN:Nobody\r\nEND:VCARD\r\n",
    ] {
        let contact = marshal::contact_from_vcard(vcard);
        unsafe {
            assert_eq!(marshal::contact_uid(contact), None, "for {vcard:?}");
            marshal::contact_unref(contact);
        }
    }
}

#[test]
fn a_malformed_vcard_is_refused_rather_than_yielding_an_empty_contact() {
    assert!(marshal::contact_from_vcard("not a vcard at all").is_null());
}
