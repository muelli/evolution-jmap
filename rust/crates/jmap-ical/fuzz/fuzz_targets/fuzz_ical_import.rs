// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // Exercise time zone and free/busy lookup helpers with arbitrary text.
        let _ = jmap_ical::unique_tzid_to_iana(text);
        let _ = jmap_ical::windows_time_zone_to_iana(text);
        let _ = jmap_ical::resolve_canonical_time_zone(text);
        let _ = jmap_ical::names_time_zone(text);
        let _ = jmap_ical::free_busy_type(text);

        // Exercise availability to VFREEBUSY conversion with arbitrary attendee and dates.
        let date = jmap_proto::state::UtcDate::new(text);
        let period = jmap_proto::principals::BusyPeriod::new(date.clone(), date.clone(), text);
        let _ = jmap_ical::busy_periods_to_vfreebusy(text, &date, &date, &[period]);

        if let Ok(event) = jmap_ical::ical_to_event(text) {
            let ical2 = jmap_ical::event_to_ical(&event);
            let _ = jmap_ical::ical_to_event(&ical2);

            // Exercise scheduling iCalendar serialization across series and overrides.
            let _ = jmap_ical::scheduling_ical(&event, "REQUEST", None);
            let _ = jmap_ical::scheduling_ical(&event, "REPLY", None);
            let _ = jmap_ical::scheduling_ical(&event, "CANCEL", None);
            let _ = jmap_ical::scheduling_ical(&event, text, None);
            if let Some(ref overrides) = event.recurrence_overrides {
                for recurrence_id in overrides.keys() {
                    let _ = jmap_ical::scheduling_ical(&event, "REQUEST", Some(recurrence_id));
                    let _ = jmap_ical::scheduling_ical(&event, "REPLY", Some(recurrence_id));
                    let _ = jmap_ical::scheduling_ical(&event, "CANCEL", Some(recurrence_id));
                }
            }

            // Exercise property mapping queries.
            let _ = jmap_ical::maps_alerts(&event);
            let _ = jmap_ical::maps_time_zone(&event);

            if let Some(ref tzid) = event.time_zone {
                let _ = jmap_ical::unique_tzid_to_iana(tzid);
                let _ = jmap_ical::windows_time_zone_to_iana(tzid);
                let _ = jmap_ical::resolve_canonical_time_zone(tzid);
                let _ = jmap_ical::time_zone_definition(&event, tzid);
                let _ = jmap_ical::names_time_zone(tzid);
                let _ = jmap_ical::defines_time_zone(&event, tzid);
            }

            if let Some(ref rule) = event.recurrence_rule {
                let _ = jmap_ical::unstateable_until(rule);
                let _ = jmap_ical::maps_recurrence_rule(rule);
            }

            if let Some(ref locations) = event.locations {
                let _ = jmap_ical::maps_locations(locations);
            }

            if let Some(ref vlocations) = event.virtual_locations {
                let _ = jmap_ical::maps_virtual_locations(vlocations);
            }

            if let Some(ref overrides) = event.recurrence_overrides {
                for (recurrence_id, patch) in overrides {
                    let _ = jmap_ical::maps_recurrence_override(&event, recurrence_id, patch);
                    let _ = jmap_ical::sends_recurrence_override(&event, recurrence_id, patch);
                }
            }

            let mut pruned = event.clone();
            jmap_ical::prune_time_zones(&mut pruned);
            let _ = jmap_ical::event_to_ical(&pruned);
        }
    }
});
