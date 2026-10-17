// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 112, step (c), through a real, running
//! `evolution-calendar-factory`: who tells a meeting's guests
//! (`docs/MEETING-SCHEDULING.md` section 4).
//!
//! `jmap-backend-cal/tests/scheduling.rs` proves the capability string, the
//! operation-flag mapping and the source read on their own, and that the
//! `impl_get_backend_property` slot is installed. Only a real backend proves
//! that a libecal consumer, which is what Evolution is, sees the capabilities
//! that stop it mailing the guests itself, and that the flags it passes reach
//! the server. `tests/functional/cal-scheduling-client.c` reports the three
//! capabilities, then creates and removes one meeting the user sends and one
//! the user chose not to, each with its own guest.
//!
//! Step (d): last, it creates a meeting with a guest the server has no way
//! to reach, and reports how `ECalClient`'s create failed. The mock, like
//! Stalwart, delivers only to `mailto:`, so a `urn:uuid:` guest is refused
//! with `noSupportedScheduleMethods` (draft-ietf-jmap-calendars section
//! 5.9.2). Evolution must see that as a failed save naming the guest, and
//! the meeting must not be stored anywhere for a later sync to send.

use std::collections::BTreeMap;

use jmap_functional::{Session, observations, required_path};
use jmap_mock::MockServer;
use jmap_proto::Id;

const ORGANIZER: &str = "mailto:olga@example.com";
const SENT_GUEST: &str = "mailto:vera@example.com";
/// Must match `UNREACHABLE_GUEST` in `cal-scheduling-client.c`.
const UNREACHABLE_GUEST: &str = "urn:uuid:5f0c9a4e-0000-4000-8000-0000000000d1";

/// The calendar's keyfile, with `extra` appended. No `[Resource] Identity=`:
/// the backend asks for the account's default calendar.
fn keyfile(port: u16, extra: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional test\n\
         Enabled=true\n\
         \n\
         [Calendar]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         \n\
         [Security]\n\
         Method=none\n\
         {extra}"
    )
}

/// What one run saw: the client's observations and the mock's outbox.
struct Run {
    observed: BTreeMap<String, String>,
    sent: Vec<(String, String)>,
    /// Events left in the mock's store once the client is done.
    stored: usize,
    report: String,
}

impl Run {
    fn seen(&self, key: &str) -> Option<&str> {
        self.observed.get(key).map(String::as_str)
    }
}

/// Runs the client against a fresh mock whose account is the organiser,
/// with `extra` keyfile lines.
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
    // The meeting sent with no flags is announced and then cancelled; the
    // one the user chose not to send asks the server for nothing, either
    // way, so Ida hears of neither.
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

#[test]
fn with_calendar_auto_schedule_off_evolution_tells_the_guests_and_the_server_is_never_asked() {
    let run = run("off", "\n[WebDAV Backend]\nCalendarAutoSchedule=false\n");
    for cap in CAPABILITIES {
        assert_eq!(
            run.seen(cap),
            Some("0"),
            "{cap} advertised with the switch off\n{}",
            run.report
        );
    }
    assert!(run.sent.is_empty(), "sent {:?}\n{}", run.sent, run.report);
    assert_eq!(run.seen("removed-sent"), Some("1"), "{}", run.report);
    // Nothing asks the server to reach anyone, so it refuses nothing.
    assert_eq!(run.seen("unreachable-created"), Some("1"), "{}", run.report);
    assert_eq!(run.stored, 0, "{}", run.report);
}

/// Step (d): the server refuses to schedule a guest it cannot reach, and
/// Evolution, which was told the server sends, gets a failed save with the
/// translated sentence naming that guest. Nothing is stored and nothing is
/// sent, so no later sync can file the meeting with the guest never told.
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
