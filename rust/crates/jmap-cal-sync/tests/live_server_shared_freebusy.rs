// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `CalSync::free_busy` for the availability of a *different* real
//! account. This is the real meeting-scheduler case. The self-lookup in
//! `tests/live_server.rs` does not test the permission check, because each
//! account can always answer for itself.
//!
//! `calendar_shared_with_a_second_real_account_is_visible_only_after_the_grant`
//! in `jmap-client/tests/live_server.rs` proves that a `mayReadFreeBusy` grant
//! changes the `myRights` of `Calendar/get` for the recipient. But it never
//! calls `Principal/getAvailability`, which the scheduler slot-picker reads.
//! This file tests that for `CalSync::free_busy`. It also tests the most
//! important behaviour that manual verification found: the server does not
//! report an ungranted principal as an error, but as silently free.
//! (`Principal/getAvailability` returns an empty list.) This is the same as a
//! real empty diary. A share changes what is *seen*, not how absence is
//! *reported*.
//!
//! ## Running it
//!
//! This is a two-account test: the recipient account asks about the write-test
//! account. Set `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` (step 3) and
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` (step 3a) from
//! `docs/manual-test-live-server.md`.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync -- --ignored
//! ```
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` or
//! `JMAP_LIVE_SERVER_RECIPIENT_USER` is unset, or when the server does not
//! advertise the principals capability. When a `_USER` variable is set, a
//! missing `_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.

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

/// Returns an hour of the day, `01`..=`20`, from [`unique_suffix`]. A failed
/// assertion in a prior run skips the cleanup, so that shared busy event stays.
/// With a fixed hour, it then causes a false positive in the "before any grant"
/// check of the next run. The range `01..=20` keeps `DTSTART`/`DTEND` in one
/// UTC day.
fn distinct_hour(suffix: u128) -> u8 {
    1 + (suffix % 20) as u8
}

/// Same as `jmap-cal-sync/tests/live_server.rs::connect_for_write`.
/// This account owns the calendar that the test creates, fills with an event, and shares.
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

/// Same as `jmap-client/tests/live_server.rs::connect_recipient`, but also
/// returns the user address. This account calls `free_busy`, as the scheduler
/// does.
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

/// Creates a new calendar and a one-hour busy event on the owner account. Then
/// asks the `CalSync::free_busy` of the *recipient* about the owner address
/// three times:
/// - Before any grant: the owner address is present, but with no busy period
///   (refer to the module docs).
/// - After a `mayReadFreeBusy`-only grant on that calendar: the exact window of
///   the event.
/// - After the grant is revoked: no busy period.
///
/// At the end, it destroys the event and the calendar. A failed assertion skips
/// this cleanup.
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

    // The owner finds the `shareWith` principal id of the recipient by email.
    // `free_busy` does the opposite: the caller finds the principal id of the
    // *asked-about* person.
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

    // `CalSync::new` needs a calendar of the recipient. `free_busy` uses only
    // `self.account_id()`/`self.client()`, not the calendar.
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
