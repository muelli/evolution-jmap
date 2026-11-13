// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Verify that the parser does not panic and parsed events reach a fixed point.
    if let Ok(text) = std::str::from_utf8(data) {
        // Exercise time zone and free/busy lookup helpers with arbitrary text.
        let _ = jmap_ical::unique_tzid_to_iana(text);
        let _ = jmap_ical::windows_time_zone_to_iana(text);
        let _ = jmap_ical::resolve_canonical_time_zone(text);
        let _ = jmap_ical::names_time_zone(text);
        let _ = jmap_ical::free_busy_type(text);

        if let Ok(event1) = jmap_ical::ical_to_event(text) {
            let ical2 = jmap_ical::event_to_ical(&event1);
            if let Ok(event2) = jmap_ical::ical_to_event(&ical2) {
                let ical3 = jmap_ical::event_to_ical(&event2);
                assert_eq!(
                    ical2, ical3,
                    "Export_2 and Export_3 must be identical fixed point"
                );
                if let Ok(event3) = jmap_ical::ical_to_event(&ical3) {
                    assert_eq!(
                        event2, event3,
                        "Event_2 and Event_3 must be identical fixed point"
                    );
                }
            }

            // Exercise scheduling iCalendar serialization across series and overrides.
            let _ = jmap_ical::scheduling_ical(&event1, "REQUEST", None);
            let _ = jmap_ical::scheduling_ical(&event1, "REPLY", None);
            let _ = jmap_ical::scheduling_ical(&event1, "CANCEL", None);
            if let Some(ref overrides) = event1.recurrence_overrides {
                for recurrence_id in overrides.keys() {
                    let _ = jmap_ical::scheduling_ical(&event1, "REQUEST", Some(recurrence_id));
                }
            }

            // Exercise property mapping queries and time zone pruning.
            let _ = jmap_ical::maps_alerts(&event1);
            let _ = jmap_ical::maps_time_zone(&event1);

            if let Some(ref tzid) = event1.time_zone {
                let _ = jmap_ical::unique_tzid_to_iana(tzid);
                let _ = jmap_ical::windows_time_zone_to_iana(tzid);
                let _ = jmap_ical::resolve_canonical_time_zone(tzid);
                let _ = jmap_ical::time_zone_definition(&event1, tzid);
                let _ = jmap_ical::names_time_zone(tzid);
                let _ = jmap_ical::defines_time_zone(&event1, tzid);
            }

            if let Some(ref rule) = event1.recurrence_rule {
                let _ = jmap_ical::unstateable_until(rule);
                let _ = jmap_ical::maps_recurrence_rule(rule);
            }

            if let Some(ref locations) = event1.locations {
                let _ = jmap_ical::maps_locations(locations);
            }

            if let Some(ref vlocations) = event1.virtual_locations {
                let _ = jmap_ical::maps_virtual_locations(vlocations);
            }

            if let Some(ref overrides) = event1.recurrence_overrides {
                for (recurrence_id, patch) in overrides {
                    let _ = jmap_ical::maps_recurrence_override(&event1, recurrence_id, patch);
                    let _ = jmap_ical::sends_recurrence_override(&event1, recurrence_id, patch);
                }
            }

            let mut pruned = event1.clone();
            jmap_ical::prune_time_zones(&mut pruned);
        }
    }
});
