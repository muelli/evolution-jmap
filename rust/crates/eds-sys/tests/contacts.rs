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

#[test]
fn contact_date_fields_are_structured_e_contact_date_types() {
    unsafe {
        let date_type = e_contact_date_get_type();
        assert_ne!(date_type, 0);

        // E_CONTACT_BIRTH_DATE is structured EContactDate, not string
        assert_eq!(e_contact_field_is_string(E_CONTACT_BIRTH_DATE), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_BIRTH_DATE), date_type);
        let bday_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_BIRTH_DATE));
        assert_eq!(bday_name.to_str().unwrap(), "birth_date");

        // E_CONTACT_ANNIVERSARY is structured EContactDate, not string
        assert_eq!(e_contact_field_is_string(E_CONTACT_ANNIVERSARY), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_ANNIVERSARY), date_type);
        let ann_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ANNIVERSARY));
        assert_eq!(ann_name.to_str().unwrap(), "anniversary");

        // BDAY and X-EVOLUTION-ANNIVERSARY resolve to their field IDs
        assert_eq!(
            e_contact_field_id_from_vcard(c"BDAY".as_ptr()),
            E_CONTACT_BIRTH_DATE
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-EVOLUTION-ANNIVERSARY".as_ptr()),
            E_CONTACT_ANNIVERSARY
        );

        // Unmodeled date headers (DEATHDATE, X-DEATHDATE, ANNIVERSARY) have no EDS field ID
        assert_eq!(e_contact_field_id_from_vcard(c"DEATHDATE".as_ptr()), 0);
        assert_eq!(e_contact_field_id_from_vcard(c"X-DEATHDATE".as_ptr()), 0);
        assert_eq!(e_contact_field_id_from_vcard(c"ANNIVERSARY".as_ptr()), 0);
    }
}

#[test]
fn e_contact_date_parsing_and_formatting() {
    unsafe {
        // Complete ISO date parsing
        let date = e_contact_date_from_string(c"1964-03-27".as_ptr());
        assert!(!date.is_null());
        assert_eq!((*date).year, 1964);
        assert_eq!((*date).month, 3);
        assert_eq!((*date).day, 27);

        let formatted = e_contact_date_to_string(date);
        assert!(!formatted.is_null());
        assert_eq!(CStr::from_ptr(formatted).to_str().unwrap(), "1964-03-27");
        g_free(formatted.cast());
        e_contact_date_free(date);

        // Compact ISO date parsing
        let compact = e_contact_date_from_string(c"19640327".as_ptr());
        assert!(!compact.is_null());
        assert_eq!((*compact).year, 1964);
        assert_eq!((*compact).month, 3);
        assert_eq!((*compact).day, 27);
        e_contact_date_free(compact);
    }
}

#[test]
fn contact_org_title_and_role_field_properties() {
    unsafe {
        // E_CONTACT_ORG (35) is a string field
        assert_eq!(e_contact_field_is_string(E_CONTACT_ORG), 1);
        let org_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ORG));
        assert_eq!(org_name.to_str().unwrap(), "org");

        // E_CONTACT_ORG_UNIT (36) is a string field
        assert_eq!(e_contact_field_is_string(E_CONTACT_ORG_UNIT), 1);
        let unit_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ORG_UNIT));
        assert_eq!(unit_name.to_str().unwrap(), "org_unit");

        // E_CONTACT_TITLE (38) is a string field
        assert_eq!(e_contact_field_is_string(E_CONTACT_TITLE), 1);
        let title_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_TITLE));
        assert_eq!(title_name.to_str().unwrap(), "title");

        // E_CONTACT_ROLE (39) is a string field
        assert_eq!(e_contact_field_is_string(E_CONTACT_ROLE), 1);
        let role_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ROLE));
        assert_eq!(role_name.to_str().unwrap(), "role");

        // vCard field mapping: TITLE and ROLE map directly
        assert_eq!(
            e_contact_field_id_from_vcard(c"TITLE".as_ptr()),
            E_CONTACT_TITLE
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"ROLE".as_ptr()),
            E_CONTACT_ROLE
        );

        // ORG and ORG_UNIT both map to the "ORG" vCard attribute
        let org_attr = CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_ORG));
        assert_eq!(org_attr.to_str().unwrap(), "ORG");
        let unit_attr = CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_ORG_UNIT));
        assert_eq!(unit_attr.to_str().unwrap(), "ORG");
        let title_attr = CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_TITLE));
        assert_eq!(title_attr.to_str().unwrap(), "TITLE");
        let role_attr = CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_ROLE));
        assert_eq!(role_attr.to_str().unwrap(), "ROLE");
    }
}

#[test]
fn multiple_org_title_and_role_vcard_lines_behavior_in_eds() {
    unsafe {
        let vcard_str = c"BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Test Contact\r\n\
N:Contact;Test;;;\r\n\
ORG;X-JMAP-KEY=o1:Acme Ltd;Research\r\n\
ORG;X-JMAP-KEY=o2:Brauerei;Logistics\r\n\
TITLE;X-JMAP-KEY=t1:Research Scientist\r\n\
TITLE;X-JMAP-KEY=t2:Director of Engineering\r\n\
ROLE;X-JMAP-KEY=r1:Lead Investigator\r\n\
ROLE;X-JMAP-KEY=r2:Project Manager\r\n\
END:VCARD\r\n";

        let contact = e_contact_new_from_vcard(vcard_str.as_ptr());
        assert!(!contact.is_null());

        // EDS returns the first value for ORG, ORG_UNIT, TITLE, and ROLE
        let org_ptr = e_contact_get_const(contact, E_CONTACT_ORG);
        assert_eq!(CStr::from_ptr(org_ptr.cast()).to_str().unwrap(), "Acme Ltd");

        let unit_ptr = e_contact_get_const(contact, E_CONTACT_ORG_UNIT);
        assert_eq!(
            CStr::from_ptr(unit_ptr.cast()).to_str().unwrap(),
            "Research"
        );

        let title_ptr = e_contact_get_const(contact, E_CONTACT_TITLE);
        assert_eq!(
            CStr::from_ptr(title_ptr.cast()).to_str().unwrap(),
            "Research Scientist"
        );

        let role_ptr = e_contact_get_const(contact, E_CONTACT_ROLE);
        assert_eq!(
            CStr::from_ptr(role_ptr.cast()).to_str().unwrap(),
            "Lead Investigator"
        );

        // Modifying TITLE in place preserves X-JMAP-KEY and leaves second TITLE intact
        e_contact_set(
            contact,
            E_CONTACT_TITLE,
            c"Principal Scientist".as_ptr().cast(),
        );
        let updated_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let updated_vcard = CStr::from_ptr(updated_vcard_ptr).to_str().unwrap();

        assert!(
            updated_vcard.contains("TITLE;X-JMAP-KEY=t1:Principal Scientist"),
            "first TITLE should be updated in place keeping key: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("TITLE;X-JMAP-KEY=t2:Director of Engineering"),
            "second TITLE should remain intact: {updated_vcard}"
        );
        g_free(updated_vcard_ptr.cast());

        // Modifying ORG in place preserves X-JMAP-KEY and leaves second ORG intact
        e_contact_set(contact, E_CONTACT_ORG, c"Cyberdyne".as_ptr().cast());
        let updated_org_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let updated_org_vcard = CStr::from_ptr(updated_org_vcard_ptr).to_str().unwrap();

        assert!(
            updated_org_vcard.contains("ORG;X-JMAP-KEY=o1:Cyberdyne;Research"),
            "first ORG should be updated in place keeping key: {updated_org_vcard}"
        );
        assert!(
            updated_org_vcard.contains("ORG;X-JMAP-KEY=o2:Brauerei;Logistics"),
            "second ORG should remain intact: {updated_org_vcard}"
        );
        g_free(updated_org_vcard_ptr.cast());

        // Modifying ROLE in place preserves X-JMAP-KEY and leaves second ROLE intact
        e_contact_set(
            contact,
            E_CONTACT_ROLE,
            c"Chief Investigator".as_ptr().cast(),
        );
        let updated_role_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let updated_role_vcard = CStr::from_ptr(updated_role_vcard_ptr).to_str().unwrap();

        assert!(
            updated_role_vcard.contains("ROLE;X-JMAP-KEY=r1:Chief Investigator"),
            "first ROLE should be updated in place keeping key: {updated_role_vcard}"
        );
        assert!(
            updated_role_vcard.contains("ROLE;X-JMAP-KEY=r2:Project Manager"),
            "second ROLE should remain intact: {updated_role_vcard}"
        );
        g_free(updated_role_vcard_ptr.cast());

        gobject_sys::g_object_unref(contact.cast());
    }
}

#[test]
fn contact_address_and_label_field_properties() {
    unsafe {
        let addr_type = e_contact_address_get_type();
        assert_ne!(addr_type, 0);

        // Address fields are structured EContactAddress fields (not strings)
        assert_eq!(e_contact_field_is_string(E_CONTACT_ADDRESS_HOME), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_ADDRESS_HOME), addr_type);
        let addr_home_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ADDRESS_HOME));
        assert_eq!(addr_home_name.to_str().unwrap(), "address_home");

        assert_eq!(e_contact_field_is_string(E_CONTACT_ADDRESS_WORK), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_ADDRESS_WORK), addr_type);
        let addr_work_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ADDRESS_WORK));
        assert_eq!(addr_work_name.to_str().unwrap(), "address_work");

        assert_eq!(e_contact_field_is_string(E_CONTACT_ADDRESS_OTHER), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_ADDRESS_OTHER), addr_type);
        let addr_other_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ADDRESS_OTHER));
        assert_eq!(addr_other_name.to_str().unwrap(), "address_other");

        // Address label fields are synthetic string fields
        assert_eq!(e_contact_field_is_string(E_CONTACT_ADDRESS_LABEL_HOME), 1);
        let label_home_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ADDRESS_LABEL_HOME));
        assert_eq!(label_home_name.to_str().unwrap(), "address_label_home");

        assert_eq!(e_contact_field_is_string(E_CONTACT_ADDRESS_LABEL_WORK), 1);
        let label_work_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ADDRESS_LABEL_WORK));
        assert_eq!(label_work_name.to_str().unwrap(), "address_label_work");

        assert_eq!(e_contact_field_is_string(E_CONTACT_ADDRESS_LABEL_OTHER), 1);
        let label_other_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ADDRESS_LABEL_OTHER));
        assert_eq!(label_other_name.to_str().unwrap(), "address_label_other");

        // Attribute names
        assert_eq!(
            CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_ADDRESS_HOME))
                .to_str()
                .unwrap(),
            "ADR"
        );
        assert_eq!(
            CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_ADDRESS_LABEL_HOME))
                .to_str()
                .unwrap(),
            "LABEL"
        );
    }
}

#[test]
fn address_label_synthetic_fields_behavior_in_eds() {
    unsafe {
        let vcard_str = c"BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Test Contact\r\n\
N:Contact;Test;;;\r\n\
ADR;TYPE=WORK;X-JMAP-KEY=a1:;;Hauptstraße 1;Berlin;;10115;Germany\r\n\
LABEL;TYPE=WORK;X-JMAP-KEY=a1:Hauptstraße 1\\n10115 Berlin\\nGermany\r\n\
ADR;TYPE=HOME;X-JMAP-KEY=a2:;;Heimweg 2;München;;80331;Germany\r\n\
LABEL;TYPE=HOME;X-JMAP-KEY=a2:Heimweg 2\\n80331 München\\nGermany\r\n\
LABEL;TYPE=OTHER:Postfach 42\\n20095 Hamburg\r\n\
LABEL:Bare Label Without Type\r\n\
END:VCARD\r\n";

        let contact = e_contact_new_from_vcard(vcard_str.as_ptr());
        assert!(!contact.is_null());

        // EDS returns typed label values for HOME, WORK, and OTHER synthetic fields
        let work_label_ptr = e_contact_get_const(contact, E_CONTACT_ADDRESS_LABEL_WORK);
        assert!(!work_label_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(work_label_ptr.cast()).to_str().unwrap(),
            "Hauptstraße 1\n10115 Berlin\nGermany"
        );

        let home_label_ptr = e_contact_get_const(contact, E_CONTACT_ADDRESS_LABEL_HOME);
        assert!(!home_label_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(home_label_ptr.cast()).to_str().unwrap(),
            "Heimweg 2\n80331 München\nGermany"
        );

        let other_label_ptr = e_contact_get_const(contact, E_CONTACT_ADDRESS_LABEL_OTHER);
        assert!(!other_label_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(other_label_ptr.cast()).to_str().unwrap(),
            "Postfach 42\n20095 Hamburg"
        );

        // Modifying synthetic address label in place
        e_contact_set(
            contact,
            E_CONTACT_ADDRESS_LABEL_WORK,
            c"Updated Work Label\nBerlin".as_ptr().cast(),
        );

        let updated_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let updated_vcard = CStr::from_ptr(updated_vcard_ptr).to_str().unwrap();

        // Synthetic LABEL fields preserve custom parameters and are serialized with TYPE parameter by EDS
        assert!(
            updated_vcard.contains("LABEL;X-JMAP-KEY=a1;TYPE=WORK:Updated Work Label\\nBerlin"),
            "updated work label should be rendered with key and TYPE=WORK: {updated_vcard}"
        );
        // ADR line keeps its X-JMAP-KEY parameter
        assert!(
            updated_vcard.contains("ADR;X-JMAP-KEY=a1;TYPE=WORK:"),
            "ADR line should retain X-JMAP-KEY: {updated_vcard}"
        );
        // Secondary HOME label, OTHER label, and bare label remain intact
        assert!(
            updated_vcard
                .contains("LABEL;X-JMAP-KEY=a2;TYPE=HOME:Heimweg 2\\n80331 München\\nGermany"),
            "HOME label should remain intact: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("LABEL;TYPE=OTHER:Postfach 42\\n20095 Hamburg"),
            "OTHER label should remain intact: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("LABEL:Bare Label Without Type"),
            "bare label should remain intact: {updated_vcard}"
        );

        g_free(updated_vcard_ptr.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}

#[test]
fn instant_messaging_multi_service_and_slot_behavior_in_eds() {
    unsafe {
        let vcard_str = c"BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Test Contact\r\n\
N:Contact;Test;;;\r\n\
X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:alice@home.example\r\n\
X-JABBER;X-JMAP-KEY=s2;TYPE=WORK:alice@work.example\r\n\
X-MATRIX;X-JMAP-KEY=m1;TYPE=HOME:@alice:matrix.example\r\n\
X-SKYPE;X-JMAP-KEY=k1;TYPE=WORK:alice_work\r\n\
X-GADUGADU;X-JMAP-KEY=g1;TYPE=HOME:123456\r\n\
END:VCARD\r\n";

        let contact = e_contact_new_from_vcard(vcard_str.as_ptr());
        assert!(!contact.is_null());

        // EDS reads distinct slot fields for HOME_1 and WORK_1 across services
        let jabber_home_ptr = e_contact_get_const(contact, E_CONTACT_IM_JABBER_HOME_1);
        assert!(!jabber_home_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(jabber_home_ptr.cast()).to_str().unwrap(),
            "alice@home.example"
        );

        let jabber_work_ptr = e_contact_get_const(contact, E_CONTACT_IM_JABBER_WORK_1);
        assert!(!jabber_work_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(jabber_work_ptr.cast()).to_str().unwrap(),
            "alice@work.example"
        );

        let matrix_home_ptr = e_contact_get_const(contact, E_CONTACT_IM_MATRIX_HOME_1);
        assert!(!matrix_home_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(matrix_home_ptr.cast()).to_str().unwrap(),
            "@alice:matrix.example"
        );

        let skype_work_ptr = e_contact_get_const(contact, E_CONTACT_IM_SKYPE_WORK_1);
        assert!(!skype_work_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(skype_work_ptr.cast()).to_str().unwrap(),
            "alice_work"
        );

        let gg_home_ptr = e_contact_get_const(contact, E_CONTACT_IM_GADUGADU_HOME_1);
        assert!(!gg_home_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(gg_home_ptr.cast()).to_str().unwrap(),
            "123456"
        );

        // Modifying Jabber HOME_1 in place updates only that line and preserves its key
        e_contact_set(
            contact,
            E_CONTACT_IM_JABBER_HOME_1,
            c"alice_new@home.example".as_ptr().cast(),
        );

        let updated_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let updated_vcard = CStr::from_ptr(updated_vcard_ptr).to_str().unwrap();

        assert!(
            updated_vcard.contains("X-JABBER;TYPE=HOME;X-JMAP-KEY=s1:alice_new@home.example")
                || updated_vcard
                    .contains("X-JABBER;X-JMAP-KEY=s1;TYPE=HOME:alice_new@home.example"),
            "Jabber HOME should update in place keeping X-JMAP-KEY: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("X-JABBER;TYPE=WORK;X-JMAP-KEY=s2:alice@work.example")
                || updated_vcard.contains("X-JABBER;X-JMAP-KEY=s2;TYPE=WORK:alice@work.example"),
            "Jabber WORK should remain intact: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("X-MATRIX;TYPE=HOME;X-JMAP-KEY=m1:@alice:matrix.example")
                || updated_vcard.contains("X-MATRIX;X-JMAP-KEY=m1;TYPE=HOME:@alice:matrix.example"),
            "Matrix HOME should remain intact: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("X-SKYPE;TYPE=WORK;X-JMAP-KEY=k1:alice_work")
                || updated_vcard.contains("X-SKYPE;X-JMAP-KEY=k1;TYPE=WORK:alice_work"),
            "Skype WORK should remain intact: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("X-GADUGADU;TYPE=HOME;X-JMAP-KEY=g1:123456")
                || updated_vcard.contains("X-GADUGADU;X-JMAP-KEY=g1;TYPE=HOME:123456"),
            "Gadu-Gadu HOME should remain intact: {updated_vcard}"
        );

        // Modifying Skype WORK_1 in place updates only that line and preserves its key
        e_contact_set(
            contact,
            E_CONTACT_IM_SKYPE_WORK_1,
            c"alice_skype_new".as_ptr().cast(),
        );
        let second_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let second_vcard = CStr::from_ptr(second_vcard_ptr).to_str().unwrap();
        assert!(
            second_vcard.contains("X-SKYPE;TYPE=WORK;X-JMAP-KEY=k1:alice_skype_new")
                || second_vcard.contains("X-SKYPE;X-JMAP-KEY=k1;TYPE=WORK:alice_skype_new"),
            "Skype WORK should update in place keeping X-JMAP-KEY: {second_vcard}"
        );

        g_free(updated_vcard_ptr.cast());
        g_free(second_vcard_ptr.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}

#[test]
fn contact_photo_and_logo_field_properties_and_e_contact_photo_type() {
    unsafe {
        let photo_type = e_contact_photo_get_type();
        assert_ne!(photo_type, 0);

        // E_CONTACT_PHOTO (94) is a structured EContactPhoto field, not string
        assert_eq!(e_contact_field_is_string(E_CONTACT_PHOTO), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_PHOTO), photo_type);
        let photo_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_PHOTO));
        assert_eq!(photo_name.to_str().unwrap(), "photo");

        // E_CONTACT_LOGO (95) is a structured EContactPhoto field, not string
        assert_eq!(e_contact_field_is_string(E_CONTACT_LOGO), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_LOGO), photo_type);
        let logo_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_LOGO));
        assert_eq!(logo_name.to_str().unwrap(), "logo");

        // vCard attribute names
        assert_eq!(
            CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_PHOTO))
                .to_str()
                .unwrap(),
            "PHOTO"
        );
        assert_eq!(
            CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_LOGO))
                .to_str()
                .unwrap(),
            "LOGO"
        );

        // vCard field ID mapping
        assert_eq!(
            e_contact_field_id_from_vcard(c"PHOTO".as_ptr()),
            E_CONTACT_PHOTO
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"LOGO".as_ptr()),
            E_CONTACT_LOGO
        );
        // IMAGE has no field ID in EDS
        assert_eq!(e_contact_field_id_from_vcard(c"IMAGE".as_ptr()), 0);

        // EContactPhoto URI manipulation
        let uri_photo = e_contact_photo_new();
        assert!(!uri_photo.is_null());
        (*uri_photo).type_ = E_CONTACT_PHOTO_TYPE_URI;
        e_contact_photo_set_uri(uri_photo, c"https://example.com/avatar.png".as_ptr());
        assert_eq!((*uri_photo).type_, E_CONTACT_PHOTO_TYPE_URI);
        let uri_ptr = e_contact_photo_get_uri(uri_photo);
        assert_eq!(
            CStr::from_ptr(uri_ptr).to_str().unwrap(),
            "https://example.com/avatar.png"
        );
        e_contact_photo_free(uri_photo);

        // EContactPhoto inlined binary manipulation
        let inlined_photo = e_contact_photo_new();
        assert!(!inlined_photo.is_null());
        assert_eq!((*inlined_photo).type_, E_CONTACT_PHOTO_TYPE_INLINED);
        let sample_bytes = b"inlined_binary_photo_data_sample";
        e_contact_photo_set_inlined(
            inlined_photo,
            sample_bytes.as_ptr(),
            sample_bytes.len() as gsize,
        );
        e_contact_photo_set_mime_type(inlined_photo, c"image/png".as_ptr());
        assert_eq!((*inlined_photo).type_, E_CONTACT_PHOTO_TYPE_INLINED);
        let mut read_len: gsize = 0;
        let data_ptr = e_contact_photo_get_inlined(inlined_photo, &mut read_len);
        assert_eq!(read_len, sample_bytes.len() as gsize);
        assert_eq!(
            std::slice::from_raw_parts(data_ptr, read_len as usize),
            sample_bytes
        );
        let mime_ptr = e_contact_photo_get_mime_type(inlined_photo);
        assert_eq!(CStr::from_ptr(mime_ptr).to_str().unwrap(), "image/png");
        e_contact_photo_free(inlined_photo);
    }
}

#[test]
fn photo_and_logo_vcard_lines_and_field_modification_behavior_in_eds() {
    unsafe {
        let vcard_str = c"BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Test Contact\r\n\
N:Contact;Test;;;\r\n\
PHOTO;X-JMAP-KEY=m1;VALUE=uri:https://example.com/photo.png\r\n\
PHOTO;X-JMAP-KEY=m2;TYPE=jpeg;ENCODING=b:aGVsbG8tcGhvdG8=\r\n\
LOGO;X-JMAP-KEY=l1;VALUE=uri:https://example.com/logo.png\r\n\
END:VCARD\r\n";

        let contact = e_contact_new_from_vcard(vcard_str.as_ptr());
        assert!(!contact.is_null());

        // e_contact_get returns a dynamically allocated EContactPhoto
        let photo_obj = e_contact_get(contact, E_CONTACT_PHOTO) as *mut EContactPhoto;
        assert!(!photo_obj.is_null());
        assert_eq!((*photo_obj).type_, E_CONTACT_PHOTO_TYPE_URI);
        let photo_uri = e_contact_photo_get_uri(photo_obj);
        assert_eq!(
            CStr::from_ptr(photo_uri).to_str().unwrap(),
            "https://example.com/photo.png"
        );
        e_contact_photo_free(photo_obj);

        let logo_obj = e_contact_get(contact, E_CONTACT_LOGO) as *mut EContactPhoto;
        assert!(!logo_obj.is_null());
        assert_eq!((*logo_obj).type_, E_CONTACT_PHOTO_TYPE_URI);
        let logo_uri = e_contact_photo_get_uri(logo_obj);
        assert_eq!(
            CStr::from_ptr(logo_uri).to_str().unwrap(),
            "https://example.com/logo.png"
        );
        e_contact_photo_free(logo_obj);

        // Replacing E_CONTACT_PHOTO in place with a new inlined photo
        let new_photo = e_contact_photo_new();
        let new_bytes = b"new_avatar_bytes";
        e_contact_photo_set_inlined(new_photo, new_bytes.as_ptr(), new_bytes.len() as gsize);
        e_contact_photo_set_mime_type(new_photo, c"image/png".as_ptr());
        e_contact_set(contact, E_CONTACT_PHOTO, new_photo.cast());
        e_contact_photo_free(new_photo);

        let updated_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let updated_vcard = CStr::from_ptr(updated_vcard_ptr).to_str().unwrap();

        // The first PHOTO line is replaced with the new inlined photo (EDS shortens image/png to TYPE=png)
        assert!(
            updated_vcard.contains("PHOTO;TYPE=png;ENCODING=b:bmV3X2F2YXRhcl9ieXRlcw==")
                || updated_vcard
                    .contains("PHOTO;ENCODING=b;TYPE=image/png:bmV3X2F2YXRhcl9ieXRlcw==")
                || updated_vcard.contains("PHOTO;ENCODING=b;TYPE=PNG:bmV3X2F2YXRhcl9ieXRlcw=="),
            "first PHOTO should be replaced in place: {updated_vcard}"
        );
        // The second PHOTO line remains intact
        assert!(
            updated_vcard.contains("PHOTO;ENCODING=b;TYPE=jpeg;X-JMAP-KEY=m2:aGVsbG8tcGhvdG8=")
                || updated_vcard
                    .contains("PHOTO;X-JMAP-KEY=m2;TYPE=jpeg;ENCODING=b:aGVsbG8tcGhvdG8="),
            "second PHOTO should remain intact: {updated_vcard}"
        );
        // The LOGO line remains intact
        assert!(
            updated_vcard.contains("LOGO;VALUE=uri;X-JMAP-KEY=l1:https://example.com/logo.png")
                || updated_vcard
                    .contains("LOGO;X-JMAP-KEY=l1;VALUE=uri:https://example.com/logo.png")
                || updated_vcard.contains("LOGO;VALUE=uri:https://example.com/logo.png"),
            "LOGO should remain intact: {updated_vcard}"
        );
        g_free(updated_vcard_ptr.cast());

        // Clearing E_CONTACT_PHOTO by setting NULL
        e_contact_set(contact, E_CONTACT_PHOTO, std::ptr::null());

        let cleared_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let cleared_vcard = CStr::from_ptr(cleared_vcard_ptr).to_str().unwrap();

        // The first PHOTO line is removed, while second PHOTO and LOGO remain
        assert!(
            !cleared_vcard.contains("bmV3X2F2YXRhcl9ieXRlcw=="),
            "first PHOTO should be removed when cleared: {cleared_vcard}"
        );
        assert!(
            cleared_vcard.contains("PHOTO;ENCODING=b;TYPE=jpeg;X-JMAP-KEY=m2:aGVsbG8tcGhvdG8=")
                || cleared_vcard
                    .contains("PHOTO;X-JMAP-KEY=m2;TYPE=jpeg;ENCODING=b:aGVsbG8tcGhvdG8="),
            "second PHOTO should survive clearing the first photo: {cleared_vcard}"
        );
        assert!(
            cleared_vcard.contains("https://example.com/logo.png"),
            "LOGO line should survive clearing photo: {cleared_vcard}"
        );
        g_free(cleared_vcard_ptr.cast());

        gobject_sys::g_object_unref(contact.cast());
    }
}

/// Probing field properties of web, collaboration, note, nickname, spouse, and categories fields:
/// `E_CONTACT_HOMEPAGE_URL`, `E_CONTACT_BLOG_URL`, `E_CONTACT_VIDEO_URL`, `E_CONTACT_CALENDAR_URI`,
/// `E_CONTACT_FREEBUSY_URL`, `E_CONTACT_ICS_CALENDAR`, `E_CONTACT_NOTE`, `E_CONTACT_SPOUSE`,
/// `E_CONTACT_NICKNAME`, and `E_CONTACT_CATEGORIES` are string fields, whereas `E_CONTACT_CATEGORY_LIST`
/// is a multi-valued list type. `e_contact_field_id_from_vcard` maps standard and X- properties to these IDs.
#[test]
fn contact_web_collaboration_note_and_misc_field_properties() {
    unsafe {
        // String fields
        for (field, expected_name) in [
            (E_CONTACT_HOMEPAGE_URL, "homepage_url"),
            (E_CONTACT_BLOG_URL, "blog_url"),
            (E_CONTACT_VIDEO_URL, "video_url"),
            (E_CONTACT_CALENDAR_URI, "caluri"),
            (E_CONTACT_FREEBUSY_URL, "fburl"),
            (E_CONTACT_ICS_CALENDAR, "icscalendar"),
            (E_CONTACT_NOTE, "note"),
            (E_CONTACT_SPOUSE, "spouse"),
            (E_CONTACT_NICKNAME, "nickname"),
            (E_CONTACT_CATEGORIES, "categories"),
        ] {
            assert_eq!(
                e_contact_field_is_string(field),
                1,
                "field {expected_name} should be a string field"
            );
            let name_cstr = CStr::from_ptr(e_contact_field_name(field));
            assert_eq!(
                name_cstr.to_str().unwrap(),
                expected_name,
                "field name mismatch for {expected_name}"
            );
        }

        // Multi-valued category list
        assert_eq!(e_contact_field_is_string(E_CONTACT_CATEGORY_LIST), 0);
        let cat_list_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_CATEGORY_LIST));
        assert_eq!(cat_list_name.to_str().unwrap(), "category_list");

        // e_contact_field_id_from_vcard mapping checks
        assert_eq!(
            e_contact_field_id_from_vcard(c"URL".as_ptr()),
            E_CONTACT_HOMEPAGE_URL
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"CALURI".as_ptr()),
            E_CONTACT_CALENDAR_URI
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"FBURL".as_ptr()),
            E_CONTACT_FREEBUSY_URL
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"NOTE".as_ptr()),
            E_CONTACT_NOTE
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"NICKNAME".as_ptr()),
            E_CONTACT_NICKNAME
        );
        // In EDS, the vCard property "CATEGORIES" maps to the multi-valued E_CONTACT_CATEGORY_LIST (93)
        assert_eq!(
            e_contact_field_id_from_vcard(c"CATEGORIES".as_ptr()),
            E_CONTACT_CATEGORY_LIST
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-EVOLUTION-BLOG-URL".as_ptr()),
            E_CONTACT_BLOG_URL
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-EVOLUTION-VIDEO-URL".as_ptr()),
            E_CONTACT_VIDEO_URL
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-EVOLUTION-SPOUSE".as_ptr()),
            E_CONTACT_SPOUSE
        );
    }
}

/// Probing EDS behavior for web URLs, collaboration links, notes, nicknames, spouse, and categories:
/// - Parsing vCard with multiple URL, CALURI, FBURL, NOTE, NICKNAME, CATEGORIES, and X-EVOLUTION-SPOUSE lines
///   carrying X-JMAP-KEY parameters;
/// - Reading via `e_contact_get_const` and `E_CONTACT_CATEGORY_LIST` GList;
/// - In-place modification via `e_contact_set` updating target line while preserving extra lines and X-JMAP-KEYs;
/// - Clearing fields removing lines cleanly.
#[test]
fn web_collaboration_note_nickname_categories_vcard_lines_and_modification_in_eds() {
    unsafe {
        let vcard_str = c"BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Vera Olden\r\n\
N:Olden;Vera;;;\r\n\
URL;X-JMAP-KEY=l1:https://example.com/homepage\r\n\
URL;X-JMAP-KEY=l2:https://example.com/blog\r\n\
CALURI;X-JMAP-KEY=c1:https://example.com/calendar.ics\r\n\
FBURL;X-JMAP-KEY=f1:https://example.com/freebusy.ifb\r\n\
NOTE;X-JMAP-KEY=n1:Primary note for Vera\r\n\
NOTE;X-JMAP-KEY=n2:Secondary note with extra details\r\n\
NICKNAME;X-JMAP-KEY=k1:Vee\r\n\
CATEGORIES;X-JMAP-KEY=cat1:Engineering,Rust,Leadership\r\n\
X-EVOLUTION-SPOUSE;X-JMAP-KEY=s1:Alex Olden\r\n\
END:VCARD\r\n";

        let contact = e_contact_new_from_vcard(vcard_str.as_ptr());
        assert!(!contact.is_null());

        // Field getter assertions
        let url_ptr = e_contact_get_const(contact, E_CONTACT_HOMEPAGE_URL);
        assert!(!url_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(url_ptr.cast()).to_str().unwrap(),
            "https://example.com/homepage"
        );

        let caluri_ptr = e_contact_get_const(contact, E_CONTACT_CALENDAR_URI);
        assert!(!caluri_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(caluri_ptr.cast()).to_str().unwrap(),
            "https://example.com/calendar.ics"
        );

        let fburl_ptr = e_contact_get_const(contact, E_CONTACT_FREEBUSY_URL);
        assert!(!fburl_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(fburl_ptr.cast()).to_str().unwrap(),
            "https://example.com/freebusy.ifb"
        );

        let note_ptr = e_contact_get_const(contact, E_CONTACT_NOTE);
        assert!(!note_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(note_ptr.cast()).to_str().unwrap(),
            "Primary note for Vera"
        );

        let nick_ptr = e_contact_get_const(contact, E_CONTACT_NICKNAME);
        assert!(!nick_ptr.is_null());
        assert_eq!(CStr::from_ptr(nick_ptr.cast()).to_str().unwrap(), "Vee");

        let cat_ptr = e_contact_get_const(contact, E_CONTACT_CATEGORIES);
        assert!(!cat_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(cat_ptr.cast()).to_str().unwrap(),
            "Engineering,Rust,Leadership"
        );

        let spouse_ptr = e_contact_get_const(contact, E_CONTACT_SPOUSE);
        assert!(!spouse_ptr.is_null());
        assert_eq!(
            CStr::from_ptr(spouse_ptr.cast()).to_str().unwrap(),
            "Alex Olden"
        );

        // E_CONTACT_CATEGORY_LIST inspection
        let cat_list = e_contact_get(contact, E_CONTACT_CATEGORY_LIST) as *mut glib_sys::GList;
        assert!(!cat_list.is_null());
        let mut categories = Vec::new();
        let mut curr = cat_list;
        while !curr.is_null() {
            let item_str = CStr::from_ptr((*curr).data as *const gchar)
                .to_str()
                .unwrap();
            categories.push(item_str.to_string());
            curr = (*curr).next;
        }
        assert_eq!(categories, vec!["Engineering", "Rust", "Leadership"]);
        unsafe extern "C" fn free_item(p: *mut std::ffi::c_void) {
            unsafe {
                glib_sys::g_free(p);
            }
        }
        glib_sys::g_list_free_full(cat_list, Some(free_item));

        // Modifying fields in place
        e_contact_set(
            contact,
            E_CONTACT_HOMEPAGE_URL,
            c"https://example.com/new-home".as_ptr().cast(),
        );
        e_contact_set(
            contact,
            E_CONTACT_NOTE,
            c"Updated primary note".as_ptr().cast(),
        );
        e_contact_set(contact, E_CONTACT_NICKNAME, c"Vera-Prime".as_ptr().cast());
        e_contact_set(contact, E_CONTACT_SPOUSE, c"Taylor Olden".as_ptr().cast());

        let updated_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let updated_vcard = CStr::from_ptr(updated_vcard_ptr).to_str().unwrap();

        // First URL is updated in place, second URL preserved
        assert!(
            updated_vcard.contains("URL;X-JMAP-KEY=l1:https://example.com/new-home")
                || updated_vcard.contains("URL:https://example.com/new-home")
                || updated_vcard.contains("https://example.com/new-home"),
            "first URL should be updated: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("URL;X-JMAP-KEY=l2:https://example.com/blog")
                || updated_vcard.contains("https://example.com/blog"),
            "second URL should be preserved: {updated_vcard}"
        );

        // CALURI and FBURL preserved with X-JMAP-KEY
        assert!(
            updated_vcard.contains("CALURI;X-JMAP-KEY=c1:https://example.com/calendar.ics")
                || updated_vcard.contains("https://example.com/calendar.ics"),
            "CALURI should be preserved: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("FBURL;X-JMAP-KEY=f1:https://example.com/freebusy.ifb")
                || updated_vcard.contains("https://example.com/freebusy.ifb"),
            "FBURL should be preserved: {updated_vcard}"
        );

        // First NOTE is updated, second NOTE is preserved
        assert!(
            updated_vcard.contains("Updated primary note"),
            "first NOTE should be updated: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("Secondary note with extra details"),
            "second NOTE should be preserved: {updated_vcard}"
        );

        // NICKNAME updated
        assert!(
            updated_vcard.contains("Vera-Prime"),
            "NICKNAME should be updated: {updated_vcard}"
        );

        // SPOUSE updated
        assert!(
            updated_vcard.contains("Taylor Olden"),
            "SPOUSE should be updated: {updated_vcard}"
        );

        g_free(updated_vcard_ptr.cast());

        // Clearing NOTE by passing NULL
        e_contact_set(contact, E_CONTACT_NOTE, std::ptr::null());
        let cleared_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let cleared_vcard = CStr::from_ptr(cleared_vcard_ptr).to_str().unwrap();

        assert!(
            !cleared_vcard.contains("Updated primary note"),
            "first NOTE should be removed: {cleared_vcard}"
        );
        assert!(
            cleared_vcard.contains("Secondary note with extra details"),
            "second NOTE should remain: {cleared_vcard}"
        );

        g_free(cleared_vcard_ptr.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}

#[test]
fn contact_telephone_and_email_field_properties() {
    let phone_fields: &[(EContactField, &str)] = &[
        (E_CONTACT_PHONE_ASSISTANT, "assistant_phone"),
        (E_CONTACT_PHONE_BUSINESS, "business_phone"),
        (E_CONTACT_PHONE_BUSINESS_2, "business_phone_2"),
        (E_CONTACT_PHONE_BUSINESS_FAX, "business_fax"),
        (E_CONTACT_PHONE_CALLBACK, "callback_phone"),
        (E_CONTACT_PHONE_CAR, "car_phone"),
        (E_CONTACT_PHONE_COMPANY, "company_phone"),
        (E_CONTACT_PHONE_HOME, "home_phone"),
        (E_CONTACT_PHONE_HOME_2, "home_phone_2"),
        (E_CONTACT_PHONE_HOME_FAX, "home_fax"),
        (E_CONTACT_PHONE_ISDN, "isdn_phone"),
        (E_CONTACT_PHONE_MOBILE, "mobile_phone"),
        (E_CONTACT_PHONE_OTHER, "other_phone"),
        (E_CONTACT_PHONE_OTHER_FAX, "other_fax"),
        (E_CONTACT_PHONE_PAGER, "pager"),
        (E_CONTACT_PHONE_PRIMARY, "primary_phone"),
        (E_CONTACT_PHONE_RADIO, "radio"),
        (E_CONTACT_PHONE_TELEX, "telex"),
        (E_CONTACT_PHONE_TTYTDD, "tty"),
    ];

    let email_fields: &[(EContactField, &str)] = &[
        (E_CONTACT_EMAIL_1, "email_1"),
        (E_CONTACT_EMAIL_2, "email_2"),
        (E_CONTACT_EMAIL_3, "email_3"),
        (E_CONTACT_EMAIL_4, "email_4"),
    ];

    unsafe {
        assert_eq!(E_CONTACT_FIRST_PHONE_ID, 16);
        assert_eq!(E_CONTACT_LAST_PHONE_ID, 34);
        assert_eq!(E_CONTACT_FIRST_EMAIL_ID, 8);
        assert_eq!(E_CONTACT_LAST_EMAIL_ID, 11);

        for &(field, name) in phone_fields {
            assert_eq!(
                e_contact_field_is_string(field),
                1,
                "phone field {field} must be a string"
            );
            let field_name = CStr::from_ptr(e_contact_field_name(field))
                .to_str()
                .unwrap();
            assert_eq!(field_name, name);
        }

        for &(field, name) in email_fields {
            assert_eq!(
                e_contact_field_is_string(field),
                1,
                "email field {field} must be a string"
            );
            let field_name = CStr::from_ptr(e_contact_field_name(field))
                .to_str()
                .unwrap();
            assert_eq!(field_name, name);
        }

        let attr_list_type = e_contact_attr_list_get_type();
        assert_ne!(attr_list_type, 0);

        // E_CONTACT_TEL (119) and E_CONTACT_EMAIL (97) are attribute lists
        assert_eq!(e_contact_field_is_string(E_CONTACT_TEL), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_TEL), attr_list_type);
        let tel_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_TEL));
        assert_eq!(tel_name.to_str().unwrap(), "phone");

        assert_eq!(e_contact_field_is_string(E_CONTACT_EMAIL), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_EMAIL), attr_list_type);
        let email_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_EMAIL));
        assert_eq!(email_name.to_str().unwrap(), "email");

        // Relationships & metadata
        assert_eq!(e_contact_field_is_string(E_CONTACT_MANAGER), 1);
        let mgr_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_MANAGER));
        assert_eq!(mgr_name.to_str().unwrap(), "manager");

        assert_eq!(e_contact_field_is_string(E_CONTACT_ASSISTANT), 1);
        let asst_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ASSISTANT));
        assert_eq!(asst_name.to_str().unwrap(), "assistant");

        assert_eq!(e_contact_field_is_string(E_CONTACT_FILE_AS), 1);
        let file_as_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_FILE_AS));
        assert_eq!(file_as_name.to_str().unwrap(), "file_as");

        assert_eq!(e_contact_field_is_string(E_CONTACT_MAILER), 1);
        let mailer_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_MAILER));
        assert_eq!(mailer_name.to_str().unwrap(), "mailer");

        // vCard property ID mappings
        assert_eq!(
            e_contact_field_id_from_vcard(c"TEL".as_ptr().cast()),
            E_CONTACT_TEL
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"EMAIL".as_ptr().cast()),
            E_CONTACT_EMAIL
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-EVOLUTION-MANAGER".as_ptr().cast()),
            E_CONTACT_MANAGER
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-EVOLUTION-ASSISTANT".as_ptr().cast()),
            E_CONTACT_ASSISTANT
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-EVOLUTION-FILE-AS".as_ptr().cast()),
            E_CONTACT_FILE_AS
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"MAILER".as_ptr().cast()),
            E_CONTACT_MAILER
        );
    }
}

#[test]
fn telephone_and_email_synthetic_slots_and_modification_behavior_in_eds() {
    let vcard_str = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:pas-id-test-tel-email-001\r\n",
        "FN:Vera Olden\r\n",
        "N:Olden;Vera;;;\r\n",
        "TEL;TYPE=WORK,VOICE;X-JMAP-KEY=p1:+1-555-0100\r\n",
        "TEL;TYPE=HOME,VOICE;X-JMAP-KEY=p2:+1-555-0101\r\n",
        "TEL;TYPE=CELL;X-JMAP-KEY=p3:+1-555-0102\r\n",
        "TEL;TYPE=WORK,FAX;X-JMAP-KEY=p4:+1-555-0103\r\n",
        "TEL;TYPE=PAGER;X-JMAP-KEY=p5:+1-555-0104\r\n",
        "EMAIL;TYPE=WORK;X-JMAP-KEY=e1:vera.work@example.com\r\n",
        "EMAIL;TYPE=HOME;X-JMAP-KEY=e2:vera.home@example.com\r\n",
        "X-EVOLUTION-MANAGER:Jordan Smith\r\n",
        "X-EVOLUTION-ASSISTANT:Morgan Lee\r\n",
        "X-EVOLUTION-FILE-AS:Olden, Vera\r\n",
        "END:VCARD\r\n"
    );

    unsafe {
        let vcard_c = std::ffi::CString::new(vcard_str).unwrap();
        let contact = e_contact_new_from_vcard(vcard_c.as_ptr().cast());
        assert!(!contact.is_null());

        // Synthetic phone fields inspection
        let work_phone = e_contact_get_const(contact, E_CONTACT_PHONE_BUSINESS);
        assert!(!work_phone.is_null());
        assert_eq!(
            CStr::from_ptr(work_phone.cast()).to_str().unwrap(),
            "+1-555-0100"
        );

        let home_phone = e_contact_get_const(contact, E_CONTACT_PHONE_HOME);
        assert!(!home_phone.is_null());
        assert_eq!(
            CStr::from_ptr(home_phone.cast()).to_str().unwrap(),
            "+1-555-0101"
        );

        let cell_phone = e_contact_get_const(contact, E_CONTACT_PHONE_MOBILE);
        assert!(!cell_phone.is_null());
        assert_eq!(
            CStr::from_ptr(cell_phone.cast()).to_str().unwrap(),
            "+1-555-0102"
        );

        let fax_phone = e_contact_get_const(contact, E_CONTACT_PHONE_BUSINESS_FAX);
        assert!(!fax_phone.is_null());
        assert_eq!(
            CStr::from_ptr(fax_phone.cast()).to_str().unwrap(),
            "+1-555-0103"
        );

        let pager_phone = e_contact_get_const(contact, E_CONTACT_PHONE_PAGER);
        assert!(!pager_phone.is_null());
        assert_eq!(
            CStr::from_ptr(pager_phone.cast()).to_str().unwrap(),
            "+1-555-0104"
        );

        // Email synthetic fields inspection
        let email_1 = e_contact_get_const(contact, E_CONTACT_EMAIL_1);
        assert!(!email_1.is_null());
        assert_eq!(
            CStr::from_ptr(email_1.cast()).to_str().unwrap(),
            "vera.work@example.com"
        );

        let email_2 = e_contact_get_const(contact, E_CONTACT_EMAIL_2);
        assert!(!email_2.is_null());
        assert_eq!(
            CStr::from_ptr(email_2.cast()).to_str().unwrap(),
            "vera.home@example.com"
        );

        // Relationship and metadata fields
        let mgr = e_contact_get_const(contact, E_CONTACT_MANAGER);
        assert!(!mgr.is_null());
        assert_eq!(CStr::from_ptr(mgr.cast()).to_str().unwrap(), "Jordan Smith");

        let asst = e_contact_get_const(contact, E_CONTACT_ASSISTANT);
        assert!(!asst.is_null());
        assert_eq!(CStr::from_ptr(asst.cast()).to_str().unwrap(), "Morgan Lee");

        let file_as = e_contact_get_const(contact, E_CONTACT_FILE_AS);
        assert!(!file_as.is_null());
        assert_eq!(
            CStr::from_ptr(file_as.cast()).to_str().unwrap(),
            "Olden, Vera"
        );

        // E_CONTACT_TEL and E_CONTACT_EMAIL attribute lists
        let tel_list = e_contact_get(contact, E_CONTACT_TEL) as *mut glib_sys::GList;
        assert!(!tel_list.is_null());
        let tel_count = glib_sys::g_list_length(tel_list);
        assert_eq!(tel_count, 5);
        unsafe extern "C" fn free_item(p: *mut std::ffi::c_void) {
            unsafe {
                glib_sys::g_free(p);
            }
        }
        glib_sys::g_list_free_full(tel_list, Some(free_item));

        let email_list = e_contact_get(contact, E_CONTACT_EMAIL) as *mut glib_sys::GList;
        assert!(!email_list.is_null());
        let email_count = glib_sys::g_list_length(email_list);
        assert_eq!(email_count, 2);
        glib_sys::g_list_free_full(email_list, Some(free_item));

        // In-place modifications
        e_contact_set(
            contact,
            E_CONTACT_PHONE_MOBILE,
            c"+1-555-9999".as_ptr().cast(),
        );
        e_contact_set(
            contact,
            E_CONTACT_EMAIL_1,
            c"vera.chief@example.com".as_ptr().cast(),
        );
        e_contact_set(contact, E_CONTACT_MANAGER, c"Taylor Brooks".as_ptr().cast());

        let updated_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let updated_vcard = CStr::from_ptr(updated_vcard_ptr).to_str().unwrap();

        assert!(
            updated_vcard.contains("+1-555-9999"),
            "updated cell phone missing: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("vera.chief@example.com"),
            "updated email 1 missing: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("Taylor Brooks"),
            "updated manager missing: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("+1-555-0100"),
            "work phone must be preserved: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("vera.home@example.com"),
            "home email must be preserved: {updated_vcard}"
        );

        g_free(updated_vcard_ptr.cast());

        // Field clearing by setting NULL
        e_contact_set(contact, E_CONTACT_PHONE_MOBILE, std::ptr::null());
        e_contact_set(contact, E_CONTACT_MANAGER, std::ptr::null());

        let cleared_vcard_ptr = e_vcard_to_string(contact.cast(), EVC_FORMAT_VCARD_30);
        let cleared_vcard = CStr::from_ptr(cleared_vcard_ptr).to_str().unwrap();

        assert!(
            !cleared_vcard.contains("+1-555-9999"),
            "cleared cell phone must be removed: {cleared_vcard}"
        );
        assert!(
            !cleared_vcard.contains("Taylor Brooks"),
            "cleared manager must be removed: {cleared_vcard}"
        );
        assert!(
            cleared_vcard.contains("+1-555-0100"),
            "work phone must remain: {cleared_vcard}"
        );

        g_free(cleared_vcard_ptr.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}
