// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `patch::diff` on `virtualLocations` with an edited event that the
//! iCalendar reader cannot make.

use jmap_cal_sync::patch::diff;
use jmap_proto::calendars::CalendarEvent;
use serde_json::{Value, json};

/// Returns an event that holds one virtual location under the key `v1`.
fn joined_online(place: Value) -> CalendarEvent {
    serde_json::from_value(json!({
        "@type": "Event",
        "uid": "standup-1",
        "title": "Standup",
        "start": "2026-01-15T09:00:00",
        "timeZone": "Europe/Berlin",
        "duration": "PT15M",
        "virtualLocations": {"v1": place},
    }))
    .expect("a CalendarEvent")
}

/// RFC 8984 §4.2.6 makes `uri` mandatory. A patch that removes it makes the
/// entry invalid, so an empty or absent edited `uri` changes nothing.
#[test]
fn an_edited_conference_without_a_uri_does_not_remove_the_servers_uri() {
    let current = joined_online(json!({
        "@type": "VirtualLocation",
        "name": "Team room",
        "uri": "https://meet.example.com/standup",
    }));
    for edited_place in [
        json!({"@type": "VirtualLocation", "name": "Team room", "uri": ""}),
        json!({"@type": "VirtualLocation", "name": "Team room"}),
    ] {
        let edited = joined_online(edited_place.clone());

        let patch = diff(&current, &edited);

        assert!(
            !patch.contains_key("virtualLocations/v1/uri"),
            "{edited_place}: the patch removes the uri: {patch:?}"
        );
    }
}
