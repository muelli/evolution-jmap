// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::free_busy` asking about a *different* real account's
//! availability — the actual meeting-scheduler shape, unlike
//! `tests/live_server.rs`'s self-lookup, which every account can answer for
//! itself regardless of sharing and so never exercises the permission check
//! at all.
//!
//! `jmap-client/tests/live_server.rs`'s
//! `calendar_shared_with_a_second_real_account_is_visible_only_after_the_grant`
//! already proves a `mayReadFreeBusy` grant changes `Calendar/get`'s
//! `myRights` for the recipient, but never calls `Principal/getAvailability`
//! to check what a scheduler slot-picker actually reads. This file closes
//! that gap for `CalSync::free_busy` itself, and pins down the one behaviour
//! `harness/stalwart/seed-freebusy-fixture.py` (a manual-verification script,
//! not a test) flagged as the most important thing to know here: an
//! ungranted principal is not reported as an error, but as silently free
//! (`Principal/getAvailability` returns an empty list) — indistinguishable
//! from a real empty diary. A share narrows what is *seen*, not how absence
//! is *reported*.
//!
//! ## Running it
//!
//! Needs both `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` (step 3) and
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` (step 3a) from
//! `docs/manual-test-live-server.md`, since this is inherently a two-account
//! test: the write-test account is the one asked about, the recipient
//! account is the one asking.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync -- --ignored
//! ```
//!
//! Skipped, not failed, when either pair is unset.

use std::env;

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_proto::calendars::{Calendar, CalendarEvent};
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_PRINCIPALS};
use serde_json::json;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// An hour-of-day, `01`..=`20`, derived from [`unique_suffix`] rather than
/// fixed: a panicked assertion in a prior run of this same test skips its
/// own cleanup at the bottom, and a fixed hour would then have that leftover
/// calendar's still-shared, still-busy event answer for the *next* run's
/// "before any grant" check too — which is exactly the false positive that
/// bit this test while it was being written. `01..=20` keeps `DTSTART`/
/// `DTEND` inside a single UTC day on both sides of the event.
fn distinct_hour(suffix: u128) -> u8 {
    1 + (suffix % 20) as u8
}

/// Mirrors `jmap-cal-sync/tests/live_server.rs::connect_for_write` exactly:
/// the account whose calendar this test creates an event on and shares.
fn connect_owner() -> Option<(Client, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password))
        .expect("could not fetch the session document for the write-test account");
    Some((client, user))
}

/// Mirrors `jmap-client/tests/live_server.rs::connect_recipient`: the
/// account this test asks `free_busy` from, i.e. the scheduler's caller.
fn connect_recipient() -> Option<(Client, String)> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password))
        .expect("could not fetch the session document for the recipient account");
    Some((client, user))
}

/// Creates a fresh calendar and a one-hour busy event on the owner account,
/// then asks the *recipient* account's `CalSync::free_busy` about the
/// owner's address three times: before any grant (expects the owner's
/// address present but reporting no busy period at all, per the module docs
/// above), after a `mayReadFreeBusy`-only grant on that one calendar
/// (expects the created event's exact window), and after revoking the grant
/// (back to no busy period). Destroys the calendar (and so its event) at the
/// end regardless of where the assertions land.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn free_busy_of_another_account_reflects_only_what_was_shared() {
    let Some((owner, owner_address)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared free/busy test"
        );
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the shared free/busy test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the shared free/busy test"
        );
        return;
    }

    let owner_account_id = owner
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let recipient_account_id = recipient
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the recipient account needs the calendars capability");

    // Who the owner grants `shareWith` to: the recipient's own principal id,
    // resolved from the owner's side by the recipient's email — the reverse
    // direction of what `free_busy` itself resolves (the *asked-about*
    // person's principal id, from the caller's side).
    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_address),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner cannot resolve the recipient's principal id by email");

    let suffix = unique_suffix();
    let hour = distinct_hour(suffix);
    let name = format!("agent-freebusy-shared-{suffix}");
    let calendar = owner
        .calendar_create(&owner_account_id, &Calendar::new(name))
        .expect("Calendar/set create failed against the real server");
    let calendar_id = calendar
        .id
        .clone()
        .expect("the server named the new calendar");

    let event = CalendarEvent::simple(
        calendar_id.clone(),
        "agent-freebusy-shared event",
        &format!("2026-09-22T{hour:02}:00:00"),
        "PT1H",
    );
    let created = owner
        .event_create(&owner_account_id, &event)
        .expect("CalendarEvent/set create failed against the real server");

    // A calendar of the recipient's own to build a `CalSync` on: `free_busy`
    // never touches it, only `self.account_id()`/`self.client()`, but
    // `CalSync::new` still asks for one.
    let recipient_calendar_id = recipient
        .calendars(&recipient_account_id)
        .expect("Calendar/get failed for the recipient account")
        .into_iter()
        .next()
        .expect("the recipient account needs a default calendar")
        .id
        .expect("the server named the recipient's calendar");

    let sync = CalSync::new(recipient, recipient_account_id, recipient_calendar_id);
    let window = (
        "2026-09-22T00:00:00Z".to_string(),
        "2026-09-23T00:00:00Z".to_string(),
    );

    let before = sync
        .free_busy(std::slice::from_ref(&owner_address), &window.0, &window.1)
        .expect("free_busy failed against the real server (before any grant)");
    assert_eq!(
        before.len(),
        1,
        "an address search hit reports the owner even with nothing to see, per RFC 9670's own \
         empty-list-not-error answer for a principal you cannot read"
    );
    assert!(
        !before[0].icalendar.contains("FREEBUSY;FBTYPE=BUSY"),
        "no share exists yet, so the created event must not be visible: {}",
        before[0].icalendar
    );

    owner
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadFreeBusy": true}}}),
        )
        .expect("Calendar/set shareWith failed against the real server");

    let during = sync
        .free_busy(std::slice::from_ref(&owner_address), &window.0, &window.1)
        .expect("free_busy failed against the real server (after the grant)");
    assert_eq!(during.len(), 1);
    assert!(
        during[0].icalendar.contains(&format!(
            "FREEBUSY;FBTYPE=BUSY:20260922T{hour:02}0000Z/20260922T{:02}0000Z",
            hour + 1
        )),
        "a mayReadFreeBusy-only grant (no mayReadItems) should already surface the busy period: {}",
        during[0].icalendar
    );

    owner
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({format!("shareWith/{}", recipient_principal_id.as_str()): null}),
        )
        .expect("Calendar/set revoke failed against the real server");

    let after = sync
        .free_busy(&[owner_address], &window.0, &window.1)
        .expect("free_busy failed against the real server (after the revoke)");
    assert_eq!(after.len(), 1);
    assert!(
        !after[0].icalendar.contains("FREEBUSY;FBTYPE=BUSY"),
        "revoking the grant should hide the busy period again: {}",
        after[0].icalendar
    );

    owner
        .event_destroy(&owner_account_id, &created.id.expect("server-named event"))
        .expect("CalendarEvent/set destroy failed against the real server (cleanup)");
    owner
        .calendar_destroy(&owner_account_id, &calendar_id)
        .expect("Calendar/set destroy failed against the real server (cleanup)");
}
