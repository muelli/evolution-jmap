// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parallel first lookups in libical's builtin zone table.
//!
//! libical 3.0 publishes the table before it reads `zone.tab` into it, and a
//! lookup takes no lock once the table exists. A thread that looks up a zone
//! while another thread fills the table reads a half-written entry. EDS does
//! such lookups without our lock, so the table must be full before any thread
//! starts. Each run of the race needs a new process, thus the test starts
//! this binary again for each run.

use std::ffi::CString;
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;

use eds_sys::i_cal_timezone_get_builtin_timezone;
use jmap_backend_cal::marshal;

/// Tells a process that this binary started to run the race.
const CHILD: &str = "JMAP_ZONE_TABLE_FIRST_USE_CHILD";

/// Number of processes that run the race.
const RUNS: usize = 100;

/// Number of threads that look up zones at the same time.
const THREADS: usize = 16;

/// Number of lookups of each thread.
const LOOKUPS: usize = 50;

/// Returns the zone names of `zone.tab`, the last ones first.
fn listed_zones() -> Vec<CString> {
    let table = std::fs::read_to_string("/usr/share/zoneinfo/zone.tab").unwrap_or_default();
    let mut names: Vec<CString> = table
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split('\t').nth(2))
        .filter_map(|name| CString::new(name).ok())
        .collect();
    names.reverse();
    names
}

/// Returns an event that starts in the zone `tzid`.
fn event_in(tzid: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:Z1\r\n\
         DTSTART;TZID={tzid}:20260810T090000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// Looks up zones from many threads at the same time, as the first users of
/// the table in this process.
fn race() {
    let zones = Arc::new(listed_zones());
    assert!(!zones.is_empty(), "no zones in zone.tab");
    let start = Arc::new(Barrier::new(THREADS));
    let workers: Vec<_> = (0..THREADS)
        .map(|offset| {
            let zones = Arc::clone(&zones);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                for index in 0..LOOKUPS {
                    let name = &zones[(index * THREADS + offset) % zones.len()];
                    if offset % 2 == 0 {
                        // This crate's path, which holds its own lock.
                        let event = event_in(&name.to_string_lossy());
                        let component = marshal::component_from_ical(&event);
                        assert!(!component.is_null(), "{name:?}: not read as an event");
                        // SAFETY: releases the reference from `component_from_ical`.
                        unsafe { marshal::component_unref(component) };
                    } else {
                        // The path of EDS, which does not hold that lock.
                        // SAFETY: `name` is a NUL-terminated string; the zone
                        // is borrowed from the table and not used.
                        let zone = unsafe { i_cal_timezone_get_builtin_timezone(name.as_ptr()) };
                        assert!(!zone.is_null(), "{name:?}: not in the table");
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("a lookup thread panicked");
    }
}

/// Verifies parallel first lookups in a new process do not crash.
#[test]
fn parallel_first_lookups_in_the_zone_table_do_not_crash() {
    if std::env::var_os(CHILD).is_some() {
        race();
        return;
    }
    let binary = std::env::current_exe().expect("no path to this test binary");
    let mut failures = Vec::new();
    for run in 0..RUNS {
        let output = Command::new(&binary)
            .args([
                "--exact",
                "parallel_first_lookups_in_the_zone_table_do_not_crash",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()
            .expect("could not start this test binary again");
        if !output.status.success() {
            failures.push(format!("run {run}: {}", output.status));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {RUNS} runs failed: {failures:?}",
        failures.len()
    );
}
