// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar: an offline edit that a real `evolution-calendar-factory` replays
//! keeps the edit of a different client. `book-offline-edit.rs` does the same
//! check for the address book.
//!
//! The test runs the client three times on the on-disk cache of one
//! [`Session`]. The first run lists the calendar, so EDS caches the event and
//! its extra. Then a different client edits the event on the server. The
//! second run sets the summary while the server is out of reach, so EDS keeps
//! the edit in its cache with the old extra. The third run connects again, and
//! EDS replays the edit with `E_CONFLICT_RESOLUTION_KEEP_SERVER`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jmap_client::{Client, Credentials};
use jmap_functional::{Keyfile, Session, SourceKind, required_path, spawn_switchable_proxy};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;
use serde_json::{Value, json};

const TITLE: &str = "Team Standup";
const LOCAL_TITLE: &str = "Team Standup (local)";

/// `calendar-changes.rs`'s keyfile, with the port of the proxy.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::Calendar, port).to_string()
}

/// One mock with one seeded event, and a session on a switchable proxy to it.
struct Setup {
    server: MockServer,
    account_id: Id,
    event_id: Id,
    up: Arc<AtomicBool>,
    session: Session,
    client: std::path::PathBuf,
}

impl Setup {
    fn start(name: &str) -> Self {
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
                CalendarEvent::simple(calendar_id, TITLE, "2026-01-15T13:00:00", "PT30M");
            event.id = Some(id.clone());
            event.uid = Some(format!("urn:example:event:{}", id.as_str()));
            account.calendar_events.seed_with_id(id.clone(), event);
            id
        };
        let authority = server
            .origin()
            .strip_prefix("http://")
            .expect("the mock serves plain http")
            .to_owned();
        let up = Arc::new(AtomicBool::new(true));
        let port = spawn_switchable_proxy(authority, up.clone());
        let mut session = Session::new(format!("{}/{name}", env!("CARGO_TARGET_TMPDIR")));
        session.write_source("jmap-functional", &keyfile(port));
        session.stage_calendar_backend(&module);
        Self {
            server,
            account_id,
            event_id,
            up,
            session,
            client,
        }
    }

    fn run(&self, arguments: &[&str]) {
        let mut all = vec!["jmap-functional"];
        all.extend_from_slice(arguments);
        let output = self.session.run(&self.client, &all);
        assert!(
            output.status.success(),
            "the client failed with {}\n--- stdout ---\n{}--- stderr ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Lists online, applies the remote `patch`, sets the summary offline, and
    /// lists online again. Returns the method calls of the last list.
    fn replay_after(&self, patch: Value) -> Vec<String> {
        self.run(&[]);
        Client::connect(self.server.origin(), Credentials::none())
            .expect("connect to the mock")
            .event_update(&self.account_id, &self.event_id, patch)
            .expect("the remote edit");
        self.up.store(false, Ordering::SeqCst);
        self.run(&[self.event_id.as_str(), LOCAL_TITLE]);
        let before = self.server.method_calls().len();
        self.up.store(true, Ordering::SeqCst);
        self.run(&[]);
        self.server.method_calls().split_off(before)
    }

    fn event(&self) -> CalendarEvent {
        let state = self.server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&self.account_id)
            .expect("the default account");
        account
            .calendar_events
            .get(&self.event_id)
            .expect("the event")
            .clone()
    }
}

#[test]
fn an_offline_edit_keeps_a_remote_edit_of_another_property() {
    let setup = Setup::start("calendar-offline-edit-other");
    setup.replay_after(json!({"description": "Remote notes"}));

    let event = setup.event();
    assert_eq!(
        event.title.as_deref(),
        Some(LOCAL_TITLE),
        "the offline edit was not replayed"
    );
    assert_eq!(
        event.description.as_deref(),
        Some("Remote notes"),
        "the replay reverted the remote edit"
    );
}

#[test]
fn keep_server_on_replay_writes_nothing_for_a_property_both_sides_changed() {
    let setup = Setup::start("calendar-offline-edit-same");
    let calls = setup.replay_after(json!({"title": "Team Standup (remote)"}));

    assert_eq!(
        setup.event().title.as_deref(),
        Some("Team Standup (remote)")
    );
    assert!(
        calls.iter().any(|call| call == "CalendarEvent/get"),
        "the third run never reached the server\n{calls:?}"
    );
    assert!(
        !calls.iter().any(|call| call == "CalendarEvent/set"),
        "the replay wrote to the server\n{calls:?}"
    );
}
