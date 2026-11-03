// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 176(a): the calendar equivalent of `book-restart-offline.rs`.
//! `jmap-backend-cal` is a plain `ECalMetaBackend` subclass, and EDS's own
//! `ecmb_open_sync` (`e-cal-meta-backend.c`) skips `ensure_connected_sync`
//! entirely once `e_cal_meta_backend_get_ever_connected` is true, the same
//! on-disk flag `e-book-meta-backend.c` keeps for address books. This test
//! confirms that holds for real.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_proto::calendars::CalendarEvent;

const TITLE: &str = "Team Standup";

/// `calendar-changes.rs`'s keyfile, with the port of the mock.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::Calendar, port).to_string()
}

#[test]
fn a_restarted_calendar_with_no_network_still_lists_a_previously_cached_event() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state.account_mut(&account_id).expect("the default account");
        let calendar_id = account.seed_calendar("Personal", true);
        let id = account.calendar_events.alloc_id();
        let mut event = CalendarEvent::simple(calendar_id, TITLE, "2026-01-15T13:00:00", "PT30M");
        event.id = Some(id.clone());
        event.uid = Some(format!("urn:example:event:{}", id.as_str()));
        account.calendar_events.seed_with_id(id, event);
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/calendar-restart-offline"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let prime = session.run(&client, &["jmap-functional"]);
    let prime_stdout = String::from_utf8_lossy(&prime.stdout);
    let prime_stderr = String::from_utf8_lossy(&prime.stderr);
    let prime_report =
        format!("--- client stdout ---\n{prime_stdout}--- client stderr ---\n{prime_stderr}");
    assert!(
        prime.status.success(),
        "the priming run failed with {}\n{prime_report}",
        prime.status
    );
    let prime_seen = observations(&prime_stdout);
    assert_eq!(
        prime_seen.get("events"),
        Some(&"1"),
        "the priming run did not see the seeded event\n{prime_report}"
    );

    // Nothing listens on the mock's port from here on. Dropping it joins its
    // serve thread, which only returns once the listening socket is closed,
    // so the port is free before the restarted run below ever starts.
    drop(server);

    let restart = session.run(&client, &["jmap-functional"]);
    let restart_stdout = String::from_utf8_lossy(&restart.stdout);
    let restart_stderr = String::from_utf8_lossy(&restart.stderr);
    let restart_report =
        format!("--- client stdout ---\n{restart_stdout}--- client stderr ---\n{restart_stderr}");
    assert!(
        restart.status.success(),
        "the restarted, offline run failed with {}\n{restart_report}",
        restart.status
    );
    let restart_seen = observations(&restart_stdout);
    assert_eq!(
        restart_seen.get("events"),
        Some(&"1"),
        "the restarted, offline run did not see the previously cached event\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("event-0"),
        Some(&TITLE),
        "the restarted, offline run's event does not match the seeded one\n{restart_report}"
    );
}
