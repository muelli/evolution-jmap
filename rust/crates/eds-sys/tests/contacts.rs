// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `EContactField` properties and instant-messaging field structure in EDS.
//!
//! EDS (libebook-contacts 3.52) defines 60 synthetic per-slot string fields for
//! the 10 instant-messaging services (`AIM`, `GADUGADU`, `GOOGLE_TALK`,
//! `GROUPWISE`, `ICQ`, `JABBER`, `MSN`, `MATRIX`, `SKYPE`, `YAHOO`), each with
//! 6 slots (`HOME_1..3`, `WORK_1..3`).
//!
//! Two other fields — `E_CONTACT_SIP` and `E_CONTACT_IM_TWITTER` — are defined as
//! `EContactAttrList` (`GList*` of `char*`) rather than strings, with no
//! synthetic per-slot fields. This test suite verifies these field properties
//! directly against the EDS type system and vCard field mappings.

use std::ffi::CStr;

use eds_sys::*;

#[test]
fn twitter_and_sip_fields_are_unslotted_attribute_lists_not_strings() {
    unsafe {
        let attr_list_type = e_contact_attr_list_get_type();
        assert_ne!(attr_list_type, 0);

        // E_CONTACT_SIP (127) is an attribute list, not a string
        assert_eq!(e_contact_field_is_string(E_CONTACT_SIP), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_SIP), attr_list_type);
        let sip_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_SIP));
        assert_eq!(sip_name.to_str().unwrap(), "sip");

        // E_CONTACT_IM_TWITTER (135) is an attribute list, not a string
        assert_eq!(e_contact_field_is_string(E_CONTACT_IM_TWITTER), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_IM_TWITTER), attr_list_type);
        let twitter_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_IM_TWITTER));
        assert_eq!(twitter_name.to_str().unwrap(), "im_twitter");
    }
}

#[test]
fn instant_messaging_slotted_fields_are_strings_and_have_home_work_slots() {
    let slotted_services: &[(EContactField, EContactField, &str)] = &[
        (E_CONTACT_IM_AIM_HOME_1, E_CONTACT_IM_AIM_WORK_3, "im_aim"),
        (
            E_CONTACT_IM_GROUPWISE_HOME_1,
            E_CONTACT_IM_GROUPWISE_WORK_3,
            "im_groupwise",
        ),
        (
            E_CONTACT_IM_JABBER_HOME_1,
            E_CONTACT_IM_JABBER_WORK_3,
            "im_jabber",
        ),
        (
            E_CONTACT_IM_YAHOO_HOME_1,
            E_CONTACT_IM_YAHOO_WORK_3,
            "im_yahoo",
        ),
        (E_CONTACT_IM_MSN_HOME_1, E_CONTACT_IM_MSN_WORK_3, "im_msn"),
        (E_CONTACT_IM_ICQ_HOME_1, E_CONTACT_IM_ICQ_WORK_3, "im_icq"),
        (
            E_CONTACT_IM_GADUGADU_HOME_1,
            E_CONTACT_IM_GADUGADU_WORK_3,
            "im_gadugadu",
        ),
        (
            E_CONTACT_IM_SKYPE_HOME_1,
            E_CONTACT_IM_SKYPE_WORK_3,
            "im_skype",
        ),
        (
            E_CONTACT_IM_GOOGLE_TALK_HOME_1,
            E_CONTACT_IM_GOOGLE_TALK_WORK_3,
            "im_google_talk",
        ),
        (
            E_CONTACT_IM_MATRIX_HOME_1,
            E_CONTACT_IM_MATRIX_WORK_3,
            "im_matrix",
        ),
    ];

    unsafe {
        for &(start, end, prefix) in slotted_services {
            assert_eq!(end - start + 1, 6, "each service must have exactly 6 slots");
            for field in start..=end {
                assert_eq!(
                    e_contact_field_is_string(field),
                    1,
                    "field {field} must be a string"
                );
                let name = CStr::from_ptr(e_contact_field_name(field))
                    .to_str()
                    .unwrap();
                assert!(
                    name.starts_with(prefix),
                    "field {field} name '{name}' must start with prefix '{prefix}'"
                );
            }
        }
    }
}

#[test]
fn e_contact_field_id_from_vcard_maps_x_lines() {
    unsafe {
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-SIP".as_ptr()),
            E_CONTACT_SIP
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-TWITTER".as_ptr()),
            E_CONTACT_IM_TWITTER
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-JABBER".as_ptr()),
            E_CONTACT_IM_JABBER
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-AIM".as_ptr()),
            E_CONTACT_IM_AIM
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-GADUGADU".as_ptr()),
            E_CONTACT_IM_GADUGADU
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-SKYPE".as_ptr()),
            E_CONTACT_IM_SKYPE
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-MATRIX".as_ptr()),
            E_CONTACT_IM_MATRIX
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-ICQ".as_ptr()),
            E_CONTACT_IM_ICQ
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-MSN".as_ptr()),
            E_CONTACT_IM_MSN
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-YAHOO".as_ptr()),
            E_CONTACT_IM_YAHOO
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-GOOGLE-TALK".as_ptr()),
            E_CONTACT_IM_GOOGLE_TALK
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-GROUPWISE".as_ptr()),
            E_CONTACT_IM_GROUPWISE
        );
    }
}
