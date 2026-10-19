// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests for mutants in `factory.rs`, `marshal.rs`,
//! `ops.rs`, and `connect.rs`.

use std::ffi::CStr;

use eds_sys::{
    E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, E_CLIENT_ERROR_INVALID_ARG, E_CLIENT_ERROR_NOT_SUPPORTED,
    E_CLIENT_ERROR_OTHER_ERROR, E_SOURCE_AUTHENTICATION_ACCEPTED, E_SOURCE_AUTHENTICATION_ERROR,
    ECalMetaBackendInfo, I_CAL_VEVENT_COMPONENT, e_cal_backend_factory_get_type,
    e_cal_client_error_quark, e_cal_meta_backend_info_free, e_client_error_quark,
};
use glib_sys::{GType, g_slist_free_full, g_slist_length, g_slist_nth_data};
use jmap_backend_cal::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};
use jmap_backend_cal::factory::{
    COMPONENT_KIND, FACTORY_NAME, JmapCalFactory, remember_backend_type,
};
use jmap_backend_cal::marshal;
use jmap_backend_cal::ops;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_cal_sync::{ComponentInfo, SyncError, Unscheduled, Unsendable};
use jmap_client::Error as ClientError;

#[test]
fn factory_class_init_types_and_remember_backend_type() {
    let dummy_type: GType = 0xDEAD_BEEF;
    remember_backend_type(dummy_type);

    let types = <JmapCalFactory as ObjectSubclass>::class_init_types();
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
    let updated_types = <JmapCalFactory as ObjectSubclass>::class_init_types();
    assert_eq!(updated_types, vec![second_dummy]);

    // Restore to zero so future registrations initialize cleanly.
    remember_backend_type(0);
}

#[test]
fn factory_metadata_and_parent_type() {
    assert_eq!(
        FACTORY_NAME, c"jmap",
        "factory name must match EDS source configuration backend name"
    );
    assert_eq!(
        COMPONENT_KIND, I_CAL_VEVENT_COMPONENT,
        "factory must serve VEVENT calendar component kind"
    );
    assert_eq!(
        <JmapCalFactory as ObjectSubclass>::NAME,
        c"ECalBackendJmapFactory",
        "subclass name must match EDS factory GType convention"
    );

    let parent_type = <JmapCalFactory as ObjectSubclass>::parent_type();
    assert_ne!(
        parent_type, 0,
        "parent_type must be a valid registered GType"
    );
    unsafe {
        assert_eq!(
            parent_type,
            e_cal_backend_factory_get_type(),
            "parent_type must derive directly from ECalBackendFactory"
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
        ComponentInfo {
            uid: "event-1".to_string(),
            revision: "rev-1".to_string(),
            icalendar: "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:event-1\r\nDTSTART:20261017T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".to_string(),
        },
        ComponentInfo {
            uid: "event-2".to_string(),
            revision: "rev-2".to_string(),
            icalendar: "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:event-2\r\nDTSTART:20261017T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".to_string(),
        },
    ];

    let list = marshal::info_list(&infos);
    assert!(!list.is_null());
    unsafe {
        assert_eq!(g_slist_length(list), 2);

        let first = g_slist_nth_data(list, 0).cast::<ECalMetaBackendInfo>();
        assert!(!first.is_null());
        assert_eq!(CStr::from_ptr((*first).uid), c"event-1");
        assert_eq!(CStr::from_ptr((*first).revision), c"rev-1");

        let second = g_slist_nth_data(list, 1).cast::<ECalMetaBackendInfo>();
        assert!(!second.is_null());
        assert_eq!(CStr::from_ptr((*second).uid), c"event-2");
        assert_eq!(CStr::from_ptr((*second).revision), c"rev-2");

        g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }

    let uids = vec!["del-1".to_string(), "del-2".to_string()];
    let removed = marshal::removed_info_list(&uids);
    assert!(!removed.is_null());
    unsafe {
        assert_eq!(g_slist_length(removed), 2);

        let first = g_slist_nth_data(removed, 0).cast::<ECalMetaBackendInfo>();
        assert_eq!(CStr::from_ptr((*first).uid), c"del-1");
        assert!((*first).revision.is_null());
        assert!((*first).object.is_null());

        let second = g_slist_nth_data(removed, 1).cast::<ECalMetaBackendInfo>();
        assert_eq!(CStr::from_ptr((*second).uid), c"del-2");

        g_slist_free_full(removed, Some(e_cal_meta_backend_info_free));
    }
}

#[test]
fn marshal_component_from_ical_validation_and_parsing() {
    let empty = marshal::component_from_ical("");
    assert!(empty.is_null(), "empty string must yield null component");

    let invalid = marshal::component_from_ical("not an icalendar");
    assert!(
        invalid.is_null(),
        "non-calendar string must yield null component"
    );

    let valid_ical = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:test-uid\r\nDTSTART:20261017T140000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let component = marshal::component_from_ical(valid_ical);
    assert!(
        !component.is_null(),
        "valid VCALENDAR with VEVENT must parse successfully"
    );
    unsafe {
        gobject_sys::g_object_unref(component.cast());
    }

    // A VCALENDAR with only VTODO must return null per the holds_event check.
    let todo_ical = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:todo-1\r\nSUMMARY:Task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    let todo_component = marshal::component_from_ical(todo_ical);
    assert!(
        todo_component.is_null(),
        "VCALENDAR without VEVENT must yield null"
    );
}

#[test]
fn ops_to_gerror_maps_all_sync_error_variants() {
    let not_found = SyncError::NotFound("evt-123".to_string());
    let err = ops::to_gerror(&not_found);
    assert!(!err.is_null());
    unsafe {
        assert_eq!((*err).domain, e_cal_client_error_quark());
        assert_eq!((*err).code, E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND as i32);
        glib_sys::g_error_free(err);
    }

    let ical_err = SyncError::ICal(jmap_ical::error::ICalError::Unterminated(
        "bad syntax".to_string(),
    ));
    let err = ops::to_gerror(&ical_err);
    assert!(!err.is_null());
    unsafe {
        assert_eq!((*err).domain, e_client_error_quark());
        assert_eq!((*err).code, E_CLIENT_ERROR_INVALID_ARG as i32);
        glib_sys::g_error_free(err);
    }

    let client_err = SyncError::Client(ClientError::Protocol("jmap error".to_string()));
    let err = ops::to_gerror(&client_err);
    assert!(!err.is_null());
    unsafe {
        assert_eq!((*err).domain, e_client_error_quark());
        assert_eq!((*err).code, E_CLIENT_ERROR_OTHER_ERROR as i32);
        glib_sys::g_error_free(err);
    }

    let unsendable_err = SyncError::Unsendable(Unsendable::Recurrence);
    let err = ops::to_gerror(&unsendable_err);
    assert!(!err.is_null());
    unsafe {
        assert_eq!((*err).domain, e_client_error_quark());
        assert_eq!((*err).code, E_CLIENT_ERROR_INVALID_ARG as i32);
        glib_sys::g_error_free(err);
    }

    let unscheduled_unreachable =
        SyncError::Unscheduled(Unscheduled::Unreachable { addresses: vec![] });
    let err = ops::to_gerror(&unscheduled_unreachable);
    assert!(!err.is_null());
    unsafe {
        assert_eq!((*err).domain, e_client_error_quark());
        assert_eq!((*err).code, E_CLIENT_ERROR_INVALID_ARG as i32);
        glib_sys::g_error_free(err);
    }

    let unscheduled_not_impl = SyncError::Unscheduled(Unscheduled::NotImplemented);
    let err = ops::to_gerror(&unscheduled_not_impl);
    assert!(!err.is_null());
    unsafe {
        assert_eq!((*err).domain, e_client_error_quark());
        assert_eq!((*err).code, E_CLIENT_ERROR_NOT_SUPPORTED as i32);
        glib_sys::g_error_free(err);
    }
}

#[test]
fn connect_constants_and_auth_result_writing() {
    assert_eq!(ACCEPTED_AUTH_RESULT, E_SOURCE_AUTHENTICATION_ACCEPTED);

    let mut result = E_SOURCE_AUTHENTICATION_ERROR;
    // SAFETY: result is a valid mutable pointer, and the function handles null.
    unsafe {
        write_auth_result(&mut result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);

        write_auth_result(std::ptr::null_mut(), E_SOURCE_AUTHENTICATION_ACCEPTED);
    }
}
