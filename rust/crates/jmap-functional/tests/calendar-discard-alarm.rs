// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A reminder that the user dismisses in Evolution, through a real, running
//! `evolution-calendar-factory`.
//!
//! The Evolution reminder watcher dismisses with
//! `e_cal_client_discard_alarm_sync()` and ignores the result.
//! `ECalMetaBackend` answers that call with `E_CLIENT_ERROR_NOT_SUPPORTED`.
//! Without an override, the dismissal stays on the machine and the reminder
//! occurs again on the other devices of the user. The backend does what the
//! EDS CalDAV backend does: it sets `ACKNOWLEDGED` on the VALARM of the
//! cached component and saves it. `jmap-ical` sends this as the
//! `acknowledged` of the alert (RFC 8984 section 4.5.2).
//!
//! `tests/functional/cal-discard-alarm-client.c` finds the seeded event,
//! gets the alarm uid that libecal reports for it, and dismisses it with the
//! same arguments as the reminder watcher.

use std::collections::BTreeMap;

use jmap_functional::{Session, observations, required_path};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, RecurrenceRule};
use serde_json::{Value, json};

const ONE_OFF: &str = "Dentist";
const SERIES: &str = "Weekly sync";
const START: &str = "2026-11-02T09:00:00";
/// An occurrence of [`SERIES`] that is not the first, in the form that the
/// reminder watcher gives: the recurrence id of the alarm instance.
const LATER_OCCURRENCE: &str = "20261116T090000Z";
const ALERT_KEY: &str = "r1";
const MEETING: &str = "Design review";
const ORGANIZER: &str = "mailto:olga@example.com";
const GUEST: &str = "mailto:gus@example.com";

fn keyfile(port: u16) -> String {
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
         Method=none\n"
    )
}

fn reminder() -> Value {
    json!({
        "@type": "Alert",
        "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
        "action": "display",
    })
}

/// The result of one run: the client observations, and the alerts of the
/// event and the scheduling messages in the mock after the run.
struct Run {
    observed: BTreeMap<String, String>,
    alerts: Option<BTreeMap<String, Value>>,
    sent: usize,
    report: String,
}

impl Run {
    fn seen(&self, key: &str) -> Option<&str> {
        self.observed.get(key).map(String::as_str)
    }
}

/// The shape of the seeded event.
#[derive(Clone, Copy, PartialEq)]
enum Shape {
    OneOff,
    Series,
    /// A one-off event the user organizes, with one guest.
    Meeting,
}

fn meeting_participants() -> BTreeMap<String, Value> {
    [
        (
            "olga".to_owned(),
            json!({"@type": "Participant", "calendarAddress": ORGANIZER,
                   "roles": {"owner": true, "attendee": true}}),
        ),
        (
            "gus".to_owned(),
            json!({"@type": "Participant", "calendarAddress": GUEST,
                   "roles": {"attendee": true}}),
        ),
    ]
    .into()
}

/// Seed one event with one reminder and dismiss it at `rid`.
///
/// With `refuse_scheduling`, the mock refuses every `CalendarEvent/set`
/// that carries `sendSchedulingMessages`.
fn run(name: &str, summary: &str, shape: Shape, refuse_scheduling: bool, rid: Option<&str>) -> Run {
    let client = required_path("JMAP_FUNCTIONAL_CAL_DISCARD_ALARM_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let mut builder = MockServer::builder();
    if refuse_scheduling {
        builder = builder.without_scheduling();
    }
    let server = builder.start();
    let account_id: Id = server.account_id();
    let event_id = {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        let calendar = account.seed_calendar("Personal", true);
        let id = account.calendar_events.alloc_id();
        let mut event = CalendarEvent::simple(calendar, summary, START, "PT30M");
        match shape {
            Shape::OneOff => {}
            Shape::Series => {
                event = event.with_recurrence_rule(RecurrenceRule::new("weekly"));
            }
            Shape::Meeting => {
                account.seed_participant_identity("Olga", ORGANIZER, true);
                event.organizer_calendar_address = Some(ORGANIZER.to_owned());
                event.participants = Some(meeting_participants());
            }
        }
        event.id = Some(id.clone());
        event.uid = Some(format!("urn:example:event:{}", id.as_str()));
        event.alerts = Some([(ALERT_KEY.to_owned(), reminder())].into());
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
        "{}/calendar-discard-alarm-{name}",
        env!("CARGO_TARGET_TMPDIR")
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let mut args = vec!["jmap-functional", summary];
    args.extend(rid);
    let output = session.run(&client, &args);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );
    let observed = observations(&stdout)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect::<BTreeMap<_, _>>();

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let account = state
        .account(&account_id)
        .expect("the mock's default account");
    let alerts = account
        .calendar_events
        .get(&event_id)
        .expect("the seeded event is still on the server")
        .alerts
        .clone();
    Run {
        observed,
        alerts,
        sent: account.scheduling_outbox.len(),
        report,
    }
}

/// The dismissal succeeded, and EDS and the server both hold it on the same
/// alert. No other part of the reminder changed.
fn assert_dismissed(run: &Run) {
    assert_eq!(run.seen("alarms"), Some("1"), "{}", run.report);
    assert_eq!(run.seen("discarded"), Some("1"), "{}", run.report);
    let local = run.seen("read-back-acknowledged").unwrap_or_default();
    assert!(
        local.ends_with('Z'),
        "EDS holds no UTC ACKNOWLEDGED: {local:?}\n{}",
        run.report
    );

    let alerts = run.alerts.as_ref().expect("the event keeps its alerts");
    assert_eq!(
        alerts.keys().collect::<Vec<_>>(),
        [ALERT_KEY],
        "the reminder was re-keyed or duplicated: {alerts:?}"
    );
    let mut alert = alerts[ALERT_KEY].clone();
    let acknowledged = alert
        .as_object_mut()
        .and_then(|alert| alert.remove("acknowledged"));
    let acknowledged = acknowledged
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the server's alert has no acknowledged: {alerts:?}"));
    assert!(
        acknowledged.ends_with('Z'),
        "not a UTCDateTime: {acknowledged:?}"
    );
    assert_eq!(alert, reminder(), "the rest of the reminder changed");
    assert_eq!(run.sent, 0, "a dismissal told the guests\n{}", run.report);
}

#[test]
fn dismissing_a_reminder_reaches_the_server() {
    let run = run("one-off", ONE_OFF, Shape::OneOff, false, None);
    assert_dismissed(&run);
}

/// The reminder watcher names the occurrence that fired. A series without a
/// detached occurrence holds its reminder on the master. Thus the dismissal
/// goes to the master, as in the EDS CalDAV backend.
#[test]
fn dismissing_an_occurrences_reminder_acknowledges_the_series() {
    let run = run(
        "series",
        SERIES,
        Shape::Series,
        false,
        Some(LATER_OCCURRENCE),
    );
    assert_dismissed(&run);
}

/// A dismissal is private to the user. The reminder watcher gives no
/// operation flags, so the backend must not ask the server to send
/// scheduling messages (draft-ietf-jmap-calendars section 5.9.2). The mock
/// refuses each `/set` that asks for them, so such a request fails.
#[test]
fn dismissing_a_meetings_reminder_never_asks_the_server_to_mail_the_guests() {
    let run = run("meeting", MEETING, Shape::Meeting, true, None);
    assert_dismissed(&run);
}
