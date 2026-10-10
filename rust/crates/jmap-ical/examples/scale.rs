// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Real-size benchmark and profiling harness for `jmap-ical`.
//!
//! Evaluates `ical_to_event` and `event_to_ical` across a realistic corpus of
//! 10,000 events synthesized from the 12 production calendar export fixtures:
//! - One third recurring with overrides and alarms (from Google and Thunderbird).
//! - Several custom solidus-prefixed VTIMEZONE definitions.
//! - Diverse single-day, multi-day, all-day, zero-duration, and DST-spanning events.
//!
//! Measures single-threaded elapsed time, throughput, and peak RSS.

use std::env;
use std::time::Instant;

use jmap_ical::{event_to_ical, ical_to_event};

const APPLE: &str = include_str!("../tests/fixtures/apple_calendar_export.ics");
const CUSTOM_TZ: &str = include_str!("../tests/fixtures/custom_vtimezone_export.ics");
const CYRUS: &str = include_str!("../tests/fixtures/cyrus_caldav_export.ics");
const DST_SPAN: &str = include_str!("../tests/fixtures/dst_transition_spanning_export.ics");
const EVOLUTION: &str = include_str!("../tests/fixtures/evolution_calendar_export.ics");
const GOOGLE: &str = include_str!("../tests/fixtures/google_calendar_export.ics");
const NEXTCLOUD: &str = include_str!("../tests/fixtures/nextcloud_calendar_export.ics");
const OUTLOOK: &str = include_str!("../tests/fixtures/outlook_m365_export.ics");
const POINT_IN_TIME: &str = include_str!("../tests/fixtures/point_in_time_milestone_export.ics");
const SOGO: &str = include_str!("../tests/fixtures/sogo_calendar_export.ics");
const THUNDERBIRD: &str = include_str!("../tests/fixtures/thunderbird_calendar_export.ics");
const THUNDERBIRD_DETACHED: &str =
    include_str!("../tests/fixtures/thunderbird_detached_export.ics");

/// Extracts the master event UID from an iCalendar string.
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

/// The resident set size this process has peaked at, read from `/proc`.
pub fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|value| value.trim().parse().ok())
    })
}

/// Category classification of an event in the benchmark corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventCategory {
    RecurringWithOverrides,
    CustomTimezone,
    Standard,
}

/// An entry in the benchmark corpus.
pub struct CorpusEntry {
    pub ics: String,
    pub category: EventCategory,
}

/// Generates a corpus of `total_events` iCalendar documents.
pub fn build_realistic_corpus(total_events: usize) -> Vec<CorpusEntry> {
    let mut corpus = Vec::with_capacity(total_events);

    let recurring_count = total_events / 3;
    let custom_tz_count = (total_events / 20).max(10);
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

    // 1. One third recurring with overrides and alarms (Google and Thunderbird Detached)
    for i in 0..recurring_count {
        let (template, old_uid) = if i % 2 == 0 {
            (GOOGLE, &google_uid)
        } else {
            (THUNDERBIRD_DETACHED, &tb_detached_uid)
        };
        let new_uid = format!("event-rec-{event_idx}@example.org");
        let new_summary = format!("SUMMARY:Recurring Event #{event_idx}");
        let renamed = template
            .replace(old_uid, &new_uid)
            .replace("SUMMARY:", &new_summary);
        corpus.push(CorpusEntry {
            ics: renamed,
            category: EventCategory::RecurringWithOverrides,
        });
        event_idx += 1;
    }

    // 2. Custom VTIMEZONE definitions
    for i in 0..custom_tz_count {
        let zone_suffix = i % 10;
        let new_zone = format!("/example.org/Custom_Zone_{zone_suffix}");
        let new_uid = format!("event-custom-tz-{event_idx}@example.org");
        let new_summary = format!("SUMMARY:Custom Timezone Event #{event_idx}");
        let renamed = CUSTOM_TZ
            .replace(&custom_tz_uid, &new_uid)
            .replace("/example.org/Custom_Eastern", &new_zone)
            .replace("SUMMARY:", &new_summary);
        corpus.push(CorpusEntry {
            ics: renamed,
            category: EventCategory::CustomTimezone,
        });
        event_idx += 1;
    }

    // 3. Standard and diverse events across the remaining fixtures
    for i in 0..standard_count {
        let (template, old_uid) = &standard_fixtures[i % standard_fixtures.len()];
        let new_uid = format!("event-std-{event_idx}@example.org");
        let new_summary = format!("SUMMARY:Standard Event #{event_idx}");
        let renamed = template
            .replace(old_uid, &new_uid)
            .replace("SUMMARY:", &new_summary);
        corpus.push(CorpusEntry {
            ics: renamed,
            category: EventCategory::Standard,
        });
        event_idx += 1;
    }

    corpus
}

#[derive(Default)]
struct CategoryStats {
    count: usize,
    inbound_total_us: f64,
    outbound_total_us: f64,
}

fn main() {
    let total_events: usize = env::var("SCALE_EVENTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);

    println!("Building realistic corpus of {total_events} events...");
    let corpus_start = Instant::now();
    let corpus = build_realistic_corpus(total_events);
    let corpus_elapsed = corpus_start.elapsed();
    println!(
        "Corpus constructed in {:.2}ms ({} events)",
        corpus_elapsed.as_secs_f64() * 1000.0,
        corpus.len()
    );

    let start_rss = peak_rss_kb().unwrap_or(0);
    println!(
        "Starting peak RSS (after corpus load): {start_rss} kB ({:.1} MB)",
        (start_rss as f64) / 1024.0
    );

    // Phase 0: Streaming round-trip pipeline (process and discard each event)
    println!("\nPhase 0: Streaming round-trip (.ics -> JSCalendar -> .ics, discarded)...");
    let stream_start = Instant::now();
    for entry in &corpus {
        let event = ical_to_event(&entry.ics).expect("parse");
        let _ = event_to_ical(&event);
    }
    let stream_elapsed = stream_start.elapsed();
    let stream_rss = peak_rss_kb().unwrap_or(0);
    println!(
        "Streaming pipeline completed in {:.3} s ({:.1} round-trips/s, {:.2} µs/event)",
        stream_elapsed.as_secs_f64(),
        (total_events as f64) / stream_elapsed.as_secs_f64(),
        (stream_elapsed.as_secs_f64() * 1_000_000.0) / (total_events as f64)
    );
    println!(
        "Streaming peak RSS: {stream_rss} kB ({:.1} MB)",
        (stream_rss as f64) / 1024.0
    );

    // Phase 1: Inbound mapping (.ics -> JSCalendar CalendarEvent)
    println!("\nPhase 1: Inbound mapping (.ics -> JSCalendar CalendarEvent)...");
    let parse_start = Instant::now();
    let mut jmap_events = Vec::with_capacity(total_events);
    let mut rec_inbound_us = 0.0;
    let mut ctz_inbound_us = 0.0;
    let mut std_inbound_us = 0.0;

    for (i, entry) in corpus.iter().enumerate() {
        let t0 = Instant::now();
        match ical_to_event(&entry.ics) {
            Ok(event) => {
                let dt = t0.elapsed().as_secs_f64() * 1_000_000.0;
                match entry.category {
                    EventCategory::RecurringWithOverrides => rec_inbound_us += dt,
                    EventCategory::CustomTimezone => ctz_inbound_us += dt,
                    EventCategory::Standard => std_inbound_us += dt,
                }
                jmap_events.push(event);
            }
            Err(err) => {
                eprintln!("Failed to parse event {i}: {err:?}");
                std::process::exit(1);
            }
        }
    }
    let parse_elapsed = parse_start.elapsed();
    let parse_sec = parse_elapsed.as_secs_f64();
    let parse_rate = (total_events as f64) / parse_sec;
    let parse_per_event_us = (parse_sec * 1_000_000.0) / (total_events as f64);
    let mid_rss = peak_rss_kb().unwrap_or(0);

    println!("Inbound parse completed:");
    println!("  Total time:       {:.3} s", parse_sec);
    println!("  Throughput:       {:.1} events/s", parse_rate);
    println!("  Per-event avg:    {:.2} µs", parse_per_event_us);
    println!(
        "  Peak RSS so far:  {mid_rss} kB ({:.1} MB)",
        (mid_rss as f64) / 1024.0
    );

    // Phase 2: Outbound mapping (JSCalendar CalendarEvent -> .ics)
    println!("\nPhase 2: Outbound mapping (JSCalendar CalendarEvent -> .ics)...");
    let emit_start = Instant::now();
    let mut emitted_ics = Vec::with_capacity(total_events);
    let mut rec_outbound_us = 0.0;
    let mut ctz_outbound_us = 0.0;
    let mut std_outbound_us = 0.0;

    for (i, event) in jmap_events.iter().enumerate() {
        let t0 = Instant::now();
        let ics = event_to_ical(event);
        let dt = t0.elapsed().as_secs_f64() * 1_000_000.0;
        match corpus[i].category {
            EventCategory::RecurringWithOverrides => rec_outbound_us += dt,
            EventCategory::CustomTimezone => ctz_outbound_us += dt,
            EventCategory::Standard => std_outbound_us += dt,
        }
        emitted_ics.push(ics);
    }
    let emit_elapsed = emit_start.elapsed();
    let emit_sec = emit_elapsed.as_secs_f64();
    let emit_rate = (total_events as f64) / emit_sec;
    let emit_per_event_us = (emit_sec * 1_000_000.0) / (total_events as f64);
    let final_rss = peak_rss_kb().unwrap_or(0);

    println!("Outbound export completed:");
    println!("  Total time:       {:.3} s", emit_sec);
    println!("  Throughput:       {:.1} events/s", emit_rate);
    println!("  Per-event avg:    {:.2} µs", emit_per_event_us);
    println!(
        "  Peak RSS final:   {final_rss} kB ({:.1} MB)",
        (final_rss as f64) / 1024.0
    );

    // Phase 3: Category breakdown
    let mut rec_stats = CategoryStats::default();
    let mut ctz_stats = CategoryStats::default();
    let mut std_stats = CategoryStats::default();

    for entry in &corpus {
        match entry.category {
            EventCategory::RecurringWithOverrides => rec_stats.count += 1,
            EventCategory::CustomTimezone => ctz_stats.count += 1,
            EventCategory::Standard => std_stats.count += 1,
        }
    }
    rec_stats.inbound_total_us = rec_inbound_us;
    rec_stats.outbound_total_us = rec_outbound_us;
    ctz_stats.inbound_total_us = ctz_inbound_us;
    ctz_stats.outbound_total_us = ctz_outbound_us;
    std_stats.inbound_total_us = std_inbound_us;
    std_stats.outbound_total_us = std_outbound_us;

    println!("\nPer-Category Breakdown (Single-Threaded):");
    println!(
        "  Recurring with Overrides & Alarms (N = {}):",
        rec_stats.count
    );
    println!(
        "    Inbound:  {:.2} µs/event ({:.1} events/s)",
        rec_stats.inbound_total_us / (rec_stats.count as f64),
        (rec_stats.count as f64) / (rec_stats.inbound_total_us / 1_000_000.0)
    );
    println!(
        "    Outbound: {:.2} µs/event ({:.1} events/s)",
        rec_stats.outbound_total_us / (rec_stats.count as f64),
        (rec_stats.count as f64) / (rec_stats.outbound_total_us / 1_000_000.0)
    );

    println!(
        "  Custom Solidus VTIMEZONE Definitions (N = {}):",
        ctz_stats.count
    );
    println!(
        "    Inbound:  {:.2} µs/event ({:.1} events/s)",
        ctz_stats.inbound_total_us / (ctz_stats.count as f64),
        (ctz_stats.count as f64) / (ctz_stats.inbound_total_us / 1_000_000.0)
    );
    println!(
        "    Outbound: {:.2} µs/event ({:.1} events/s)",
        ctz_stats.outbound_total_us / (ctz_stats.count as f64),
        (ctz_stats.count as f64) / (ctz_stats.outbound_total_us / 1_000_000.0)
    );

    println!("  Standard & Diverse Events (N = {}):", std_stats.count);
    println!(
        "    Inbound:  {:.2} µs/event ({:.1} events/s)",
        std_stats.inbound_total_us / (std_stats.count as f64),
        (std_stats.count as f64) / (std_stats.inbound_total_us / 1_000_000.0)
    );
    println!(
        "    Outbound: {:.2} µs/event ({:.1} events/s)",
        std_stats.outbound_total_us / (std_stats.count as f64),
        (std_stats.count as f64) / (std_stats.outbound_total_us / 1_000_000.0)
    );

    // Phase 4: Sub-stage micro-profiling breakdown
    println!("\nSub-Stage Micro-Profiling Breakdown:");
    let mut check_struct_us = 0.0;
    let mut calcard_parse_us = 0.0;
    let sample_n = 1000.min(total_events);
    for entry in corpus.iter().take(sample_n) {
        let t0 = Instant::now();
        let _ = jmap_ical::event::check_structure(&entry.ics);
        check_struct_us += t0.elapsed().as_secs_f64() * 1_000_000.0;

        let t1 = Instant::now();
        let _ = jmap_ical::event::parse_ical(&entry.ics);
        calcard_parse_us += t1.elapsed().as_secs_f64() * 1_000_000.0;
    }
    let ast_only_us = calcard_parse_us - check_struct_us;
    let total_inbound_sample_us = parse_per_event_us;
    let semantic_mapping_us = total_inbound_sample_us - (calcard_parse_us / (sample_n as f64));

    println!("  Inbound pipeline breakdown (per event):");
    println!(
        "    Envelope validation (check_structure): {:.2} µs",
        check_struct_us / (sample_n as f64)
    );
    println!(
        "    calcard AST lexical parse:             {:.2} µs",
        ast_only_us / (sample_n as f64)
    );
    println!(
        "    Semantic mapping to JSCalendar:        {:.2} µs",
        semantic_mapping_us
    );
    println!(
        "    Total inbound parse:                   {:.2} µs",
        total_inbound_sample_us
    );

    let mut event_calendar_us = 0.0;
    let mut to_ics_us = 0.0;
    for event in jmap_events.iter().take(sample_n) {
        let t0 = Instant::now();
        let comp = jmap_ical::event::event_calendar(event, None);
        event_calendar_us += t0.elapsed().as_secs_f64() * 1_000_000.0;

        let t1 = Instant::now();
        let _ = comp.to_ics();
        to_ics_us += t1.elapsed().as_secs_f64() * 1_000_000.0;
    }
    println!("  Outbound pipeline breakdown (per event):");
    println!(
        "    JSCalendar to calcard Component tree:  {:.2} µs",
        event_calendar_us / (sample_n as f64)
    );
    println!(
        "    calcard Component.to_ics() rendering:  {:.2} µs",
        to_ics_us / (sample_n as f64)
    );
    println!(
        "    Total outbound export:                 {:.2} µs",
        (event_calendar_us + to_ics_us) / (sample_n as f64)
    );

    // Full round-trip summary
    let total_sec = parse_sec + emit_sec;
    let total_rate = (total_events as f64) / total_sec;
    let total_per_event_us = (total_sec * 1_000_000.0) / (total_events as f64);

    println!(
        "\nOverall Summary ({} events, single-threaded):",
        total_events
    );
    println!(
        "  Inbound (.ics -> JSCalendar):  {:.3} s ({:.1} events/s, {:.2} µs/event)",
        parse_sec, parse_rate, parse_per_event_us
    );
    println!(
        "  Outbound (JSCalendar -> .ics): {:.3} s ({:.1} events/s, {:.2} µs/event)",
        emit_sec, emit_rate, emit_per_event_us
    );
    println!(
        "  Total round-trip time:         {:.3} s ({:.1} round-trips/s, {:.2} µs/event)",
        total_sec, total_rate, total_per_event_us
    );
    println!(
        "  Peak RSS (streaming pipeline): {stream_rss} kB ({:.1} MB)",
        (stream_rss as f64) / 1024.0
    );
    println!(
        "  Peak RSS (full retention):     {final_rss} kB ({:.1} MB)",
        (final_rss as f64) / 1024.0
    );
}
