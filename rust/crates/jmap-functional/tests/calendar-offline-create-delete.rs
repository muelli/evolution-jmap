// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar: an event created or deleted while offline reaches the server
//! once a real `evolution-calendar-factory` reconnects.
//! `calendar-offline-edit.rs` does the same check for an edit of an existing
//! event; this file covers the two remaining write shapes ROADMAP item
//! 176(a) asks about. `book-offline-create-delete.rs` does the same for the
//! address book.
//!
//! Both tests run the client three times on the on-disk cache of one
//! [`Session`]: once online (so EDS caches the calendar and, for the delete
//! test, the seeded event), once with the proxy down (the write that
//! `ECalMetaBackend` queues as an offline change instead of sending), and
//! once online again, which is when EDS uploads the queued change.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jmap_functional::{
    Keyfile, Session, SourceKind, observations, required_path, spawn_switchable_proxy,
};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;

const SEEDED_TITLE: &str = "Team Standup";
const NEW_TITLE: &str = "Offsite Planning";

fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::Calendar, port).to_string()
}

/// One mock with one seeded event, a session on a switchable proxy to it, and
/// the paths of the client.
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
                CalendarEvent::simple(calendar_id, SEEDED_TITLE, "2026-01-15T13:00:00", "PT30M");
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

    fn run(&self, arguments: &[&str]) -> String {
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
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn titles_on_server(&self) -> Vec<String> {
        let state = self.server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&self.account_id)
            .expect("the default account");
        account
            .calendar_events
            .iter()
            .map(|(_, event)| event.title.clone().unwrap_or_default())
            .collect()
    }

    fn has_event_on_server(&self, id: &Id) -> bool {
        let state = self.server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&self.account_id)
            .expect("the default account");
        account.calendar_events.contains(id)
    }
}

#[test]
fn an_event_created_offline_reaches_the_server_on_reconnect() {
    let setup = Setup::start("calendar-offline-create");

    // Online: caches the calendar and sets `ever_connected`, the condition
    // `ECalMetaBackend` needs before it will open with no connection at all.
    setup.run(&[]);

    setup.up.store(false, Ordering::SeqCst);
    let stdout = setup.run(&["--create", NEW_TITLE]);
    let seen = observations(&stdout);
    let added = seen
        .get("added")
        .filter(|uid| !uid.is_empty())
        .unwrap_or_else(|| panic!("the offline create reported no UID\n{stdout}"));
    assert!(
        !setup.titles_on_server().contains(&NEW_TITLE.to_owned()),
        "the create reached the server while the proxy was down"
    );

    setup.up.store(true, Ordering::SeqCst);
    // Reconnecting uploads the queued create; a plain list run is enough to
    // trigger it, the same way `calendar-offline-edit.rs`'s third run
    // triggers the replay of a queued edit.
    setup.run(&[]);

    assert!(
        setup.titles_on_server().contains(&NEW_TITLE.to_owned()),
        "the offline create never reached the server after reconnecting, EDS-assigned uid {added}"
    );
}

#[test]
fn an_event_deleted_offline_reaches_the_server_on_reconnect() {
    let setup = Setup::start("calendar-offline-delete");

    setup.run(&[]);

    setup.up.store(false, Ordering::SeqCst);
    let stdout = setup.run(&["--remove", setup.event_id.as_str()]);
    let seen = observations(&stdout);
    assert_eq!(seen.get("removed"), Some(&"1"));
    assert!(
        setup.has_event_on_server(&setup.event_id),
        "the delete reached the server while the proxy was down"
    );

    setup.up.store(true, Ordering::SeqCst);
    setup.run(&[]);

    assert!(
        !setup.has_event_on_server(&setup.event_id),
        "the offline delete never reached the server after reconnecting"
    );
}
