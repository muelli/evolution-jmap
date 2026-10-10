// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Real-size scale tests for `jmap-ical`.
//!
//! Validates mapping correctness, stability, and throughput across a realistic
//! corpus of calendar events:
//! - Recurring events with overrides and VALARMs.
//! - Events with custom solidus-prefixed VTIMEZONE definitions.
//! - Point-in-time zero-duration milestone events.
//! - DST-transition-spanning events.
//! - Standard multi-day and timed events across major exporters.

use std::env;
use std::time::Instant;

use jmap_ical::{event_to_ical, ical_to_event};

const APPLE: &str = include_str!("fixtures/apple_calendar_export.ics");
const CUSTOM_TZ: &str = include_str!("fixtures/custom_vtimezone_export.ics");
const CYRUS: &str = include_str!("fixtures/cyrus_caldav_export.ics");
const DST_SPAN: &str = include_str!("fixtures/dst_transition_spanning_export.ics");
const EVOLUTION: &str = include_str!("fixtures/evolution_calendar_export.ics");
const GOOGLE: &str = include_str!("fixtures/google_calendar_export.ics");
const NEXTCLOUD: &str = include_str!("fixtures/nextcloud_calendar_export.ics");
const OUTLOOK: &str = include_str!("fixtures/outlook_m365_export.ics");
const POINT_IN_TIME: &str = include_str!("fixtures/point_in_time_milestone_export.ics");
const SOGO: &str = include_str!("fixtures/sogo_calendar_export.ics");
const THUNDERBIRD: &str = include_str!("fixtures/thunderbird_calendar_export.ics");
const THUNDERBIRD_DETACHED: &str = include_str!("fixtures/thunderbird_detached_export.ics");

fn extract_event_uid(ics: &str) -> String {
    let mut in_vevent = false;
    for line in ics.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("BEGIN:VEVENT") {
            in_vevent = true;
        } else if trimmed.eq_ignore_ascii_case("END:VEVENT") {
            in_vevent = false;
        } else if in_vevent && trimmed.starts_with("UID:") {
            return trimmed.strip_prefix("UID:").unwrap_or("").to_owned();
        }
    }
    String::new()
}

fn build_test_corpus(total_events: usize) -> Vec<String> {
    let mut corpus = Vec::with_capacity(total_events);

    let recurring_count = total_events / 3;
    let custom_tz_count = (total_events / 20).max(5);
    let standard_count = total_events.saturating_sub(recurring_count + custom_tz_count);

    let google_uid = extract_event_uid(GOOGLE);
    let tb_detached_uid = extract_event_uid(THUNDERBIRD_DETACHED);
    let custom_tz_uid = extract_event_uid(CUSTOM_TZ);

    let standard_fixtures: [(&str, String); 9] = [
        (APPLE, extract_event_uid(APPLE)),
        (CYRUS, extract_event_uid(CYRUS)),
        (DST_SPAN, extract_event_uid(DST_SPAN)),
        (EVOLUTION, extract_event_uid(EVOLUTION)),
        (NEXTCLOUD, extract_event_uid(NEXTCLOUD)),
        (OUTLOOK, extract_event_uid(OUTLOOK)),
        (POINT_IN_TIME, extract_event_uid(POINT_IN_TIME)),
        (SOGO, extract_event_uid(SOGO)),
        (THUNDERBIRD, extract_event_uid(THUNDERBIRD)),
    ];

    let mut event_idx = 0;

    // 1. Recurring with overrides and alarms
    for i in 0..recurring_count {
        let (template, old_uid) = if i % 2 == 0 {
            (GOOGLE, &google_uid)
        } else {
            (THUNDERBIRD_DETACHED, &tb_detached_uid)
        };
        let new_uid = format!("test-rec-{event_idx}@example.org");
        let new_summary = format!("SUMMARY:Recurring Event #{event_idx}");
        let renamed = template
            .replace(old_uid, &new_uid)
            .replace("SUMMARY:", &new_summary);
        corpus.push(renamed);
        event_idx += 1;
    }

    // 2. Custom VTIMEZONE definitions
    for i in 0..custom_tz_count {
        let zone_suffix = i % 10;
        let new_zone = format!("/example.org/Custom_Zone_{zone_suffix}");
        let new_uid = format!("test-custom-tz-{event_idx}@example.org");
        let new_summary = format!("SUMMARY:Custom Timezone Event #{event_idx}");
        let renamed = CUSTOM_TZ
            .replace(&custom_tz_uid, &new_uid)
            .replace("/example.org/Custom_Eastern", &new_zone)
            .replace("SUMMARY:", &new_summary);
        corpus.push(renamed);
        event_idx += 1;
    }

    // 3. Standard fixtures
    for i in 0..standard_count {
        let (template, old_uid) = &standard_fixtures[i % standard_fixtures.len()];
        let new_uid = format!("test-std-{event_idx}@example.org");
        let new_summary = format!("SUMMARY:Standard Event #{event_idx}");
        let renamed = template
            .replace(old_uid, &new_uid)
            .replace("SUMMARY:", &new_summary);
        corpus.push(renamed);
        event_idx += 1;
    }

    corpus
}

#[test]
fn realistic_corpus_roundtrip_at_scale() {
    let total_events: usize = env::var("SCALE_TEST_EVENTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);

    let corpus = build_test_corpus(total_events);
    assert_eq!(corpus.len(), total_events);

    let start = Instant::now();
    for (i, ics) in corpus.iter().enumerate() {
        let event = ical_to_event(ics).unwrap_or_else(|err| {
            panic!("Event #{i} failed to parse: {err:?}");
        });

        assert!(
            event.uid.is_some() || event.id.is_some(),
            "Event #{i} must carry an identifier"
        );

        let emitted = event_to_ical(&event);
        assert!(
            emitted.starts_with("BEGIN:VCALENDAR"),
            "Event #{i} output must be a valid VCALENDAR"
        );
        assert!(
            emitted.ends_with("END:VCALENDAR\r\n") || emitted.ends_with("END:VCALENDAR\n"),
            "Event #{i} output must terminate with END:VCALENDAR"
        );

        let roundtrip = ical_to_event(&emitted).unwrap_or_else(|err| {
            panic!("Event #{i} re-parse failed: {err:?}");
        });

        assert_eq!(event.title, roundtrip.title, "Title preserved on #{i}");
        assert_eq!(event.start, roundtrip.start, "Start preserved on #{i}");
        assert_eq!(
            event.time_zone, roundtrip.time_zone,
            "TimeZone preserved on #{i}"
        );
    }
    let elapsed = start.elapsed();
    eprintln!(
        "SCALE TEST: {total_events} events verified in {:.3} s ({:.1} events/s)",
        elapsed.as_secs_f64(),
        (total_events as f64) / elapsed.as_secs_f64()
    );
}

#[test]
#[ignore = "explicit full 10,000 event scale test; run with -- --ignored"]
fn realistic_corpus_10k_full_scale() {
    let corpus = build_test_corpus(10_000);
    assert_eq!(corpus.len(), 10_000);

    let start = Instant::now();
    for (i, ics) in corpus.iter().enumerate() {
        let event = ical_to_event(ics).expect("parse 10k");
        let emitted = event_to_ical(&event);
        let back = ical_to_event(&emitted).expect("reparse 10k");
        assert_eq!(event.title, back.title, "Title mismatch #{i}");
    }
    let elapsed = start.elapsed();
    eprintln!(
        "SCALE TEST 10K: 10,000 events verified in {:.3} s ({:.1} events/s)",
        elapsed.as_secs_f64(),
        10_000.0 / elapsed.as_secs_f64()
    );
}
