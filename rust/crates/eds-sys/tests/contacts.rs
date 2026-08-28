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

use std::ffi::{CStr, CString};

use eds_sys::*;
// Not glob-imported: the point of these two is that the call site does not
// have to know which spelling this EDS uses (see `eds_sys::compat`), and
// naming them is what keeps that visible here.
use eds_sys::compat::{e_contact_date_to_string_vcard_30, e_vcard_to_string_vcard_30};

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
    // 3.60 resolves every slotted IM service's `X-` line to the first
    // `_HOME_1` slot instead of the plain attribute-list field 3.52 uses — a
    // libebook behaviour change with no header-visible signal of its own
    // (both symbols exist on both releases). Measured directly against both
    // legs (this uncovered more of the same drift than
    // `docs/eds-version-matrix.md` (B) had recorded — it only sampled
    // JABBER/AIM/GADUGADU, but the change is systemic to every slotted
    // service). `E_CONTACT_SIP`/`E_CONTACT_IM_TWITTER` have no slots and stay
    // unaffected on both. See `eds_death_date_field`'s doc comment in
    // `build.rs` for why that unrelated-looking cfg is the pivot here too.
    macro_rules! im_field {
        ($unslotted:ident, $home1:ident) => {
            if cfg!(eds_death_date_field) {
                $home1
            } else {
                $unslotted
            }
        };
    }
    let jabber = im_field!(E_CONTACT_IM_JABBER, E_CONTACT_IM_JABBER_HOME_1);
    let aim = im_field!(E_CONTACT_IM_AIM, E_CONTACT_IM_AIM_HOME_1);
    let gadugadu = im_field!(E_CONTACT_IM_GADUGADU, E_CONTACT_IM_GADUGADU_HOME_1);
    let skype = im_field!(E_CONTACT_IM_SKYPE, E_CONTACT_IM_SKYPE_HOME_1);
    let matrix = im_field!(E_CONTACT_IM_MATRIX, E_CONTACT_IM_MATRIX_HOME_1);
    let icq = im_field!(E_CONTACT_IM_ICQ, E_CONTACT_IM_ICQ_HOME_1);
    let msn = im_field!(E_CONTACT_IM_MSN, E_CONTACT_IM_MSN_HOME_1);
    let yahoo = im_field!(E_CONTACT_IM_YAHOO, E_CONTACT_IM_YAHOO_HOME_1);
    let google_talk = im_field!(E_CONTACT_IM_GOOGLE_TALK, E_CONTACT_IM_GOOGLE_TALK_HOME_1);
    let groupwise = im_field!(E_CONTACT_IM_GROUPWISE, E_CONTACT_IM_GROUPWISE_HOME_1);

    unsafe {
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-SIP".as_ptr()),
            E_CONTACT_SIP
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-TWITTER".as_ptr()),
            E_CONTACT_IM_TWITTER
        );
        assert_eq!(e_contact_field_id_from_vcard(c"X-JABBER".as_ptr()), jabber);
        assert_eq!(e_contact_field_id_from_vcard(c"X-AIM".as_ptr()), aim);
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-GADUGADU".as_ptr()),
            gadugadu
        );
        assert_eq!(e_contact_field_id_from_vcard(c"X-SKYPE".as_ptr()), skype);
        assert_eq!(e_contact_field_id_from_vcard(c"X-MATRIX".as_ptr()), matrix);
        assert_eq!(e_contact_field_id_from_vcard(c"X-ICQ".as_ptr()), icq);
        assert_eq!(e_contact_field_id_from_vcard(c"X-MSN".as_ptr()), msn);
        assert_eq!(e_contact_field_id_from_vcard(c"X-YAHOO".as_ptr()), yahoo);
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-GOOGLE-TALK".as_ptr()),
            google_talk
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-GROUPWISE".as_ptr()),
            groupwise
        );
    }
}

#[test]
fn contact_date_fields_are_structured_e_contact_date_types() {
    // 3.60 swaps which vCard line maps to `E_CONTACT_ANNIVERSARY`
    // (`ANNIVERSARY` instead of `X-EVOLUTION-ANNIVERSARY`) and adds a
    // dedicated `DEATHDATE` field EDS 3.52 has no field ID for at all.
    // `E_CONTACT_DEATHDATE` does not exist in 3.52's `EContactField` enum,
    // so it can only be named behind the cfg that detects it — see that cfg's
    // doc comment in `build.rs` for why the anniversary swap piggybacks on
    // the same signal. Measured on both legs in
    // `docs/eds-version-matrix.md` (B).
    #[cfg(eds_death_date_field)]
    let (anniversary_line, evo_anniversary_line, deathdate_line) =
        (E_CONTACT_ANNIVERSARY, 0, E_CONTACT_DEATHDATE);
    #[cfg(not(eds_death_date_field))]
    let (anniversary_line, evo_anniversary_line, deathdate_line) = (0, E_CONTACT_ANNIVERSARY, 0);

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

        // BDAY always resolves to its field ID
        assert_eq!(
            e_contact_field_id_from_vcard(c"BDAY".as_ptr()),
            E_CONTACT_BIRTH_DATE
        );

        // ANNIVERSARY / X-EVOLUTION-ANNIVERSARY: exactly one resolves to
        // E_CONTACT_ANNIVERSARY, the other is unmodeled (field ID 0).
        assert_eq!(
            e_contact_field_id_from_vcard(c"ANNIVERSARY".as_ptr()),
            anniversary_line
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"X-EVOLUTION-ANNIVERSARY".as_ptr()),
            evo_anniversary_line
        );

        // DEATHDATE has a field ID only where E_CONTACT_DEATHDATE exists;
        // X-DEATHDATE stays unmodeled on both.
        assert_eq!(
            e_contact_field_id_from_vcard(c"DEATHDATE".as_ptr()),
            deathdate_line
        );
        assert_eq!(e_contact_field_id_from_vcard(c"X-DEATHDATE".as_ptr()), 0);
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

        let formatted = e_contact_date_to_string_vcard_30(date);
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
fn e_contact_date_bare_year_and_partial_date_clamping() {
    unsafe {
        // Bare 4-digit year: VERSION-DEPENDENT. EDS 3.52's
        // e_contact_date_from_string fails to parse short strings (< 8 chars
        // or non-ISO) and returns an empty EContactDate (year=0, month=0,
        // day=0); newer EDS (observed on the CI matrix's Fedora leg,
        // 2026-08-28) parses the bare year. Both are characterized; what the
        // invariant pins is that it is one of exactly these two, never a
        // garbage year, and month/day stay 0 either way.
        let date = e_contact_date_from_string(c"1984".as_ptr());
        assert!(!date.is_null());
        assert!(
            (*date).year == 0 || (*date).year == 1984,
            "bare-year parse must yield 0 (EDS 3.52) or 1984 (newer), got {}",
            (*date).year
        );
        assert_eq!((*date).month, 0);
        assert_eq!((*date).day, 0);

        let formatted = e_contact_date_to_string_vcard_30(date);
        assert!(!formatted.is_null());
        // CLAMP forces every 0 part into printable range (year -> 1000,
        // month -> 1, day -> 1): the unparsed arm corrupts 1984 into
        // 1000-01-01, the parsed arm keeps the year and invents the rest.
        let formatted_str = CStr::from_ptr(formatted).to_str().unwrap();
        assert!(
            formatted_str == "1000-01-01" || formatted_str == "1984-01-01",
            "clamped bare year must print 1000-01-01 (EDS 3.52) or 1984-01-01 (newer): {formatted_str}"
        );
        g_free(formatted.cast());
        e_contact_date_free(date);

        // Year-month: "1984-05" — same version split; a parsing EDS may keep
        // the month too, so the parsed arm only pins that the year survived.
        let ym = e_contact_date_from_string(c"1984-05".as_ptr());
        assert!(!ym.is_null());
        assert!(
            (*ym).year == 0 || (*ym).year == 1984,
            "year-month parse must yield year 0 (EDS 3.52) or 1984 (newer), got {}",
            (*ym).year
        );
        let formatted_ym_raw = e_contact_date_to_string_vcard_30(ym);
        let formatted_ym = CStr::from_ptr(formatted_ym_raw).to_str().unwrap();
        assert!(
            formatted_ym == "1000-01-01" || formatted_ym.starts_with("1984-"),
            "clamped year-month must print 1000-01-01 (EDS 3.52) or keep 1984 (newer): {formatted_ym}"
        );
        g_free(formatted_ym_raw.cast());
        e_contact_date_free(ym);

        // Year-less: "--05-20" — same split: an EDS that parses it keeps the
        // month and day and clamps only the year.
        let yless = e_contact_date_from_string(c"--05-20".as_ptr());
        assert!(!yless.is_null());
        assert_eq!((*yless).year, 0, "no EDS invents a year for --05-20");
        let formatted_yless_raw = e_contact_date_to_string_vcard_30(yless);
        let formatted_yless = CStr::from_ptr(formatted_yless_raw).to_str().unwrap();
        assert!(
            formatted_yless == "1000-01-01" || formatted_yless == "1000-05-20",
            "clamped year-less date must print 1000-01-01 (EDS 3.52) or 1000-05-20 (newer): {formatted_yless}"
        );
        g_free(formatted_yless_raw.cast());
        e_contact_date_free(yless);
    }
}

#[test]
fn contact_editor_bday_and_anniversary_bare_year_in_place_clamping_corruption() {
    // Characterizes Evolution Contact Editor lifecycle on bare-year dates:
    // When a vCard arrives with BDAY:1984 and X-EVOLUTION-ANNIVERSARY:1996,
    // EVCard keeps the raw attribute string. But the moment Evolution's contact
    // editor loads the fields via e_contact_get() and saves them back via
    // e_contact_set(), EDS clamps year=0 to 1000, month=0 to 1, and day=0 to 1,
    // corrupting BDAY:1984 into BDAY:1000-01-01 and Anniversary to 1000-01-01!
    unsafe {
        let vcard_raw = c"BEGIN:VCARD\r\nVERSION:3.0\r\nUID:c1\r\nFN:Alice\r\nBDAY:1984\r\nX-EVOLUTION-ANNIVERSARY:1996\r\nEND:VCARD\r\n";
        let contact = e_contact_new_from_vcard(vcard_raw.as_ptr());
        assert!(!contact.is_null());

        // Untouched vCard string contains raw lines
        let untouched = e_vcard_to_string_vcard_30(contact.cast());
        assert!(
            CStr::from_ptr(untouched)
                .to_str()
                .unwrap()
                .contains("BDAY:1984")
        );
        assert!(
            CStr::from_ptr(untouched)
                .to_str()
                .unwrap()
                .contains("X-EVOLUTION-ANNIVERSARY:1996")
        );
        g_free(untouched.cast());

        // Contact editor reading: VERSION-DEPENDENT, same split as
        // `e_contact_date_bare_year_and_partial_date_clamping` above — EDS
        // 3.52 yields the empty date (year=0), newer EDS parses the bare
        // year. The corruption this test exists to pin is unchanged either
        // way: month/day never invent values.
        let bday = e_contact_get(contact, E_CONTACT_BIRTH_DATE).cast::<EContactDate>();
        assert!(!bday.is_null());
        assert!(
            (*bday).year == 0 || (*bday).year == 1984,
            "bare-year BDAY must read 0 (EDS 3.52) or 1984 (newer), got {}",
            (*bday).year
        );
        assert_eq!((*bday).month, 0);
        assert_eq!((*bday).day, 0);

        let anniv = e_contact_get(contact, E_CONTACT_ANNIVERSARY).cast::<EContactDate>();
        assert!(!anniv.is_null());
        assert!(
            (*anniv).year == 0 || (*anniv).year == 1996,
            "bare-year anniversary must read 0 (EDS 3.52) or 1996 (newer), got {}",
            (*anniv).year
        );
        assert_eq!((*anniv).month, 0);
        assert_eq!((*anniv).day, 0);

        // Contact editor saving: e_contact_set invokes e_contact_date_to_string,
        // which clamps the 0 values to 1000-01-01
        e_contact_set(contact, E_CONTACT_BIRTH_DATE, bday.cast());
        e_contact_set(contact, E_CONTACT_ANNIVERSARY, anniv.cast());

        let rewritten = e_vcard_to_string_vcard_30(contact.cast());
        let rewritten_str = CStr::from_ptr(rewritten).to_str().unwrap();
        // Either way the round trip invents a month and a day; the year is
        // the version-dependent half — 3.52's unparsed 0 clamps to 1000,
        // newer EDS keeps the parsed year.
        assert!(
            rewritten_str.contains("BDAY:1000-01-01") || rewritten_str.contains("BDAY:1984-01-01"),
            "bare year BDAY must clamp to 1000-01-01 (EDS 3.52) or 1984-01-01 (newer): {rewritten_str}"
        );
        assert!(
            rewritten_str.contains("X-EVOLUTION-ANNIVERSARY:1000-01-01")
                || rewritten_str.contains("X-EVOLUTION-ANNIVERSARY:1996-01-01"),
            "bare year anniversary must clamp to 1000-01-01 (EDS 3.52) or 1996-01-01 (newer): {rewritten_str}"
        );
        g_free(rewritten.cast());

        e_contact_date_free(bday);
        e_contact_date_free(anniv);
        g_object_unref(contact.cast());
    }
}

#[test]
fn a_date_before_the_year_1000_is_written_back_as_the_year_1000() {
    // `e_contact_date_to_string()` CLAMPs each part into the range it can
    // print: the year to 1000..=9999, the month to 1..=12, the day to 1..=31.
    // Reading is not clamped, so the round trip is lossy in one direction
    // only, and silently.
    //
    // This is why `jmap-vcard` states no date line for a year under 1000: the
    // month and the day survive, the millennium does not.
    unsafe {
        for (text, written) in [
            ("0800-06-21", "1000-06-21"),
            ("0999-12-31", "1000-12-31"),
            ("0001-01-01", "1000-01-01"),
            // The first year it can state, and the last, both unchanged.
            ("1000-01-01", "1000-01-01"),
            ("9999-12-31", "9999-12-31"),
        ] {
            let stated = CString::new(text).expect("no interior NUL");
            let parsed = e_contact_date_from_string(stated.as_ptr());
            assert!(!parsed.is_null(), "{text} did not parse");
            let formatted = e_contact_date_to_string_vcard_30(parsed);
            assert!(!formatted.is_null());
            assert_eq!(
                CStr::from_ptr(formatted).to_str().unwrap(),
                written,
                "{text} was written back as something else"
            );
            g_free(formatted.cast());
            e_contact_date_free(parsed);
        }
    }
}

#[test]
fn setting_a_birthday_before_the_year_1000_rewrites_the_bday_line() {
    // The clamp reaching a whole card, which is the shape the hazard actually
    // takes: a line merely passing through keeps the year it arrived with,
    // because `EVCard` hands back the attribute it parsed. It is *setting* the
    // field — which the contact editor does to every field it shows, every
    // time the user presses Save — that rebuilds the line from the clamped
    // numbers.
    unsafe {
        let contact = e_contact_new_from_vcard(
            c"BEGIN:VCARD\r\nVERSION:3.0\r\nUID:k8\r\nFN:Karl\r\nBDAY:0800-06-21\r\nEND:VCARD\r\n"
                .as_ptr(),
        );

        let untouched = e_vcard_to_string_vcard_30(contact.cast());
        assert!(
            CStr::from_ptr(untouched)
                .to_str()
                .unwrap()
                .contains("BDAY:0800-06-21"),
            "an untouched line was rewritten"
        );
        g_free(untouched.cast());

        // What the editor does: read the field out and put it back.
        let date = e_contact_get(contact, E_CONTACT_BIRTH_DATE).cast::<EContactDate>();
        assert!(!date.is_null());
        assert_eq!((*date).year, 800, "reading is not clamped");
        e_contact_set(contact, E_CONTACT_BIRTH_DATE, date.cast());

        let rewritten = e_vcard_to_string_vcard_30(contact.cast());
        assert!(
            CStr::from_ptr(rewritten)
                .to_str()
                .unwrap()
                .contains("BDAY:1000-06-21"),
            "the clamp did not reach the line: {}",
            CStr::from_ptr(rewritten).to_str().unwrap()
        );
        g_free(rewritten.cast());

        e_contact_date_free(date);
        g_object_unref(contact.cast());
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
        let updated_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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
        let updated_org_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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
        let updated_role_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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

/// How far into an `ORG` value EDS has fields, and what happens to the rest.
///
/// The `ORG` line states an employer and then its units, and EDS gives the
/// first three components a field each: `E_CONTACT_ORG` (the company),
/// `E_CONTACT_ORG_UNIT` (the department) and `E_CONTACT_OFFICE`. A fourth
/// component has no field at all — but it is not lost either, because a `set`
/// rewrites the one component it is the field for and leaves the value's other
/// components exactly where they were, empties included.
///
/// Which is why `jmap-book-sync`'s `merge_units` may take the edited list at
/// its word: every unit with a name is written onto the line, and every unit on
/// the line comes back, whether EDS had a field to show it in or not.
#[test]
fn an_org_component_past_the_third_has_no_field_but_survives_an_edit_of_the_others() {
    unsafe {
        let vcard = c"BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Test Contact\r\n\
N:Contact;Test;;;\r\n\
ORG;X-JMAP-KEY=o1:Acme Ltd;Research;Optics;Lenses\r\n\
END:VCARD\r\n";
        let contact = e_contact_new_from_vcard(vcard.as_ptr());
        assert!(!contact.is_null());

        // The first three components have a field each, in that order.
        for (field, expected) in [
            (E_CONTACT_ORG, "Acme Ltd"),
            (E_CONTACT_ORG_UNIT, "Research"),
            (E_CONTACT_OFFICE, "Optics"),
        ] {
            let value = e_contact_get_const(contact, field);
            assert!(!value.is_null(), "no value for field {field}");
            assert_eq!(CStr::from_ptr(value.cast()).to_str().unwrap(), expected);
        }
        // `Lenses` is in none of them: no field states the fourth component.
        for field in [E_CONTACT_ORG, E_CONTACT_ORG_UNIT, E_CONTACT_OFFICE] {
            let value = e_contact_get_const(contact, field);
            assert_ne!(CStr::from_ptr(value.cast()).to_str().unwrap(), "Lenses");
        }

        // Editing the department rewrites that component and no other.
        e_contact_set(contact, E_CONTACT_ORG_UNIT, c"Acoustics".as_ptr().cast());
        let edited_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let edited = CStr::from_ptr(edited_ptr).to_str().unwrap();
        assert!(
            edited.contains("ORG;X-JMAP-KEY=o1:Acme Ltd;Acoustics;Optics;Lenses"),
            "the components EDS did not edit moved or vanished: {edited}"
        );
        g_free(edited_ptr.cast());

        // Clearing a field empties its component in place rather than closing
        // the gap, so the components after it keep their positions.
        e_contact_set(contact, E_CONTACT_OFFICE, std::ptr::null());
        let cleared_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let cleared = CStr::from_ptr(cleared_ptr).to_str().unwrap();
        assert!(
            cleared.contains("ORG;X-JMAP-KEY=o1:Acme Ltd;Acoustics;;Lenses"),
            "clearing the office shifted the components after it: {cleared}"
        );
        g_free(cleared_ptr.cast());

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

        let updated_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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

        let updated_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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
        let second_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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

        let updated_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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

        let cleared_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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

        let updated_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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
        let cleared_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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

        let updated_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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

        let cleared_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
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

#[test]
fn contact_structured_name_geo_cert_and_boolean_field_properties() {
    unsafe {
        let name_type = e_contact_name_get_type();
        assert_ne!(name_type, 0);

        let geo_type = e_contact_geo_get_type();
        assert_ne!(geo_type, 0);

        let cert_type = e_contact_cert_get_type();
        assert_ne!(cert_type, 0);

        // Name fields
        assert_eq!(e_contact_field_is_string(E_CONTACT_FULL_NAME), 1);
        let fn_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_FULL_NAME));
        assert_eq!(fn_name.to_str().unwrap(), "full_name");

        assert_eq!(e_contact_field_is_string(E_CONTACT_GIVEN_NAME), 1);
        let given_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_GIVEN_NAME));
        assert_eq!(given_name.to_str().unwrap(), "given_name");

        assert_eq!(e_contact_field_is_string(E_CONTACT_FAMILY_NAME), 1);
        let family_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_FAMILY_NAME));
        assert_eq!(family_name.to_str().unwrap(), "family_name");

        assert_eq!(e_contact_field_is_string(E_CONTACT_NAME), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_NAME), name_type);
        let name_field = CStr::from_ptr(e_contact_field_name(E_CONTACT_NAME));
        assert_eq!(name_field.to_str().unwrap(), "name");

        // Geo field
        assert_eq!(e_contact_field_is_string(E_CONTACT_GEO), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_GEO), geo_type);
        let geo_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_GEO));
        assert_eq!(geo_name.to_str().unwrap(), "geo");

        // Certificate fields
        assert_eq!(e_contact_field_is_string(E_CONTACT_X509_CERT), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_X509_CERT), cert_type);
        let x509_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_X509_CERT));
        assert_eq!(x509_name.to_str().unwrap(), "x509Cert");

        assert_eq!(e_contact_field_is_string(E_CONTACT_PGP_CERT), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_PGP_CERT), cert_type);
        let pgp_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_PGP_CERT));
        assert_eq!(pgp_name.to_str().unwrap(), "pgpCert");

        // Boolean and metadata fields
        assert_eq!(e_contact_field_is_string(E_CONTACT_WANTS_HTML), 0);
        let wants_html_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_WANTS_HTML));
        assert_eq!(wants_html_name.to_str().unwrap(), "wants_html");

        assert_eq!(e_contact_field_is_string(E_CONTACT_IS_LIST), 0);
        let is_list_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_IS_LIST));
        assert_eq!(is_list_name.to_str().unwrap(), "list");

        assert_eq!(e_contact_field_is_string(E_CONTACT_LIST_SHOW_ADDRESSES), 0);
        let list_show_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_LIST_SHOW_ADDRESSES));
        assert_eq!(list_show_name.to_str().unwrap(), "list_show_addresses");

        assert_eq!(e_contact_field_is_string(E_CONTACT_REV), 1);
        let rev_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_REV));
        assert_eq!(rev_name.to_str().unwrap(), "Rev");

        assert_eq!(e_contact_field_is_string(E_CONTACT_NAME_OR_ORG), 1);
        let name_or_org_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_NAME_OR_ORG));
        assert_eq!(name_or_org_name.to_str().unwrap(), "name_or_org");

        assert_eq!(e_contact_field_is_string(E_CONTACT_BOOK_UID), 1);
        let book_uid_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_BOOK_UID));
        assert_eq!(book_uid_name.to_str().unwrap(), "book_uid");

        // vCard field ID mappings
        assert_eq!(
            e_contact_field_id_from_vcard(c"FN".as_ptr().cast()),
            E_CONTACT_FULL_NAME
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"N".as_ptr().cast()),
            E_CONTACT_NAME
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"GEO".as_ptr().cast()),
            E_CONTACT_GEO
        );
        assert_eq!(
            e_contact_field_id_from_vcard(c"REV".as_ptr().cast()),
            E_CONTACT_REV
        );

        // EContactName construction, formatting, and copying
        let name = e_contact_name_new();
        assert!(!name.is_null());
        (*name).family = glib_sys::g_strdup(c"Oldenburg".as_ptr().cast());
        (*name).given = glib_sys::g_strdup(c"Vera".as_ptr().cast());
        (*name).additional = glib_sys::g_strdup(c"Marie".as_ptr().cast());
        (*name).prefixes = glib_sys::g_strdup(c"Dr.".as_ptr().cast());
        (*name).suffixes = glib_sys::g_strdup(c"MSc".as_ptr().cast());

        let rendered_name_ptr = e_contact_name_to_string(name);
        assert!(!rendered_name_ptr.is_null());
        let rendered_name = CStr::from_ptr(rendered_name_ptr).to_str().unwrap();
        assert!(
            rendered_name.contains("Vera") && rendered_name.contains("Oldenburg"),
            "rendered name: {rendered_name}"
        );
        glib_sys::g_free(rendered_name_ptr.cast());

        let copied_name = e_contact_name_copy(name);
        assert!(!copied_name.is_null());
        assert_eq!(
            CStr::from_ptr((*copied_name).family).to_str().unwrap(),
            "Oldenburg"
        );
        assert_eq!(
            CStr::from_ptr((*copied_name).given).to_str().unwrap(),
            "Vera"
        );
        assert_eq!(
            CStr::from_ptr((*copied_name).additional).to_str().unwrap(),
            "Marie"
        );
        assert_eq!(
            CStr::from_ptr((*copied_name).prefixes).to_str().unwrap(),
            "Dr."
        );
        assert_eq!(
            CStr::from_ptr((*copied_name).suffixes).to_str().unwrap(),
            "MSc"
        );
        e_contact_name_free(name);
        e_contact_name_free(copied_name);

        // EContactGeo construction and destruction
        let geo = e_contact_geo_new();
        assert!(!geo.is_null());
        println!("DEBUG 8");
        (*geo).latitude = 37.386013;
        (*geo).longitude = -122.082932;
        assert_eq!((*geo).latitude, 37.386013);
        assert_eq!((*geo).longitude, -122.082932);
        println!("DEBUG 9");
        e_contact_geo_free(geo);
        println!("DEBUG 10");

        // EContactCert construction and destruction
        let cert = e_contact_cert_new();
        assert!(!cert.is_null());
        println!("DEBUG 11");
        e_contact_cert_free(cert);
        println!("DEBUG 12");
    }
}

#[test]
fn structured_name_geo_and_metadata_vcard_lines_and_modification_in_eds() {
    let vcard_str = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:pas-id-test-name-geo-001\r\n",
        "FN:Dr. Vera Marie Oldenburg MSc\r\n",
        "N:Oldenburg;Vera;Marie;Dr.;MSc\r\n",
        "GEO:37.386013;-122.082932\r\n",
        "REV:2026-08-14T12:00:00Z\r\n",
        "X-MOZILLA-HTML:TRUE\r\n",
        "END:VCARD\r\n"
    );

    unsafe {
        let vcard_c = std::ffi::CString::new(vcard_str).unwrap();
        let contact = e_contact_new_from_vcard(vcard_c.as_ptr().cast());
        assert!(!contact.is_null());

        // Full name and synthetic name components
        let full_name = e_contact_get_const(contact, E_CONTACT_FULL_NAME);
        assert!(!full_name.is_null());
        assert_eq!(
            CStr::from_ptr(full_name.cast()).to_str().unwrap(),
            "Dr. Vera Marie Oldenburg MSc"
        );

        let given_name = e_contact_get_const(contact, E_CONTACT_GIVEN_NAME);
        assert!(!given_name.is_null());
        assert_eq!(CStr::from_ptr(given_name.cast()).to_str().unwrap(), "Vera");

        let family_name = e_contact_get_const(contact, E_CONTACT_FAMILY_NAME);
        assert!(!family_name.is_null());
        assert_eq!(
            CStr::from_ptr(family_name.cast()).to_str().unwrap(),
            "Oldenburg"
        );

        // Structured EContactName
        let name_struct = e_contact_get(contact, E_CONTACT_NAME) as *mut EContactName;
        assert!(!name_struct.is_null());
        assert_eq!(
            CStr::from_ptr((*name_struct).family).to_str().unwrap(),
            "Oldenburg"
        );
        assert_eq!(
            CStr::from_ptr((*name_struct).given).to_str().unwrap(),
            "Vera"
        );
        assert_eq!(
            CStr::from_ptr((*name_struct).additional).to_str().unwrap(),
            "Marie"
        );
        assert_eq!(
            CStr::from_ptr((*name_struct).prefixes).to_str().unwrap(),
            "Dr."
        );
        assert_eq!(
            CStr::from_ptr((*name_struct).suffixes).to_str().unwrap(),
            "MSc"
        );
        e_contact_name_free(name_struct);

        // Geographic coordinates
        let geo_struct = e_contact_get(contact, E_CONTACT_GEO) as *mut EContactGeo;
        assert!(!geo_struct.is_null());
        assert!(((*geo_struct).latitude - 37.386013).abs() < 1e-5);
        assert!(((*geo_struct).longitude - -122.082932).abs() < 1e-5);
        e_contact_geo_free(geo_struct);

        // REV and NAME_OR_ORG
        let rev = e_contact_get_const(contact, E_CONTACT_REV);
        assert!(!rev.is_null());
        assert_eq!(
            CStr::from_ptr(rev.cast()).to_str().unwrap(),
            "2026-08-14T12:00:00Z"
        );

        let name_or_org = e_contact_get_const(contact, E_CONTACT_NAME_OR_ORG);
        assert!(!name_or_org.is_null());
        // NAME_OR_ORG returns the first of [File-As, Full Name, Org, Email1];
        // when File-As is not explicit, 3.52 derives it as "Family, Given"
        // while 3.60 hands back the full name as it stands — a libebook
        // behaviour change with no header-visible signal of its own, measured
        // on both legs in docs/eds-version-matrix.md (B). See
        // `eds_death_date_field`'s doc comment in `build.rs` for why that cfg
        // is the pivot here too.
        let expected_name_or_org = if cfg!(eds_death_date_field) {
            "Dr. Vera Marie Oldenburg MSc"
        } else {
            "Oldenburg, Vera"
        };
        assert_eq!(
            CStr::from_ptr(name_or_org.cast()).to_str().unwrap(),
            expected_name_or_org
        );

        // In-place modification of full name and geo
        e_contact_set(
            contact,
            E_CONTACT_FULL_NAME,
            c"Prof. Dr. Vera Oldenburg".as_ptr().cast(),
        );

        let new_geo = e_contact_geo_new();
        (*new_geo).latitude = 47.3769;
        (*new_geo).longitude = 8.5417;
        e_contact_set(contact, E_CONTACT_GEO, new_geo.cast());
        e_contact_geo_free(new_geo);

        let updated_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let updated_vcard = CStr::from_ptr(updated_vcard_ptr).to_str().unwrap();

        assert!(
            updated_vcard.contains("Prof. Dr. Vera Oldenburg"),
            "updated full name missing: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("47.3769") || updated_vcard.contains("47.37689"),
            "updated latitude missing: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("8.5417"),
            "updated longitude missing: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("N:Oldenburg;Vera;Marie;Dr.;MSc"),
            "N line must be preserved: {updated_vcard}"
        );

        g_free(updated_vcard_ptr.cast());

        // Field clearing by setting NULL
        e_contact_set(contact, E_CONTACT_GEO, std::ptr::null());

        let cleared_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let cleared_vcard = CStr::from_ptr(cleared_vcard_ptr).to_str().unwrap();

        assert!(
            !cleared_vcard.contains("GEO:"),
            "cleared GEO must be removed: {cleared_vcard}"
        );
        assert!(
            cleared_vcard.contains("Prof. Dr. Vera Oldenburg"),
            "full name must remain: {cleared_vcard}"
        );

        g_free(cleared_vcard_ptr.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}

#[test]
fn contact_structured_address_and_office_field_properties() {
    unsafe {
        let addr_type = e_contact_address_get_type();
        assert_ne!(addr_type, 0);

        assert_eq!(E_CONTACT_FIRST_ADDRESS_ID, 90);
        assert_eq!(E_CONTACT_LAST_ADDRESS_ID, 92);
        assert_eq!(E_CONTACT_FIRST_LABEL_ID, 13);
        assert_eq!(E_CONTACT_LAST_LABEL_ID, 15);

        // E_CONTACT_ADDRESS is a multi-valued structured field
        assert_eq!(e_contact_field_is_string(E_CONTACT_ADDRESS), 0);
        let addr_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_ADDRESS));
        assert_eq!(addr_name.to_str().unwrap(), "address");

        // Structured address synthetic fields
        let address_fields: &[(EContactField, &str)] = &[
            (E_CONTACT_ADDRESS_HOME, "address_home"),
            (E_CONTACT_ADDRESS_WORK, "address_work"),
            (E_CONTACT_ADDRESS_OTHER, "address_other"),
        ];

        for &(field, expected_name) in address_fields {
            assert_eq!(
                e_contact_field_is_string(field),
                0,
                "field {expected_name} must be a structured address field"
            );
            assert_eq!(
                e_contact_field_type(field),
                addr_type,
                "field {expected_name} must match EContactAddress type"
            );
            let name_cstr = CStr::from_ptr(e_contact_field_name(field));
            assert_eq!(name_cstr.to_str().unwrap(), expected_name);
            let attr_cstr = CStr::from_ptr(e_contact_vcard_attribute(field));
            assert_eq!(attr_cstr.to_str().unwrap(), "ADR");
        }

        // Office field is a simple string stored on the ORG attribute (3rd component)
        assert_eq!(e_contact_field_is_string(E_CONTACT_OFFICE), 1);
        let office_name = CStr::from_ptr(e_contact_field_name(E_CONTACT_OFFICE));
        assert_eq!(office_name.to_str().unwrap(), "office");
        let office_attr = CStr::from_ptr(e_contact_vcard_attribute(E_CONTACT_OFFICE));
        assert_eq!(office_attr.to_str().unwrap(), "ORG");

        // Synthetic address label fields
        let label_fields: &[(EContactField, &str)] = &[
            (E_CONTACT_ADDRESS_LABEL_HOME, "address_label_home"),
            (E_CONTACT_ADDRESS_LABEL_WORK, "address_label_work"),
            (E_CONTACT_ADDRESS_LABEL_OTHER, "address_label_other"),
        ];

        for &(field, expected_name) in label_fields {
            assert_eq!(
                e_contact_field_is_string(field),
                1,
                "field {expected_name} must be a string field"
            );
            let name_cstr = CStr::from_ptr(e_contact_field_name(field));
            assert_eq!(name_cstr.to_str().unwrap(), expected_name);
            let attr_cstr = CStr::from_ptr(e_contact_vcard_attribute(field));
            assert_eq!(attr_cstr.to_str().unwrap(), "LABEL");
        }

        // vCard field ID mapping for ADR
        assert_eq!(
            e_contact_field_id_from_vcard(c"ADR".as_ptr().cast()),
            E_CONTACT_ADDRESS
        );

        // EContactAddress allocation and manipulation
        let addr = e_contact_address_new();
        assert!(!addr.is_null());
        (*addr).po = glib_sys::g_strdup(c"PO Box 42".as_ptr().cast());
        (*addr).ext = glib_sys::g_strdup(c"Suite 100".as_ptr().cast());
        (*addr).street = glib_sys::g_strdup(c"Hauptstraße 1".as_ptr().cast());
        (*addr).locality = glib_sys::g_strdup(c"Berlin".as_ptr().cast());
        (*addr).region = glib_sys::g_strdup(c"Brandenburg".as_ptr().cast());
        (*addr).code = glib_sys::g_strdup(c"10115".as_ptr().cast());
        (*addr).country = glib_sys::g_strdup(c"Germany".as_ptr().cast());

        assert_eq!(CStr::from_ptr((*addr).po).to_str().unwrap(), "PO Box 42");
        assert_eq!(CStr::from_ptr((*addr).ext).to_str().unwrap(), "Suite 100");
        assert_eq!(
            CStr::from_ptr((*addr).street).to_str().unwrap(),
            "Hauptstraße 1"
        );
        assert_eq!(CStr::from_ptr((*addr).locality).to_str().unwrap(), "Berlin");
        assert_eq!(
            CStr::from_ptr((*addr).region).to_str().unwrap(),
            "Brandenburg"
        );
        assert_eq!(CStr::from_ptr((*addr).code).to_str().unwrap(), "10115");
        assert_eq!(CStr::from_ptr((*addr).country).to_str().unwrap(), "Germany");

        e_contact_address_free(addr);
    }
}

#[test]
fn structured_address_and_office_vcard_lines_and_modification_in_eds() {
    let vcard_str = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:pas-id-test-address-office-001\r\n",
        "FN:Vera Olden\r\n",
        "N:Olden;Vera;;;\r\n",
        "ORG;X-JMAP-KEY=o1:Acme Ltd;Research;Building 4 Room 204\r\n",
        "ADR;TYPE=WORK;X-JMAP-KEY=a1:PO Box 42;Suite 100;Hauptstraße 1;Berlin;Brandenburg;10115;Germany\r\n",
        "LABEL;TYPE=WORK;X-JMAP-KEY=a1:PO Box 42\\nSuite 100\\nHauptstraße 1\\n10115 Berlin\\nGermany\r\n",
        "ADR;TYPE=HOME;X-JMAP-KEY=a2:;;Heimweg 2;München;Bayern;80331;Germany\r\n",
        "LABEL;TYPE=HOME;X-JMAP-KEY=a2:Heimweg 2\\n80331 München\\nGermany\r\n",
        "END:VCARD\r\n"
    );

    unsafe {
        let vcard_c = std::ffi::CString::new(vcard_str).unwrap();
        let contact = e_contact_new_from_vcard(vcard_c.as_ptr().cast());
        assert!(!contact.is_null());

        // Structured address inspect
        let work_addr = e_contact_get(contact, E_CONTACT_ADDRESS_WORK) as *mut EContactAddress;
        assert!(!work_addr.is_null());
        assert_eq!(
            CStr::from_ptr((*work_addr).po).to_str().unwrap(),
            "PO Box 42"
        );
        assert_eq!(
            CStr::from_ptr((*work_addr).ext).to_str().unwrap(),
            "Suite 100"
        );
        assert_eq!(
            CStr::from_ptr((*work_addr).street).to_str().unwrap(),
            "Hauptstraße 1"
        );
        assert_eq!(
            CStr::from_ptr((*work_addr).locality).to_str().unwrap(),
            "Berlin"
        );
        assert_eq!(
            CStr::from_ptr((*work_addr).region).to_str().unwrap(),
            "Brandenburg"
        );
        assert_eq!(CStr::from_ptr((*work_addr).code).to_str().unwrap(), "10115");
        assert_eq!(
            CStr::from_ptr((*work_addr).country).to_str().unwrap(),
            "Germany"
        );
        e_contact_address_free(work_addr);

        let home_addr = e_contact_get(contact, E_CONTACT_ADDRESS_HOME) as *mut EContactAddress;
        assert!(!home_addr.is_null());
        assert_eq!(
            CStr::from_ptr((*home_addr).street).to_str().unwrap(),
            "Heimweg 2"
        );
        assert_eq!(
            CStr::from_ptr((*home_addr).locality).to_str().unwrap(),
            "München"
        );
        assert_eq!(
            CStr::from_ptr((*home_addr).region).to_str().unwrap(),
            "Bayern"
        );
        assert_eq!(CStr::from_ptr((*home_addr).code).to_str().unwrap(), "80331");
        assert_eq!(
            CStr::from_ptr((*home_addr).country).to_str().unwrap(),
            "Germany"
        );
        e_contact_address_free(home_addr);

        // Office inspection from 3rd component of ORG
        let office = e_contact_get_const(contact, E_CONTACT_OFFICE);
        assert!(!office.is_null());
        assert_eq!(
            CStr::from_ptr(office.cast()).to_str().unwrap(),
            "Building 4 Room 204"
        );

        // In-place modification of work address and office
        let new_work_addr = e_contact_address_new();
        (*new_work_addr).street = glib_sys::g_strdup(c"Unter den Linden 5".as_ptr().cast());
        (*new_work_addr).locality = glib_sys::g_strdup(c"Berlin".as_ptr().cast());
        (*new_work_addr).code = glib_sys::g_strdup(c"10117".as_ptr().cast());
        (*new_work_addr).country = glib_sys::g_strdup(c"Germany".as_ptr().cast());

        e_contact_set(contact, E_CONTACT_ADDRESS_WORK, new_work_addr.cast());
        e_contact_address_free(new_work_addr);

        e_contact_set(
            contact,
            E_CONTACT_OFFICE,
            c"Tower 1, Floor 15".as_ptr().cast(),
        );

        let updated_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let updated_vcard = CStr::from_ptr(updated_vcard_ptr).to_str().unwrap();

        assert!(
            updated_vcard.contains("Unter den Linden 5"),
            "updated street missing: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("10117"),
            "updated postcode missing: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("Heimweg 2"),
            "home street must be preserved: {updated_vcard}"
        );
        assert!(
            updated_vcard.contains("Tower 1\\, Floor 15")
                || updated_vcard.contains("Tower 1, Floor 15"),
            "updated office missing: {updated_vcard}"
        );

        g_free(updated_vcard_ptr.cast());

        // Field clearing by setting NULL
        e_contact_set(contact, E_CONTACT_ADDRESS_WORK, std::ptr::null());
        e_contact_set(contact, E_CONTACT_OFFICE, std::ptr::null());

        let cleared_vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let cleared_vcard = CStr::from_ptr(cleared_vcard_ptr).to_str().unwrap();

        assert!(
            !cleared_vcard.contains("Unter den Linden 5"),
            "cleared work address must be removed: {cleared_vcard}"
        );
        assert!(
            !cleared_vcard.contains("Tower 1"),
            "cleared office must be removed: {cleared_vcard}"
        );
        assert!(
            cleared_vcard.contains("Heimweg 2"),
            "home address must remain: {cleared_vcard}"
        );
        assert!(
            cleared_vcard.contains("Acme Ltd;Research"),
            "organization name and unit must remain after clearing office: {cleared_vcard}"
        );

        g_free(cleared_vcard_ptr.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}

#[test]
fn a_line_wearing_both_context_types_fills_two_slots_that_overwrite_each_other() {
    let vcard_str = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:pas-id-test-both-contexts-001\r\n",
        "FN:Vera Olden\r\n",
        "ADR;TYPE=WORK,HOME;X-JMAP-KEY=a1:;;Hauptstraße 1;Berlin;;10115;Germany\r\n",
        "TEL;TYPE=WORK,HOME,VOICE;X-JMAP-KEY=p1:+49 30 111\r\n",
        "END:VCARD\r\n"
    );

    unsafe {
        let vcard_c = CString::new(vcard_str).unwrap();
        let contact = e_contact_new_from_vcard(vcard_c.as_ptr().cast());
        assert!(!contact.is_null());

        // One `ADR` line, and both of the two per-context fields Evolution's
        // contact editor shows read it. `E_CONTACT_ADDRESS_OTHER` — the field a
        // line with no `TYPE` lands in — stays empty.
        let work = e_contact_get(contact, E_CONTACT_ADDRESS_WORK) as *mut EContactAddress;
        let home = e_contact_get(contact, E_CONTACT_ADDRESS_HOME) as *mut EContactAddress;
        let other = e_contact_get(contact, E_CONTACT_ADDRESS_OTHER) as *mut EContactAddress;
        assert!(!work.is_null() && !home.is_null());
        assert_eq!(
            CStr::from_ptr((*work).street).to_str().unwrap(),
            "Hauptstraße 1"
        );
        assert_eq!(
            CStr::from_ptr((*home).street).to_str().unwrap(),
            "Hauptstraße 1",
            "the work address is also the home address"
        );
        assert!(other.is_null());
        e_contact_address_free(home);

        // The same for the telephone: `E_CONTACT_PHONE_BUSINESS` wants
        // `WORK`+`VOICE` and `E_CONTACT_PHONE_HOME` wants `HOME`+`VOICE`, and a
        // line carrying all three satisfies both.
        let business = e_contact_get_const(contact, E_CONTACT_PHONE_BUSINESS);
        let home_phone = e_contact_get_const(contact, E_CONTACT_PHONE_HOME);
        assert!(!business.is_null() && !home_phone.is_null());
        assert_eq!(
            CStr::from_ptr(business.cast()).to_str().unwrap(),
            "+49 30 111"
        );
        assert_eq!(
            CStr::from_ptr(home_phone.cast()).to_str().unwrap(),
            "+49 30 111"
        );

        // Which is why the mapping never writes both: the user retypes the
        // *work* address, and the one line behind both fields is rewritten, so
        // their *home* address silently becomes the new work one.
        glib_sys::g_free((*work).street.cast());
        (*work).street = glib_sys::g_strdup(c"Nebenstraße 2".as_ptr().cast());
        e_contact_set(contact, E_CONTACT_ADDRESS_WORK, work.cast());
        e_contact_address_free(work);

        let edited_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let edited = CStr::from_ptr(edited_ptr).to_str().unwrap();
        assert_eq!(
            edited.matches("\r\nADR").count(),
            1,
            "still one line, not one per slot: {edited}"
        );
        assert!(
            edited.contains("ADR;X-JMAP-KEY=a1;TYPE=WORK,HOME:;;Nebenstraße 2;"),
            "{edited}"
        );
        let edited_c = CString::new(edited).unwrap();
        g_free(edited_ptr.cast());

        let after = e_contact_new_from_vcard(edited_c.as_ptr().cast());
        let home_after = e_contact_get(after, E_CONTACT_ADDRESS_HOME) as *mut EContactAddress;
        assert_eq!(
            CStr::from_ptr((*home_after).street).to_str().unwrap(),
            "Nebenstraße 2",
            "the home address the user never touched moved with the work one"
        );
        e_contact_address_free(home_after);

        gobject_sys::g_object_unref(after.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}

/// Which of the phone fields a `TEL` line wearing these `TYPE`s reaches.
fn phone_fields_reached(types: &str) -> Vec<&'static str> {
    const FIELDS: [(u32, &str); 6] = [
        (E_CONTACT_PHONE_BUSINESS, "business"),
        (E_CONTACT_PHONE_BUSINESS_FAX, "business_fax"),
        (E_CONTACT_PHONE_MOBILE, "mobile"),
        (E_CONTACT_PHONE_OTHER, "other"),
        (E_CONTACT_PHONE_OTHER_FAX, "other_fax"),
        (E_CONTACT_PHONE_PAGER, "pager"),
    ];
    let vcard = format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:pas-id-test-features-001\r\nFN:Vera Olden\r\n\
         TEL;TYPE={types}:+49 30 111\r\nEND:VCARD\r\n"
    );
    unsafe {
        let vcard_c = CString::new(vcard).unwrap();
        let contact = e_contact_new_from_vcard(vcard_c.as_ptr().cast());
        assert!(!contact.is_null());
        let reached = FIELDS
            .into_iter()
            .filter(|(id, _)| !e_contact_get_const(contact, *id).is_null())
            .map(|(_, name)| name)
            .collect();
        gobject_sys::g_object_unref(contact.cast());
        reached
    }
}

#[test]
fn a_line_wearing_several_feature_types_reaches_two_fields_or_none() {
    // The same defect as `a_line_wearing_both_context_types_fills_two_slots_
    // that_overwrite_each_other`, one axis over: a phone's features also pick
    // the field, so a number that is both a voice line and a fax fills the
    // Business Phone field and the Business Fax field alike, and there is one
    // `TEL` behind both.
    assert_eq!(
        phone_fields_reached("WORK,VOICE,FAX"),
        ["business", "business_fax"]
    );
    // A mobile that is also a pager needs no context to reach two.
    assert_eq!(phone_fields_reached("CELL,PAGER"), ["mobile", "pager"]);
    // And with no context at all the pair reaches *neither* field: the two
    // unqualified fields are exclusive, so a number that is both a voice line
    // and a fax is in no field of the contact editor whatsoever.
    assert_eq!(phone_fields_reached("VOICE,FAX"), [] as [&str; 0]);
    assert_eq!(phone_fields_reached("VOICE"), ["other"]);
    assert_eq!(phone_fields_reached("FAX"), ["other_fax"]);

    // Which feature the mapping keeps when it can state only one follows from
    // what EDS does with the pair where it resolves it itself: the feature
    // naming a device wins over the unqualified `VOICE`/`FAX` fields.
    assert_eq!(phone_fields_reached("VOICE,CELL"), ["mobile"]);
    assert_eq!(phone_fields_reached("FAX,CELL"), ["mobile"]);
    assert_eq!(phone_fields_reached("VOICE,PAGER"), ["pager"]);
    assert_eq!(phone_fields_reached("FAX,PAGER"), ["pager"]);

    // `VIDEO` is not a `TYPE` this EDS knows: on its own it reaches nothing,
    // and beside another feature it is ignored — which is why it is the last
    // feature the mapping would ever state.
    assert_eq!(phone_fields_reached("VIDEO"), [] as [&str; 0]);
    assert_eq!(phone_fields_reached("VOICE,VIDEO"), ["other"]);
    assert_eq!(phone_fields_reached("FAX,VIDEO"), ["other_fax"]);
    assert_eq!(phone_fields_reached("CELL,VIDEO"), ["mobile"]);

    // A `TEL` that names no feature at all is a voice line to EDS, which is
    // what makes `voice` the unmarked one: it is still said when left off.
    assert_eq!(phone_fields_reached("WORK"), ["business"]);
}

#[test]
fn editing_one_of_the_two_fields_a_multi_feature_line_fills_rewrites_the_other() {
    let vcard_str = concat!(
        "BEGIN:VCARD\r\n",
        "VERSION:3.0\r\n",
        "UID:pas-id-test-features-002\r\n",
        "FN:Vera Olden\r\n",
        "TEL;TYPE=WORK,VOICE,FAX;X-JMAP-KEY=p1:+49 30 111\r\n",
        "END:VCARD\r\n"
    );

    unsafe {
        let vcard_c = CString::new(vcard_str).unwrap();
        let contact = e_contact_new_from_vcard(vcard_c.as_ptr().cast());
        assert!(!contact.is_null());

        // The user retypes the office phone number.
        e_contact_set(
            contact,
            E_CONTACT_PHONE_BUSINESS,
            c"+49 30 222".as_ptr().cast::<std::ffi::c_void>().cast_mut(),
        );
        let edited_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let edited = CStr::from_ptr(edited_ptr).to_str().unwrap();
        assert_eq!(
            edited.matches("\r\nTEL").count(),
            1,
            "still one line, not one per field: {edited}"
        );
        let edited_c = CString::new(edited).unwrap();
        g_free(edited_ptr.cast());

        let after = e_contact_new_from_vcard(edited_c.as_ptr().cast());
        let fax = e_contact_get_const(after, E_CONTACT_PHONE_BUSINESS_FAX);
        assert!(!fax.is_null());
        assert_eq!(
            CStr::from_ptr(fax.cast()).to_str().unwrap(),
            "+49 30 222",
            "the fax number the user never touched moved with the phone one"
        );

        gobject_sys::g_object_unref(after.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}

#[test]
fn test_unslotted_twitter_and_sip_attr_lists_vs_slotted_im_fields_in_eds() {
    let vcard_str = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Vera User\r\n\
N:User;Vera;;;\r\n\
X-JABBER;TYPE=HOME:vera@jabber.example\r\n\
X-AIM;TYPE=HOME:vera_aim\r\n\
X-ICQ;TYPE=HOME:12345678\r\n\
X-MSN;TYPE=HOME:vera@msn.com\r\n\
X-YAHOO;TYPE=HOME:vera_yahoo\r\n\
X-GADUGADU;TYPE=HOME:87654321\r\n\
X-GROUPWISE;TYPE=HOME:vera_gw\r\n\
X-GOOGLE-TALK;TYPE=HOME:vera@gmail.com\r\n\
X-MATRIX;TYPE=HOME:@vera:matrix.example\r\n\
X-SKYPE;TYPE=HOME:vera_skype\r\n\
X-TWITTER:@vera_tw\r\n\
X-SIP:sip:vera@example.com\r\n\
END:VCARD\r\n";

    unsafe {
        let vcard_c = CString::new(vcard_str).unwrap();
        let contact = e_contact_new_from_vcard(vcard_c.as_ptr().cast());
        assert!(!contact.is_null());

        // 1. Verify all 10 slotted IM fields are strings and hold the expected handles
        let slotted_expected: &[(EContactField, &str)] = &[
            (E_CONTACT_IM_JABBER_HOME_1, "vera@jabber.example"),
            (E_CONTACT_IM_AIM_HOME_1, "vera_aim"),
            (E_CONTACT_IM_ICQ_HOME_1, "12345678"),
            (E_CONTACT_IM_MSN_HOME_1, "vera@msn.com"),
            (E_CONTACT_IM_YAHOO_HOME_1, "vera_yahoo"),
            (E_CONTACT_IM_GADUGADU_HOME_1, "87654321"),
            (E_CONTACT_IM_GROUPWISE_HOME_1, "vera_gw"),
            (E_CONTACT_IM_GOOGLE_TALK_HOME_1, "vera@gmail.com"),
            (E_CONTACT_IM_MATRIX_HOME_1, "@vera:matrix.example"),
            (E_CONTACT_IM_SKYPE_HOME_1, "vera_skype"),
        ];

        for &(field, expected_handle) in slotted_expected {
            assert_eq!(
                e_contact_field_is_string(field),
                1,
                "field {field} must be a string"
            );
            let val_ptr = e_contact_get_const(contact, field);
            assert!(!val_ptr.is_null(), "field {field} must not be null");
            assert_eq!(
                CStr::from_ptr(val_ptr.cast()).to_str().unwrap(),
                expected_handle
            );
        }

        // 2. Verify X-TWITTER and X-SIP are EContactAttrList (not strings, no slots)
        let attr_list_type = e_contact_attr_list_get_type();
        assert_eq!(e_contact_field_is_string(E_CONTACT_IM_TWITTER), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_IM_TWITTER), attr_list_type);
        assert_eq!(e_contact_field_is_string(E_CONTACT_SIP), 0);
        assert_eq!(e_contact_field_type(E_CONTACT_SIP), attr_list_type);

        unsafe extern "C" fn free_string_item(p: *mut std::ffi::c_void) {
            unsafe {
                glib_sys::g_free(p);
            }
        }

        // Read Twitter attribute list
        let twitter_list = e_contact_get(contact, E_CONTACT_IM_TWITTER) as *mut glib_sys::GList;
        assert!(!twitter_list.is_null());
        let twitter_val = CStr::from_ptr((*twitter_list).data as *const gchar)
            .to_str()
            .unwrap();
        assert_eq!(twitter_val, "@vera_tw");
        glib_sys::g_list_free_full(twitter_list, Some(free_string_item));

        // Read SIP attribute list
        let sip_list = e_contact_get(contact, E_CONTACT_SIP) as *mut glib_sys::GList;
        assert!(!sip_list.is_null());
        let sip_val = CStr::from_ptr((*sip_list).data as *const gchar)
            .to_str()
            .unwrap();
        assert_eq!(sip_val, "sip:vera@example.com");
        glib_sys::g_list_free_full(sip_list, Some(free_string_item));

        gobject_sys::g_object_unref(contact.cast());
    }
}

#[test]
fn test_photo_handling_uri_rendering_replacement_and_clearing_in_eds() {
    unsafe {
        // 1. Initial contact with no photo
        let contact = e_contact_new();
        assert!(!contact.is_null());
        e_contact_set(
            contact,
            E_CONTACT_FULL_NAME,
            c"Vera Oldenburg".as_ptr().cast(),
        );

        let photo_none = e_contact_get(contact, E_CONTACT_PHOTO) as *mut EContactPhoto;
        assert!(photo_none.is_null());

        // 2. Set URI photo (HTTPS)
        let uri_photo = e_contact_photo_new();
        (*uri_photo).type_ = E_CONTACT_PHOTO_TYPE_URI;
        e_contact_photo_set_uri(uri_photo, c"https://example.com/avatar.jpg".as_ptr());
        e_contact_set(contact, E_CONTACT_PHOTO, uri_photo.cast());
        e_contact_photo_free(uri_photo);

        let vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let vcard_str = CStr::from_ptr(vcard_ptr).to_str().unwrap();
        assert!(
            vcard_str.contains("PHOTO;VALUE=uri:https://example.com/avatar.jpg"),
            "URI photo must emit VALUE=uri without TYPE: {vcard_str}"
        );
        assert!(
            !vcard_str.contains("PHOTO;TYPE="),
            "URI photo must not state TYPE: {vcard_str}"
        );
        g_free(vcard_ptr.cast());

        // 3. Set URI photo (file:// local URI)
        let file_uri_photo = e_contact_photo_new();
        (*file_uri_photo).type_ = E_CONTACT_PHOTO_TYPE_URI;
        e_contact_photo_set_uri(
            file_uri_photo,
            c"file:///home/runner/.photos/vera.png".as_ptr(),
        );
        e_contact_set(contact, E_CONTACT_PHOTO, file_uri_photo.cast());
        e_contact_photo_free(file_uri_photo);

        let vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let vcard_str = CStr::from_ptr(vcard_ptr).to_str().unwrap();
        assert!(
            vcard_str.contains("PHOTO;VALUE=uri:file:///home/runner/.photos/vera.png"),
            "file URI photo must emit VALUE=uri: {vcard_str}"
        );
        assert!(
            !vcard_str.contains("https://example.com/avatar.jpg"),
            "previous URI should be replaced"
        );
        g_free(vcard_ptr.cast());

        // 4. Replace URI photo with inlined binary photo (JPEG)
        let inlined_photo = e_contact_photo_new();
        assert_eq!((*inlined_photo).type_, E_CONTACT_PHOTO_TYPE_INLINED);
        let sample_jpeg = b"\xFF\xD8\xFF\xE0sample_jpeg_binary_payload";
        e_contact_photo_set_inlined(
            inlined_photo,
            sample_jpeg.as_ptr(),
            sample_jpeg.len() as gsize,
        );
        e_contact_photo_set_mime_type(inlined_photo, c"image/jpeg".as_ptr());
        e_contact_set(contact, E_CONTACT_PHOTO, inlined_photo.cast());
        e_contact_photo_free(inlined_photo);

        let vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let vcard_str = CStr::from_ptr(vcard_ptr).to_str().unwrap();
        assert!(
            vcard_str.contains("PHOTO;TYPE=jpeg;ENCODING=b:")
                || vcard_str.contains("PHOTO;ENCODING=b;TYPE=jpeg:"),
            "inlined photo should replace URI and emit TYPE=jpeg;ENCODING=b: {vcard_str}"
        );
        assert!(
            !vcard_str.contains("VALUE=uri"),
            "inlined photo must not state VALUE=uri: {vcard_str}"
        );
        g_free(vcard_ptr.cast());

        // 5. Replace inlined JPEG with inlined PNG photo
        let png_photo = e_contact_photo_new();
        let sample_png = b"\x89PNG\r\n\x1a\nsample_png_binary_payload";
        e_contact_photo_set_inlined(png_photo, sample_png.as_ptr(), sample_png.len() as gsize);
        e_contact_photo_set_mime_type(png_photo, c"image/png".as_ptr());
        e_contact_set(contact, E_CONTACT_PHOTO, png_photo.cast());
        e_contact_photo_free(png_photo);

        let vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let vcard_str = CStr::from_ptr(vcard_ptr).to_str().unwrap();
        assert!(
            vcard_str.contains("PHOTO;TYPE=png;ENCODING=b:")
                || vcard_str.contains("PHOTO;ENCODING=b;TYPE=png:")
                || vcard_str.contains("PHOTO;ENCODING=b;TYPE=PNG:"),
            "PNG photo should replace JPEG and emit TYPE=png: {vcard_str}"
        );
        g_free(vcard_ptr.cast());

        // 6. Inlined photo with NULL mime type in EDS -> writes TYPE="X-EVOLUTION-UNKNOWN" or no subtype
        let unknown_photo = e_contact_photo_new();
        let sample_raw = b"raw_unknown_image_bytes";
        e_contact_photo_set_inlined(
            unknown_photo,
            sample_raw.as_ptr(),
            sample_raw.len() as gsize,
        );
        // mime_type left as NULL
        e_contact_set(contact, E_CONTACT_PHOTO, unknown_photo.cast());
        e_contact_photo_free(unknown_photo);

        let vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let vcard_str = CStr::from_ptr(vcard_ptr).to_str().unwrap();
        assert!(
            vcard_str.contains("PHOTO;TYPE=\"X-EVOLUTION-UNKNOWN\";ENCODING=b:")
                || vcard_str.contains("PHOTO;TYPE=X-EVOLUTION-UNKNOWN;ENCODING=b:")
                || vcard_str.contains("PHOTO;ENCODING=b:"),
            "NULL mime_type in EDS should emit X-EVOLUTION-UNKNOWN or bare ENCODING=b: {vcard_str}"
        );
        g_free(vcard_ptr.cast());

        // 7. Clear E_CONTACT_PHOTO (user removed picture in editor)
        e_contact_set(contact, E_CONTACT_PHOTO, std::ptr::null());

        let vcard_ptr = e_vcard_to_string_vcard_30(contact.cast());
        let vcard_str = CStr::from_ptr(vcard_ptr).to_str().unwrap();
        assert!(
            !vcard_str.contains("PHOTO"),
            "clearing photo must remove PHOTO line from vCard: {vcard_str}"
        );
        g_free(vcard_ptr.cast());

        // 8. Inbound vCard with PHOTO;VALUE=uri vs bare PHOTO:https://
        let test_vcard = c"BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
FN:Test Multi Photo\r\n\
PHOTO;VALUE=uri:https://example.com/explicit_uri.png\r\n\
PHOTO;X-JMAP-KEY=m2;TYPE=jpeg;ENCODING=b:c2FtcGxl\r\n\
LOGO;VALUE=uri:https://example.com/logo.png\r\n\
END:VCARD\r\n";
        let multi_contact = e_contact_new_from_vcard(test_vcard.as_ptr());
        assert!(!multi_contact.is_null());

        let read_photo = e_contact_get(multi_contact, E_CONTACT_PHOTO) as *mut EContactPhoto;
        assert!(!read_photo.is_null());
        assert_eq!((*read_photo).type_, E_CONTACT_PHOTO_TYPE_URI);
        let read_uri = e_contact_photo_get_uri(read_photo);
        assert_eq!(
            CStr::from_ptr(read_uri).to_str().unwrap(),
            "https://example.com/explicit_uri.png"
        );
        // Note: EDS e_contact_photo_get_mime_type asserts photo->type == E_CONTACT_PHOTO_TYPE_INLINED.
        // URI photos in EDS do not have a mime_type field.
        e_contact_photo_free(read_photo);

        // Replacing first photo with URI on multi-photo contact leaves second photo and logo intact
        let new_uri_obj = e_contact_photo_new();
        (*new_uri_obj).type_ = E_CONTACT_PHOTO_TYPE_URI;
        e_contact_photo_set_uri(
            new_uri_obj,
            c"https://example.com/replaced_avatar.jpg".as_ptr(),
        );
        e_contact_set(multi_contact, E_CONTACT_PHOTO, new_uri_obj.cast());
        e_contact_photo_free(new_uri_obj);

        let vcard_ptr = e_vcard_to_string_vcard_30(multi_contact.cast());
        let vcard_str = CStr::from_ptr(vcard_ptr).to_str().unwrap();
        assert!(
            vcard_str.contains("PHOTO;VALUE=uri:https://example.com/replaced_avatar.jpg"),
            "first photo replaced with new URI: {vcard_str}"
        );
        assert!(
            vcard_str.contains("PHOTO;X-JMAP-KEY=m2;TYPE=jpeg;ENCODING=b:c2FtcGxl")
                || vcard_str.contains("PHOTO;ENCODING=b;TYPE=jpeg;X-JMAP-KEY=m2:c2FtcGxl"),
            "second photo preserved: {vcard_str}"
        );
        assert!(
            vcard_str.contains("LOGO;VALUE=uri:https://example.com/logo.png"),
            "logo preserved: {vcard_str}"
        );
        g_free(vcard_ptr.cast());

        gobject_sys::g_object_unref(multi_contact.cast());
        gobject_sys::g_object_unref(contact.cast());
    }
}
