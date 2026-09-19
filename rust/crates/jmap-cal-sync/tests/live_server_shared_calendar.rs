// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync` reading and writing a *shared* calendar's actual event content,
//! i.e. one owned by a different real account, through an ACL grant.
//!
//! `tests/live_server_shared_freebusy.rs` already proves a `mayReadFreeBusy`
//! grant lets a second account see the busy/free shape of another account's
//! calendar via `Principal/getAvailability`, but that method never reveals
//! event content and does not require `mayReadItems` on the calendar itself.
//! This file closes the sibling gap: does a `mayReadItems`/`mayWriteOwn`
//! grant on the calendar let `CalSync` itself read, and then write, the
//! shared calendar's
//! events, the same way `jmap-book-sync/tests/live_server_shared_book.rs`
//! pinned down for a shared address book. This is exactly the path a real
//! Evolution session takes once a shared calendar is added as a source.
//!
//! Before any grant, the real server was observed answering both ways for
//! address books: a method-level `forbidden`, and (more often) a successful
//! but empty listing. This test tolerates either for `CalendarEvent`, the
//! same way.
//!
//! ## Running it
//!
//! Needs both `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` and
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` from
//! `docs/manual-test-live-server.md`, since this is inherently a two-account
//! test: the write-test account owns the shared calendar, the recipient
//! account is granted access to it.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync -- --ignored
//! ```
//!
//! Skipped, not failed, when either pair is unset.

use std::env;

use jmap_cal_sync::{CalSync, SyncError};
use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_proto::calendars::Calendar;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_PRINCIPALS};
use serde_json::json;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `live_server_shared_freebusy.rs::connect_owner`.
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

/// Mirrors `live_server_shared_freebusy.rs::connect_recipient`.
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

/// Mirrors `jmap-book-sync/tests/live_server_shared_book.rs::is_forbidden`: a
/// missing grant can surface as a whole method refused outright, or a create
/// let through method dispatch but rejected per-record.
fn is_forbidden(error: &SyncError) -> bool {
    matches!(
        error,
        SyncError::Client(ClientError::Method(method_error))
            if method_error.error_type == "forbidden"
    ) || matches!(
        error,
        SyncError::Client(ClientError::Set(set_error))
            if set_error.error_type == "forbidden"
    )
}

/// Mirrors `assert_owner_contact_not_visible`: the real server answers either
/// a method-level `forbidden` or a successful but empty/filtered listing for
/// an absent grant, and both mean the same thing here.
fn assert_owner_event_not_visible(recipient_sync: &CalSync, owner_uid: &str, when: &str) {
    match recipient_sync.list_existing() {
        Err(error) => assert!(
            is_forbidden(&error),
            "expected forbidden {when}, got: {error:?}"
        ),
        Ok((_, events)) => assert!(
            events.iter().all(|event| event.uid != owner_uid),
            "the owner's event should not be visible {when}: {:?}",
            events.iter().map(|e| &e.uid).collect::<Vec<_>>()
        ),
    }
}

/// `summary` also seeds the local `UID`: the JSCalendar `uid` a create falls
/// back to when the ICS carries no better source, so a fixed literal here
/// would collide once both the owner's and the recipient's events land in
/// the same shared calendar.
fn ics(summary: &str, hour: u8) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{summary}@localhost\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T{hour:02}0000\r\n\
         DURATION:PT1H\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

/// Creates a fresh calendar and an event on the owner account, then drives
/// the recipient account's own `CalSync` against that same calendar (owner's
/// account id, owner's calendar id) through three stages: forbidden before
/// any grant, read-only once `mayRead` is granted (sees the owner's event,
/// but a write is still refused), and read-write once the grant widens to
/// `mayWrite` (can create an event the owner then sees too). Destroys the
/// calendar (and everything in it) at the end regardless of where the
/// assertions land.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_calendars_events_are_readable_then_writable_as_the_grant_widens() {
    let Some((owner, _owner_address)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared calendar test"
        );
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the shared calendar test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the shared calendar test"
        );
        return;
    }

    let owner_account_id = owner
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");

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
    let calendar_name = format!("agent-calsync-shared-{suffix}");
    let calendar = owner
        .calendar_create(&owner_account_id, &Calendar::new(calendar_name))
        .expect("Calendar/set create failed against the real server");
    let calendar_id = calendar
        .id
        .clone()
        .expect("the server named the new calendar");

    let owner_sync = CalSync::new(owner, owner_account_id.clone(), calendar_id.clone());

    let owner_summary = format!("agent-calsync-shared-owner-{suffix}");
    let owner_saved = owner_sync
        .save_component(&ics(&owner_summary, 9), None)
        .expect("CalendarEvent/set create failed against the real server");

    let recipient_sync = CalSync::new(recipient, owner_account_id.clone(), calendar_id.clone());

    assert_owner_event_not_visible(&recipient_sync, &owner_saved.uid, "before any grant");

    owner_sync
        .client()
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true}}}),
        )
        .expect("Calendar/set shareWith failed against the real server");

    let (_, listed) = recipient_sync
        .list_existing()
        .expect("a mayReadItems grant should let the recipient list the shared calendar");
    assert!(
        listed.iter().any(|event| event.uid == owner_saved.uid),
        "the owner's event should be visible through the shared calendar: {:?}",
        listed.iter().map(|e| &e.uid).collect::<Vec<_>>()
    );

    let recipient_summary = format!("agent-calsync-shared-recipient-{suffix}");
    let write_without_grant = recipient_sync
        .save_component(&ics(&recipient_summary, 11), None)
        .expect_err("a mayReadItems-only grant must not allow creating an event");
    assert!(
        is_forbidden(&write_without_grant),
        "expected forbidden for a write under a read-only grant, got: {write_without_grant:?}"
    );

    owner_sync
        .client()
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true, "mayWriteOwn": true, "mayWriteAll": true}}}),
        )
        .expect("Calendar/set shareWith widen failed against the real server");

    let recipient_saved = recipient_sync
        .save_component(&ics(&recipient_summary, 11), None)
        .expect(
            "a mayWriteOwn/mayWriteAll grant should let the recipient create an event in the shared calendar",
        );

    let (_, listed_after_write) = owner_sync
        .list_existing()
        .expect("listing the calendar as its owner failed after the recipient's write");
    assert!(
        listed_after_write
            .iter()
            .any(|event| event.uid == recipient_saved.uid),
        "the event the recipient created should be visible to the owner too: {:?}",
        listed_after_write
            .iter()
            .map(|e| &e.uid)
            .collect::<Vec<_>>()
    );

    owner_sync
        .remove_component(&recipient_saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server (cleanup)");
    owner_sync
        .remove_component(&owner_saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server (cleanup)");
    owner_sync
        .client()
        .calendar_destroy(&owner_account_id, &calendar_id)
        .expect("Calendar/set destroy failed against the real server (cleanup)");
}
