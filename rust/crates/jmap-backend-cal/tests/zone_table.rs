// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Concurrent loads that resolve zones outside libical's `zone.tab`.
//!
//! libical 3.0 appends such a zone to its shared builtin table without a lock.
//! A thread that scans the table during the append can read a half-written entry.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

use jmap_backend_cal::marshal;

/// Path to the system zoneinfo directory.
const ZONEINFO: &str = "/usr/share/zoneinfo";

/// Number of threads that load events at the same time.
const THREADS: usize = 12;

/// Returns the zone names that `zone.tab` lists.
fn listed_zones(root: &Path) -> HashSet<String> {
    let table = std::fs::read_to_string(root.join("zone.tab")).unwrap_or_default();
    table
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split('\t').nth(2))
        .map(str::to_owned)
        .collect()
}

/// Returns the TZif zone names that `zone.tab` does not list, sorted.
fn unlisted_zones() -> Vec<String> {
    let root = PathBuf::from(ZONEINFO);
    let listed = listed_zones(&root);
    let mut names = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(name) = path.strip_prefix(&root) else {
                continue;
            };
            let name = name.to_string_lossy().into_owned();
            if name.starts_with("posix") || name.starts_with("right") || name == "localtime" {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if !listed.contains(&name)
                && std::fs::read(&path).is_ok_and(|bytes| bytes.starts_with(b"TZif"))
            {
                names.push(name);
            }
        }
    }
    names.sort();
    names
}

/// Returns an event that starts in the zone `tzid`.
fn event_in(tzid: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:Z1\r\n\
         DTSTART;TZID={tzid}:20260810T090000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// Verifies parallel loads that add zones to libical's table do not crash.
#[test]
fn parallel_loads_of_zones_outside_the_zone_table_do_not_crash() {
    let zones = Arc::new(unlisted_zones());
    assert!(
        zones.len() > 50,
        "too few unlisted zones in {ZONEINFO} to race: {}",
        zones.len()
    );
    let start = Arc::new(Barrier::new(THREADS));
    let workers: Vec<_> = (0..THREADS)
        .map(|offset| {
            let zones = Arc::clone(&zones);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                // Each thread starts at a different zone, so first lookups overlap.
                let count = zones.len();
                for index in 0..count {
                    let tzid = &zones[(index + offset * count / THREADS) % count];
                    let component = marshal::component_from_ical(&event_in(tzid));
                    assert!(!component.is_null(), "{tzid}: not read as an event");
                    // SAFETY: releases the reference from `component_from_ical`.
                    unsafe { marshal::component_unref(component) };
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("a loading thread panicked");
    }
}
