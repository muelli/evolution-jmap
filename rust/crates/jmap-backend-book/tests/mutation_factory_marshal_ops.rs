// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Targeted integration tests killing mutants across `factory.rs`, `marshal.rs`,
//! `ops.rs`, and `connect.rs`.

use std::ffi::CStr;

use eds_sys::{
    E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND, E_CLIENT_ERROR_INVALID_ARG, EBookMetaBackendInfo,
    e_book_backend_factory_get_type, e_book_client_error_quark, e_book_meta_backend_info_free,
    e_client_error_quark,
};
use glib_sys::{GType, g_slist_free_full, g_slist_length, g_slist_nth_data};
use jmap_backend_book::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};
use jmap_backend_book::factory::{FACTORY_NAME, JmapBookFactory, remember_backend_type};
use jmap_backend_book::marshal;
use jmap_backend_book::ops;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_book_sync::{ContactInfo, SyncError};
use jmap_client::Error as ClientError;

#[test]
fn factory_class_init_types_and_remember_backend_type() {
    let dummy_type: GType = 0xDEAD_BEEF;
    remember_backend_type(dummy_type);

    let types = <JmapBookFactory as ObjectSubclass>::class_init_types();
    assert_eq!(
        types.len(),
        1,
        "class_init_types must declare exactly one prerequisite type"
    );
    assert_eq!(
        types[0], dummy_type,
        "class_init_types must return the remembered backend type"
    );

    let second_dummy: GType = 0xCAFE_BABE;
    remember_backend_type(second_dummy);
    let updated_types = <JmapBookFactory as ObjectSubclass>::class_init_types();
    assert_eq!(updated_types, vec![second_dummy]);

    // Restore to zero so future registrations can initialize cleanly.
    remember_backend_type(0);
}

#[test]
fn factory_metadata_and_parent_type() {
    assert_eq!(
        FACTORY_NAME, c"jmap",
        "factory name must match EDS source configuration backend name"
    );
    assert_eq!(
        <JmapBookFactory as ObjectSubclass>::NAME,
        c"EBookBackendJmapFactory",
        "subclass name must match EDS factory GType convention"
    );

    let parent_type = <JmapBookFactory as ObjectSubclass>::parent_type();
    assert_ne!(
        parent_type, 0,
        "parent_type must be a valid registered GType"
    );
    unsafe {
        assert_eq!(
            parent_type,
            e_book_backend_factory_get_type(),
            "parent_type must derive directly from EBookBackendFactory"
        );
    }
}

#[test]
fn marshal_info_list_ordering_and_null_termination() {
    let empty_list = marshal::info_list(&[]);
    assert!(empty_list.is_null(), "empty info list must be null");

    let empty_removed = marshal::removed_info_list(&[]);
    assert!(empty_removed.is_null(), "empty removed list must be null");

    let infos = vec![
        ContactInfo {
            uid: "contact-1".to_string(),
            revision: "rev-1".to_string(),
            vcard: "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:First\r\nEND:VCARD\r\n".to_string(),
        },
        ContactInfo {
            uid: "contact-2".to_string(),
            revision: "rev-2".to_string(),
            vcard: "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Second\r\nEND:VCARD\r\n".to_string(),
        },
    ];

    let list = marshal::info_list(&infos);
    assert!(!list.is_null());
    unsafe {
        assert_eq!(g_slist_length(list), 2);

        let first = g_slist_nth_data(list, 0).cast::<EBookMetaBackendInfo>();
        assert!(!first.is_null());
        assert_eq!(CStr::from_ptr((*first).uid).to_str().unwrap(), "contact-1");
        assert_eq!(CStr::from_ptr((*first).revision).to_str().unwrap(), "rev-1");

        let second = g_slist_nth_data(list, 1).cast::<EBookMetaBackendInfo>();
        assert!(!second.is_null());
        assert_eq!(CStr::from_ptr((*second).uid).to_str().unwrap(), "contact-2");
        assert_eq!(
            CStr::from_ptr((*second).revision).to_str().unwrap(),
            "rev-2"
        );

        g_slist_free_full(list, Some(e_book_meta_backend_info_free));
    }

    let removed = vec!["del-1".to_string(), "del-2".to_string()];
    let removed_list = marshal::removed_info_list(&removed);
    assert!(!removed_list.is_null());
    unsafe {
        assert_eq!(g_slist_length(removed_list), 2);

        let node1 = g_slist_nth_data(removed_list, 0).cast::<EBookMetaBackendInfo>();
        assert_eq!(CStr::from_ptr((*node1).uid).to_str().unwrap(), "del-1");
        assert!((*node1).object.is_null());
        assert!((*node1).extra.is_null());

        let node2 = g_slist_nth_data(removed_list, 1).cast::<EBookMetaBackendInfo>();
        assert_eq!(CStr::from_ptr((*node2).uid).to_str().unwrap(), "del-2");

        g_slist_free_full(removed_list, Some(e_book_meta_backend_info_free));
    }
}

#[test]
fn marshal_contact_from_vcard_validation_and_parsing() {
    let valid_vcard =
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:uid-123\r\nFN:Alice Smith\r\nEND:VCARD\r\n";
    let contact = marshal::contact_from_vcard(valid_vcard);
    assert!(
        !contact.is_null(),
        "valid vCard must parse to non-null contact"
    );

    unsafe {
        assert_eq!(marshal::contact_uid(contact).as_deref(), Some("uid-123"));
        let rendered = marshal::vcard_from_contact(contact).expect("vcard serialization");
        assert!(rendered.contains("FN:Alice Smith"));
        marshal::contact_unref(contact);
    }

    let lowercase_vcard = "begin:vcard\r\nversion:3.0\r\nfn:Bob\r\nend:vcard\r\n";
    let lower_contact = marshal::contact_from_vcard(lowercase_vcard);
    assert!(
        !lower_contact.is_null(),
        "case-insensitive begin:vcard must parse"
    );
    unsafe { marshal::contact_unref(lower_contact) };

    let padded_vcard = "   \r\n  BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Charlie\r\nEND:VCARD\r\n";
    let padded_contact = marshal::contact_from_vcard(padded_vcard);
    assert!(
        !padded_contact.is_null(),
        "whitespace-prefixed vCard must parse"
    );
    unsafe { marshal::contact_unref(padded_contact) };

    assert!(
        marshal::contact_from_vcard("").is_null(),
        "empty string must not parse as vCard"
    );
    assert!(
        marshal::contact_from_vcard("NOT A VCARD").is_null(),
        "invalid string must return null"
    );
    assert!(
        marshal::contact_from_vcard("VERSION:3.0\r\nFN:Missing Begin\r\nEND:VCARD\r\n").is_null(),
        "vCard lacking BEGIN:VCARD must return null"
    );

    unsafe {
        assert!(marshal::contact_uid(std::ptr::null_mut()).is_none());
        assert!(marshal::vcard_from_contact(std::ptr::null_mut()).is_none());
        marshal::contact_unref(std::ptr::null_mut());
    }
}

#[test]
fn ops_to_gerror_maps_all_sync_error_variants() {
    let not_found_err = SyncError::NotFound("card-404".to_string());
    let gerror_not_found = ops::to_gerror(&not_found_err);
    assert!(
        !gerror_not_found.is_null(),
        "to_gerror must allocate a non-null GError"
    );
    unsafe {
        assert_eq!((*gerror_not_found).domain, e_book_client_error_quark());
        assert_eq!(
            (*gerror_not_found).code,
            E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND as i32
        );
        let msg = CStr::from_ptr((*gerror_not_found).message)
            .to_str()
            .unwrap();
        assert!(msg.contains("card-404"));
        glib_sys::g_error_free(gerror_not_found);
    }

    let vcard_err = SyncError::VCard(jmap_vcard::VCardError::NotAVCard);
    let gerror_vcard = ops::to_gerror(&vcard_err);
    assert!(!gerror_vcard.is_null());
    unsafe {
        assert_eq!((*gerror_vcard).domain, e_client_error_quark());
        assert_eq!((*gerror_vcard).code, E_CLIENT_ERROR_INVALID_ARG as i32);
        glib_sys::g_error_free(gerror_vcard);
    }

    let client_err = SyncError::Client(ClientError::Transport("down".to_string()));
    let gerror_client = ops::to_gerror(&client_err);
    assert!(!gerror_client.is_null());
    unsafe {
        assert_eq!((*gerror_client).domain, e_client_error_quark());
        glib_sys::g_error_free(gerror_client);
    }
}

#[test]
fn connect_constants_and_auth_result_writing() {
    assert_eq!(
        ACCEPTED_AUTH_RESULT,
        eds_sys::E_SOURCE_AUTHENTICATION_ACCEPTED,
        "ACCEPTED_AUTH_RESULT must match EDS enum value"
    );

    unsafe {
        write_auth_result(std::ptr::null_mut(), ACCEPTED_AUTH_RESULT);

        let mut out_result = eds_sys::E_SOURCE_AUTHENTICATION_REJECTED;
        write_auth_result(&mut out_result, ACCEPTED_AUTH_RESULT);
        assert_eq!(out_result, ACCEPTED_AUTH_RESULT);
    }
}
