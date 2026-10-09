// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Attempt parsing arbitrary bytes as UTF-8 iCalendar text.
    // Parser errors (Err) are expected and normal for hostile/malformed input:
    // this fuzzer asserts no panic occurs and that validly parsed events reach
    // a fixed point after the second round trip (docs/ICAL-MAPPING.md Section 6).
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(event1) = jmap_ical::ical_to_event(text) {
            // Pass 2: Canonical Outbound serialization
            let ical2 = jmap_ical::event_to_ical(&event1);
            // Pass 3: Re-import and re-export for fixed-point stability
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
        }
    }
});
