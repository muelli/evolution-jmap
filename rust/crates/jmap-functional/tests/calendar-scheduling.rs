// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Who sends mail to the guests of a meeting (`docs/MEETING-SCHEDULING.md`
//! section 4), through a real, running `evolution-calendar-factory`.
//!
//! `jmap-backend-cal/tests/scheduling.rs` tests the capability string, the
//! operation-flag mapping separately, and that the
//! `impl_get_backend_property` slot is installed. Only a real backend shows
//! that a libecal consumer (such as Evolution) sees the capabilities that
//! stop it from sending mail to the guests, and that its flags reach the
//! server. `tests/functional/cal-scheduling-client.c` reports the three
//! capabilities. Then it creates and removes one meeting that the user sends
//! and one that the user does not send, each with a different guest.
//!
//! Last, it creates a meeting with a guest that the server cannot reach, and
//! reports how the create of `ECalClient` failed. The mock, as Stalwart,
//! delivers only to `mailto:`, so it refuses a `urn:uuid:` guest with
//! `noSupportedScheduleMethods` (draft-ietf-jmap-calendars section 5.9.2).
//! Evolution must see a failed save that names the guest. The meeting must
//! not be stored, so a later sync cannot send it.

use std::collections::BTreeMap;

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_mock::MockServer;
use jmap_proto::Id;

const ORGANIZER: &str = "mailto:olga@example.com";
const SENT_GUEST: &str = "mailto:vera@example.com";
/// Must match `UNREACHABLE_GUEST` in `cal-scheduling-client.c`.
const UNREACHABLE_GUEST: &str = "urn:uuid:5f0c9a4e-0000-4000-8000-0000000000d1";

/// The calendar keyfile, with `extra` at the end. No `[Resource] Identity=`:
/// the backend asks for the default calendar of the account.
fn keyfile(port: u16, extra: &str) -> String {
    Keyfile::new(SourceKind::Calendar, port)
        .extra(extra)
        .to_string()
}

/// The result of one run: the client observations and the mock outbox.
struct Run {
    observed: BTreeMap<String, String>,
    sent: Vec<(String, String)>,
    /// The number of events in the mock store after the client exits.
    stored: usize,
    report: String,
}

impl Run {
    fn seen(&self, key: &str) -> Option<&str> {
        self.observed.get(key).map(String::as_str)
    }
}

/// Run the client against a new mock whose account is the organiser, with
/// the `extra` keyfile lines.
fn run(name: &str, extra: &str) -> Run {
    let client = required_path("JMAP_FUNCTIONAL_CAL_SCHEDULING_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = MockServer::builder().start();
    let account_id: Id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.seed_participant_identity("Olga", ORGANIZER, true);
        account.seed_calendar("Work", true);
    }
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(format!(
        "{}/calendar-scheduling-{name}",
        env!("CARGO_TARGET_TMPDIR")
    ));
    session.write_source("jmap-functional", &keyfile(port, extra));
    session.stage_calendar_backend(&module);

    let output = session.run(&client, &["jmap-functional"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let observed = observations(&stdout)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(
        observed.get("connection-status").map(String::as_str),
        Some("connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let account = state
        .account(&account_id)
        .expect("the mock's default account");
    let sent = account
        .scheduling_outbox
        .iter()
        .map(|message| (message.method.clone(), message.recipient.clone()))
        .collect();
    Run {
        observed,
        sent,
        stored: account.calendar_events.len(),
        report,
    }
}

const CAPABILITIES: [&str; 3] = [
    "create-messages",
    "save-schedules",
    "itip-suppress-on-remove-supported",
];

#[test]
fn by_default_the_server_tells_the_guests_and_evolution_is_told_not_to() {
    let run = run("default", "");
    for cap in CAPABILITIES {
        assert_eq!(
            run.seen(cap),
            Some("1"),
            "{cap} not advertised\n{}",
            run.report
        );
    }
    // The meeting sent without flags gets a REQUEST and then a CANCEL. The
    // meeting that the user does not send asks the server for nothing, so
    // its guest (Ida) gets no message.
    assert_eq!(
        run.sent,
        [
            ("REQUEST".to_owned(), SENT_GUEST.to_owned()),
            ("CANCEL".to_owned(), SENT_GUEST.to_owned()),
        ],
        "{}",
        run.report
    );
}

/// An old config can still carry `CalendarAutoSchedule=false`. The backend
/// ignores it: the server always tells the guests.
#[test]
fn an_old_calendar_auto_schedule_off_still_has_the_server_tell_the_guests() {
    let run = run("off", "\n[WebDAV Backend]\nCalendarAutoSchedule=false\n");
    for cap in CAPABILITIES {
        assert_eq!(
            run.seen(cap),
            Some("1"),
            "{cap} not advertised with the old switch off\n{}",
            run.report
        );
    }
    assert_eq!(
        run.sent,
        [
            ("REQUEST".to_owned(), SENT_GUEST.to_owned()),
            ("CANCEL".to_owned(), SENT_GUEST.to_owned()),
        ],
        "{}",
        run.report
    );
    assert_eq!(run.seen("unreachable-created"), Some("0"), "{}", run.report);
    assert_eq!(run.stored, 0, "{}", run.report);
}

/// The server refuses to schedule a guest that it cannot reach. Evolution,
/// which expects the server to send, gets a failed save with the translated
/// message that names that guest. Nothing is stored or sent, so a later sync
/// cannot save the meeting without a message to the guest.
#[test]
fn a_guest_the_server_cannot_reach_fails_the_save_in_evolution_and_names_them() {
    let run = run("unreachable", "");
    assert_eq!(run.seen("unreachable-created"), Some("0"), "{}", run.report);
    assert_eq!(
        run.seen("unreachable-invalid-arg"),
        Some("1"),
        "not E_CLIENT_ERROR_INVALID_ARG\n{}",
        run.report
    );
    let message = run.seen("unreachable-message").unwrap_or_default();
    assert!(
        message.contains("so the change was not saved") && message.contains(UNREACHABLE_GUEST),
        "the message does not say the save failed for {UNREACHABLE_GUEST}: {message:?}\n{}",
        run.report
    );
    assert!(
        !message.contains(SENT_GUEST),
        "the message names a guest the server can reach: {message:?}"
    );
    assert_eq!(
        run.stored, 0,
        "the refused meeting was stored\n{}",
        run.report
    );
    assert!(
        run.sent.iter().all(|(_, to)| to != UNREACHABLE_GUEST),
        "sent {:?}\n{}",
        run.sent,
        run.report
    );
}
