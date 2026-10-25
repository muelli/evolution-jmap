// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar: an edit of a copy that an editor kept open keeps the edit of a
//! different client, also after a refresh replaced the cached event.
//!
//! The first run gets the event and writes it to a file, as an editor keeps it
//! open. Then a different client changes the description on the server. The
//! second run waits until the refresh replaces the cached event, then saves the
//! old copy with a new summary.

use jmap_client::{Client, Credentials};
use jmap_functional::{Keyfile, Session, SourceKind, required_path};
use jmap_mock::MockServer;
use jmap_proto::calendars::CalendarEvent;
use serde_json::json;

const LOCAL_TITLE: &str = "Team Standup (local)";

#[test]
fn a_stale_editor_keeps_a_remote_edit_of_another_property() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let event_id = {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state.account_mut(&account_id).expect("the default account");
        let calendar_id = account.seed_calendar("Personal", true);
        let id = account.calendar_events.alloc_id();
        let mut event =
            CalendarEvent::simple(calendar_id, "Team Standup", "2026-01-15T13:00:00", "PT30M");
        event.id = Some(id.clone());
        event.uid = Some(format!("urn:example:event:{}", id.as_str()));
        account.calendar_events.seed_with_id(id.clone(), event);
        id
    };
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");
    let mut session = Session::new(format!(
        "{}/calendar-stale-edit",
        env!("CARGO_TARGET_TMPDIR")
    ));
    session.write_source(
        "jmap-functional",
        &Keyfile::new(SourceKind::Calendar, port).to_string(),
    );
    session.stage_calendar_backend(&module);
    let copy = format!("{}/calendar-stale-edit.ics", env!("CARGO_TARGET_TMPDIR"));
    let run = |arguments: &[&str]| {
        let mut all = vec!["jmap-functional"];
        all.extend_from_slice(arguments);
        let output = session.run(&client, &all);
        assert!(
            output.status.success(),
            "the client failed with {}\n--- stdout ---\n{}--- stderr ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&["--save", event_id.as_str(), &copy]);
    Client::connect(server.origin(), Credentials::none())
        .expect("connect to the mock")
        .event_update(
            &account_id,
            &event_id,
            json!({"description": "Remote notes"}),
        )
        .expect("the remote edit");
    run(&["--modify-from", &copy, LOCAL_TITLE]);

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let event = state
        .account(&account_id)
        .expect("the default account")
        .calendar_events
        .get(&event_id)
        .expect("the event")
        .clone();
    assert_eq!(
        event.title.as_deref(),
        Some(LOCAL_TITLE),
        "the edit was not saved"
    );
    assert_eq!(
        event.description.as_deref(),
        Some("Remote notes"),
        "the stale copy reverted the remote edit"
    );
}
