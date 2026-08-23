// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::set_color` against a real JMAP server — the sync-layer
//! function `source_changed` calls when the calendar-properties dialog's
//! colour picker writes straight to the `ESource`, exercised end to end for
//! the first time.
//!
//! `jmap-cal-sync/tests/live_server.rs` proves `CalSync::free_busy` and
//! `live_server_save.rs` proves `CalSync::save_component`/
//! `remove_component` against the real server, but neither drives
//! `set_color` — the last of this crate's four user-triggered writes (see
//! `docs/NIGHT-LOG.md`'s Track B1 "`jmap-cal-sync` now has all four of its
//! user-triggered writes traced" note) to get a live-server test. Only
//! `jmap-mockd` has ever exercised it (`jmap-cal-sync/tests/color.rs`); this
//! file is its live-server counterpart, following the same recipe as this
//! crate's other live-server files.
//!
//! ## Running it
//!
//! Same environment as this crate's other live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_color -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like this crate's other
//! live-server files, `#[ignore]` alone already keeps it out of a plain
//! `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment.

use std::env;

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_proto::calendars::Calendar;
use jmap_proto::session::CAPABILITY_CALENDARS;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover calendar can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server_save.rs::connect_for_write`
/// exactly.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// Creates a throwaway calendar, pushes a colour via `CalSync::set_color`,
/// confirms it via a fresh `Calendar/get`, clears it (`None`), confirms the
/// clear, then destroys the calendar.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn setting_then_clearing_a_calendar_colour_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");

    let display_name = format!("agent-calsync-color-{}", unique_suffix());
    let created = client
        .calendar_create(
            &account_id,
            &Calendar {
                name: display_name,
                ..Default::default()
            },
        )
        .expect("Calendar/set create failed against the real server");
    let calendar_id = created.id.clone().expect("the server named the calendar");
    assert_eq!(
        created.color, None,
        "a freshly created calendar should start without a colour"
    );

    let sync = CalSync::new(client, account_id.clone(), calendar_id.clone());

    sync.set_color(Some("#00ff00"))
        .expect("Calendar/set colour update failed against the real server");
    let coloured = sync
        .client()
        .calendars(&account_id)
        .expect("listing calendars failed")
        .into_iter()
        .find(|calendar| calendar.id.as_ref() == Some(&calendar_id))
        .expect("the calendar we just created should still be listed");
    assert_eq!(
        coloured.color.as_deref(),
        Some("#00ff00"),
        "the pushed colour should be visible on reload"
    );

    sync.set_color(None)
        .expect("Calendar/set colour clear failed against the real server");
    let cleared = sync
        .client()
        .calendars(&account_id)
        .expect("listing calendars failed")
        .into_iter()
        .find(|calendar| calendar.id.as_ref() == Some(&calendar_id))
        .expect("the calendar we just created should still be listed");
    assert_eq!(
        cleared.color, None,
        "clearing the colour should be visible on reload"
    );

    sync.client()
        .calendar_destroy(sync.account_id(), &calendar_id)
        .expect("Calendar/set destroy failed against the real server");
}
