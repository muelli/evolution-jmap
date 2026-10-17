// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar backend reading and writing a shared calendar against
//! a real JMAP server.
//!
//! Verifies that `connect::open_calendar_with_account` correctly connects to a
//! calendar in another account shared via RFC 9670 and draft-ietf-jmap-calendars
//! `shareWith`, respects `myRights` to mark the backend read-only
//! (`writable: false`) or writable (`writable: true`), and that the underlying
//! backend sync engine reads and, where allowed, writes calendar event records.
//!
//! ## Running it
//!
//! Needs both `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` and
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` pointing to a real Stalwart
//! server:
//!
//! ```console
//! $ cargo test -p jmap-backend-cal --test live_server_shared -- --ignored
//! ```
//!
//! Skipped, not failed, when either account pair is unset.

use std::env;

use jmap_backend_cal::connect;
use jmap_backend_core::source::{ConnectTarget, SourceConfig};
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

fn connect_owner() -> Option<(Client, String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch session document for write-test account");
    Some((client, user, password, origin))
}

fn connect_recipient() -> Option<(Client, String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch session document for recipient account");
    Some((client, user, password, origin))
}

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

fn ics(summary: &str, hour: u8) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{summary}@localhost\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20261022T{hour:02}0000\r\n\
         DURATION:PT1H\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn shared_calendar_opens_in_backend_as_read_only_then_writable_as_grant_widens() {
    let Some((owner, _owner_address, _owner_password, origin)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set: skipping the shared calendar test"
        );
        return;
    };
    let Some((recipient, recipient_address, recipient_password, _)) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set: skipping the shared calendar test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}: skipping the shared calendar test"
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
        .expect("the owner cannot resolve the recipient principal id by email");

    let suffix = unique_suffix();
    let calendar_name = format!("agent-backend-cal-shared-{suffix}");
    let calendar = owner
        .calendar_create(&owner_account_id, &Calendar::new(calendar_name))
        .expect("Calendar/set create failed against the real server");
    let calendar_id = calendar
        .id
        .clone()
        .expect("the server named the new calendar");

    let owner_sync = CalSync::new(owner, owner_account_id.clone(), calendar_id.clone());

    let owner_summary = format!("agent-backend-cal-shared-owner-{suffix}");
    let owner_saved = owner_sync
        .save_component(&ics(&owner_summary, 9), None)
        .expect("CalendarEvent/set create failed on owner account");

    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let config = SourceConfig {
        target: ConnectTarget::Origin(origin),
        user: Some(recipient_address.clone()),
        resource_id: Some(calendar_id.to_string()),
        rebase_urls: rebase,
    };

    // Stage 1: Before any grant, opening the shared calendar through recipient backend must fail.
    let before_grant = connect::open_calendar_with_account(
        &config,
        Credentials::basic(recipient_address.clone(), recipient_password.clone()),
        Some(owner_account_id.as_str()),
    );
    assert!(
        before_grant.is_err(),
        "opening shared calendar before grant must fail"
    );

    // Stage 2: Grant read-only access (mayReadItems: true).
    owner_sync
        .client()
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true}}}),
        )
        .expect("Calendar/set shareWith failed against the real server");

    let open_cal_ro = connect::open_calendar_with_account(
        &config,
        Credentials::basic(recipient_address.clone(), recipient_password.clone()),
        Some(owner_account_id.as_str()),
    )
    .expect("opening shared calendar with mayReadItems grant should succeed");

    assert!(
        !open_cal_ro.writable,
        "shared calendar with mayReadItems-only grant must open as read-only"
    );

    let (_, listed) = open_cal_ro
        .list_existing()
        .expect("listing events through shared calendar should succeed under mayReadItems");
    assert!(
        listed.iter().any(|e| e.uid == owner_saved.uid),
        "owner event must be visible to recipient in shared calendar"
    );

    let recipient_summary = format!("agent-backend-cal-shared-recipient-{suffix}");
    let write_err = open_cal_ro
        .save_component(&ics(&recipient_summary, 11), None)
        .expect_err("saving event under mayReadItems-only grant must fail");
    assert!(
        is_forbidden(&write_err),
        "saving event under mayReadItems-only grant must be forbidden, got: {write_err:?}"
    );

    // Stage 3: Widen grant to read-write (mayReadItems: true, mayWriteOwn: true, mayWriteAll: true).
    owner_sync
        .client()
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true, "mayWriteOwn": true, "mayWriteAll": true}}}),
        )
        .expect("Calendar/set shareWith widen failed against the real server");

    let open_cal_rw = connect::open_calendar_with_account(
        &config,
        Credentials::basic(recipient_address.clone(), recipient_password),
        Some(owner_account_id.as_str()),
    )
    .expect("opening shared calendar with mayWrite grant should succeed");

    assert!(
        open_cal_rw.writable,
        "shared calendar with mayWrite grant must open as writable"
    );

    let recipient_saved = open_cal_rw
        .save_component(&ics(&recipient_summary, 11), None)
        .expect("saving event under mayWrite grant should succeed");

    let (_, owner_listed) = owner_sync
        .list_existing()
        .expect("owner listing events after recipient write should succeed");
    assert!(
        owner_listed.iter().any(|e| e.uid == recipient_saved.uid),
        "event created by recipient must be visible to owner in shared calendar"
    );

    // Cleanup.
    let _ = owner_sync.remove_component(&recipient_saved.uid);
    let _ = owner_sync.remove_component(&owner_saved.uid);
    let _ = owner_sync
        .client()
        .calendar_destroy(&owner_account_id, &calendar_id);
}
