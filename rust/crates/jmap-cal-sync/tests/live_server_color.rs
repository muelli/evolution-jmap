// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `CalSync::set_color` end to end against a real JMAP server.
//! `source_changed` calls this sync-layer function when the colour picker of
//! the calendar-properties dialog writes directly to the `ESource`.
//!
//! `jmap-cal-sync/tests/live_server.rs` proves `CalSync::free_busy`, and
//! `live_server_save.rs` proves `CalSync::save_component`/`remove_component`.
//! Neither one calls `set_color`, the last of the four user-triggered operations
//! of this crate to get a live-server test. The mock-server counterpart is
//! `jmap-cal-sync/tests/color.rs` (on `jmap-mockd`). This file uses the same
//! recipe as the other live-server files of this crate.
//!
//! ## Running it
//!
//! The environment is the same as for the other live-server tests of this crate.
//! Refer to `docs/manual-test-live-server.md`. Set
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD`, then run:
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_color -- --ignored
//! ```
//!
//! No `--features live-server` gate is necessary. `#[ignore]` alone keeps the
//! test out of a plain `cargo test`.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. When that
//! variable is set, a missing `_WRITE_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.

use jmap_cal_sync::CalSync;
use jmap_proto::calendars::Calendar;
use jmap_proto::session::CAPABILITY_CALENDARS;
use jmap_test_support::{connect_for_write, unique_suffix};

/// Creates a temporary calendar and sets a colour with `CalSync::set_color`.
/// Clears the colour (`None`), and confirms each change with a new `Calendar/get`.
/// Then destroys the calendar.
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
