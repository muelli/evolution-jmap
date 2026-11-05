// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cross-protocol audit (ROADMAP 169), area "JMAP strings into vCard text".
//!
//! The book backend gives every server `ContactCard` to
//! `jmap_vcard::card_to_vcard` and then to `e_contact_new_from_vcard()`.
//! A server string must not add a content line, and must not close a quoted
//! parameter value to add a parameter. These tests put one hostile payload
//! into each string slot and check the output.

use jmap_proto::contacts::ContactCard;
use jmap_vcard::card_to_vcard;
use serde_json::{Value, json};

/// A payload that tries to add a property line.
const LINE: &str = "x\r\nX-INJECTED:1\r\nY";
/// A payload that tries to add a property line with a bare LF.
const BARE_LF: &str = "x\nX-INJECTED:1\nY";
/// A payload that tries to close a quoted parameter and add a parameter.
const PARAM: &str = "a\";X-EVIL=1;Q=\"b";

/// Unfolded content lines of a vCard.
fn lines(vcard: &str) -> Vec<String> {
    vcard
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .split("\r\n")
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The name and parameters of a content line, without the text of quoted
/// parameter values: all text before the first `:` outside quotes.
fn head(line: &str) -> String {
    let mut quoted = false;
    let mut out = String::new();
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => break,
            _ if !quoted => out.push(c),
            _ => {}
        }
    }
    out
}

/// Every card shape that carries `p` in one string slot, as JSContact JSON.
fn slots(p: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("uid", json!({"uid": p})),
        ("name.full", json!({"name": {"full": p}})),
        (
            "name.components",
            json!({"name": {"components": [{"kind": "surname", "value": p}]}}),
        ),
        ("nicknames", json!({"nicknames": {"n": {"name": p}}})),
        ("emails.address", json!({"emails": {"e": {"address": p}}})),
        (
            "emails.label",
            json!({"emails": {"e": {"address": "a@example.com", "label": p}}}),
        ),
        ("phones.number", json!({"phones": {"t": {"number": p}}})),
        (
            "phones.label",
            json!({"phones": {"t": {"number": "+1", "label": p}}}),
        ),
        (
            "organizations",
            json!({"organizations": {"o": {"name": p, "units": [{"name": p}]}}}),
        ),
        ("titles", json!({"titles": {"t": {"name": p}}})),
        ("notes", json!({"notes": {"n": {"note": p}}})),
        ("keywords", json!({"keywords": {p: true}})),
        (
            "addresses.components",
            json!({"addresses": {"a": {"components": [{"kind": "locality", "value": p}]}}}),
        ),
        ("addresses.full", json!({"addresses": {"a": {"full": p}}})),
        ("links.uri", json!({"links": {"l": {"uri": p}}})),
        (
            "links.label",
            json!({"links": {"l": {"uri": "https://x.example/", "label": p}}}),
        ),
        (
            "onlineServices",
            json!({"onlineServices": {"s": {"service": p, "user": p, "uri": p}}}),
        ),
        (
            "calendars",
            json!({"calendars": {"c": {"kind": "calendar", "uri": p}}}),
        ),
        (
            "schedulingAddresses",
            json!({"schedulingAddresses": {"s": {"uri": p}}}),
        ),
        (
            "media.photo",
            json!({"media": {"m": {"kind": "photo", "uri": format!("https://x.example/{p}")}}}),
        ),
        (
            "relatedTo",
            json!({"relatedTo": {p: {"relation": {"spouse": true}}}}),
        ),
        (
            "anniversaries",
            json!({"anniversaries": {"a": {"kind": "birth", "date": p}}}),
        ),
        ("members", json!({"kind": "group", "members": {p: true}})),
    ]
}

fn card(slot: &Value) -> ContactCard {
    let mut v = json!({"id": "C1", "name": {"full": "Vera"}});
    for (k, x) in slot.as_object().expect("object") {
        v[k] = x.clone();
    }
    serde_json::from_value(v).expect("card")
}

/// Whether a content line is the one a payload tries to add.
fn is_injected(line: &str) -> bool {
    line.to_ascii_uppercase().starts_with("X-INJECTED")
}

#[test]
fn no_string_slot_adds_a_content_line() {
    for payload in [LINE, BARE_LF] {
        for (slot, shape) in slots(payload) {
            let vcard = card_to_vcard(&card(&shape));
            assert!(
                !lines(&vcard).iter().any(|l| is_injected(l)),
                "slot {slot} added a line: {vcard}"
            );
            assert!(
                !vcard.replace("\r\n", "").contains('\n')
                    && !vcard.replace("\r\n", "").contains('\r'),
                "slot {slot} left a bare CR or LF: {vcard:?}"
            );
        }
    }
}

#[test]
fn no_string_slot_closes_a_quoted_parameter() {
    for (slot, shape) in slots(PARAM) {
        let vcard = card_to_vcard(&card(&shape));
        for line in lines(&vcard) {
            assert!(
                !head(&line).to_ascii_uppercase().contains("X-EVIL"),
                "slot {slot} added a parameter: {line}"
            );
        }
    }
}

/// RFC 2426 Section 4 and RFC 6350 Section 3.4 escape `,` and `;` only in
/// TEXT values. A URI value with a backslash opens a different URL.
#[test]
fn a_uri_value_is_not_text_escaped() {
    let uri = "https://x.example/j/1;p=2,3";
    for (slot, shape) in [
        ("links.uri", json!({"links": {"l": {"uri": uri}}})),
        (
            "calendars",
            json!({"calendars": {"c": {"kind": "calendar", "uri": uri}}}),
        ),
        (
            "schedulingAddresses",
            json!({"schedulingAddresses": {"s": {"uri": uri}}}),
        ),
        (
            "freeBusy",
            json!({"calendars": {"c": {"kind": "freeBusy", "uri": uri}}}),
        ),
        (
            "media.photo",
            json!({"media": {"m": {"kind": "photo", "uri": uri}}}),
        ),
    ] {
        let vcard = card_to_vcard(&card(&shape));
        assert!(
            !vcard.replace("\r\n ", "").contains("\\;")
                && !vcard.replace("\r\n ", "").contains("\\,"),
            "slot {slot} escaped a URI value: {vcard}"
        );
        assert!(
            vcard.replace("\r\n ", "").contains(uri),
            "slot {slot} lost the URI: {vcard}"
        );
    }
}
