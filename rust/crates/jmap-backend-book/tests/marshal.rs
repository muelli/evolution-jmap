// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The C boundary of the address book backend, exercised the way EDS will:
//! every list is walked as a `GSList` and freed with the function EDS would
//! call, so a wrong node type or a missing copy shows up here rather than as
//! a crash in `evolution-addressbook-factory`.

use std::ffi::{CStr, CString};

use eds_sys::{
    E_CONTACT_EMAIL_1, E_CONTACT_FULL_NAME, E_CONTACT_UID, E_SOURCE_CREDENTIAL_PASSWORD,
    EBookMetaBackendInfo, e_book_meta_backend_info_free, e_contact_get_const,
    e_named_parameters_free, e_named_parameters_new, e_named_parameters_set,
};
use glib_sys::{GSList, g_free, g_slist_free_full, g_slist_length, g_slist_nth_data};
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
    assert!(marshal::uid_list(&[]).is_null());
}

#[test]
fn a_uid_list_copies_the_strings_so_the_caller_can_free_them() {
    let uids = ["K1".to_owned(), "K2".to_owned()];
    let list = marshal::uid_list(&uids);

    unsafe {
        assert_eq!(g_slist_length(list), 2);
        let first = g_slist_nth_data(list, 0).cast::<i8>();
        assert_eq!(CStr::from_ptr(first).to_str().unwrap(), "K1");
        // Frees each node's payload with g_free, which is only valid if the
        // marshalling allocated them rather than pointing into the Rust
        // strings above.
        g_slist_free_full(list, Some(g_free));
    }
}

/// The whole mapping design rests on `X-JMAP-KEY` surviving the trip through
/// `EContact` — an edit that loses the key becomes a remove-and-re-add
/// server-side. This is the first test that puts a real `EVCard` under it.
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

/// A contact Evolution has just created has no `UID` yet; `save_contact_sync`
/// tells a create from an edit by exactly this, so a missing `UID` must not
/// come back as a usable identifier.
///
/// Both spellings are checked because `EVCard` distinguishes them and the
/// backend must not: no `UID` line at all reads back as NULL, but a `UID:`
/// with an empty value reads back as `""`, which would be sent to the server
/// as the identifier of a card to patch.
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

#[test]
fn the_password_is_read_out_of_the_named_parameters() {
    unsafe {
        let params = e_named_parameters_new();
        let value = CString::new("hunter2").unwrap();
        e_named_parameters_set(
            params,
            E_SOURCE_CREDENTIAL_PASSWORD.as_ptr(),
            value.as_ptr(),
        );
        assert_eq!(marshal::password(params).as_deref(), Some("hunter2"));
        e_named_parameters_free(params);
    }
}

/// Reporting a stored-but-empty password as absent would make `connect_sync`
/// ask for a prompt, and a user who enters nothing would be prompted again
/// forever. Sending it and being told it is wrong terminates.
#[test]
fn an_empty_stored_password_is_present_not_absent() {
    unsafe {
        let params = e_named_parameters_new();
        let value = CString::new("").unwrap();
        e_named_parameters_set(
            params,
            E_SOURCE_CREDENTIAL_PASSWORD.as_ptr(),
            value.as_ptr(),
        );
        assert_eq!(marshal::password(params).as_deref(), Some(""));
        e_named_parameters_free(params);
    }
}

#[test]
fn credentials_without_a_password_report_none() {
    unsafe {
        let params = e_named_parameters_new();
        assert_eq!(marshal::password(params), None);
        e_named_parameters_free(params);
    }
}

/// EDS calls `connect_sync` with NULL credentials on the first attempt, before
/// it has asked libsecret for anything.
#[test]
fn null_credentials_report_none() {
    unsafe {
        assert_eq!(marshal::password(std::ptr::null()), None);
    }
}

#[test]
fn an_out_string_is_a_copy_the_caller_owns() {
    let mut out: *mut i8 = std::ptr::null_mut();
    unsafe {
        marshal::set_out_string(&mut out, "state-7");
        assert_eq!(CStr::from_ptr(out).to_str().unwrap(), "state-7");
        g_free(out.cast());
        // A NULL out-parameter is the GLib convention for "not interested".
        marshal::set_out_string(std::ptr::null_mut(), "state-8");
    }
}
