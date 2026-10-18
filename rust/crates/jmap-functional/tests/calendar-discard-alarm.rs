// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A reminder dismissed in Evolution, through a real, running
//! `evolution-calendar-factory`.
//!
//! Evolution's reminder watcher dismisses with
//! `e_cal_client_discard_alarm_sync()` and ignores the outcome.
//! `ECalMetaBackend` answers that call with `E_CLIENT_ERROR_NOT_SUPPORTED`,
//! so without an override the dismissal never leaves the machine and the
//! reminder fires again on the user's other devices. The backend now does
//! what EDS's CalDAV backend does: it sets the VALARM's `ACKNOWLEDGED` on the
//! cached component and saves it, which `jmap-ical` sends as the alert's
//! `acknowledged` (RFC 8984 section 4.5.2).
//!
//! `tests/functional/cal-discard-alarm-client.c` finds the seeded event,
//! takes the alarm uid libecal reports for it and dismisses it with the
//! same arguments the reminder watcher passes.

use std::collections::BTreeMap;

use jmap_functional::{Session, observations, required_path};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, RecurrenceRule};
use serde_json::{Value, json};

const ONE_OFF: &str = "Dentist";
const SERIES: &str = "Weekly sync";
const START: &str = "2026-11-02T09:00:00";
/// An occurrence of [`SERIES`] that is not its first, in the form the
/// reminder watcher passes: the alarm instance's own recurrence id.
const LATER_OCCURRENCE: &str = "20261116T090000Z";
const ALERT_KEY: &str = "r1";

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

/// What one run saw: the client's observations, and the event's alerts and
/// the scheduling messages as the mock holds them afterwards.
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

/// Seeds one event with one reminder, `recurring` or not, and dismisses it
/// at `rid`.
fn run(name: &str, summary: &str, recurring: bool, rid: Option<&str>) -> Run {
    let client = required_path("JMAP_FUNCTIONAL_CAL_DISCARD_ALARM_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = MockServer::builder().start();
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
        if recurring {
            event = event.with_recurrence_rule(RecurrenceRule::new("weekly"));
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

/// The dismissal succeeded, EDS holds it, and the server holds it on the
/// same alert, with nothing else about the reminder changed.
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
    let run = run("one-off", ONE_OFF, false, None);
    assert_dismissed(&run);
}

/// The reminder watcher names the occurrence that fired. A series with no
/// detached occurrence holds its reminder on the master, so that is where
/// the dismissal goes, as EDS's CalDAV backend does.
#[test]
fn dismissing_an_occurrences_reminder_acknowledges_the_series() {
    let run = run("series", SERIES, true, Some(LATER_OCCURRENCE));
    assert_dismissed(&run);
}
