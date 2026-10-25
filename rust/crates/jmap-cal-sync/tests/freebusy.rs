// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `CalSync::free_busy` against the mock server: the full behaviour of
//! `get_free_busy_sync`, without the marshalling.
//!
//! Each attendee costs two JMAP calls. `Principal/query` changes the address from
//! the meeting editor into a principal id, and `Principal/getAvailability` gets the
//! busy periods of that principal. Each attendee with an answer gets one
//! `VFREEBUSY`. The important cases are the attendees *without* an answer, because
//! "no answer" and "free" must never look the same.

mod common;

use std::time::Duration;

use common::Fixture;
use jmap_cal_sync::{CalSync, FreeBusy, SyncError};
use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials};

use jmap_proto::Id;
use jmap_proto::principals::Principal;
use serde_json::{Value, json};

const WINDOW_START: &str = "2026-09-01T00:00:00Z";
const WINDOW_END: &str = "2026-09-02T00:00:00Z";

/// The real transport, but each `Principal/getAvailability` answer becomes a
/// method-level error of `kind`.
struct FailingAvailability {
    inner: UreqTransport,
    kind: &'static str,
}

impl Transport for FailingAvailability {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let mut response = self.inner.execute(request)?;
        if let Ok(mut body) = serde_json::from_slice::<Value>(&response.body) {
            if let Some(invocations) = body
                .get_mut("methodResponses")
                .and_then(Value::as_array_mut)
            {
                for invocation in invocations {
                    if invocation[0] == "Principal/getAvailability" {
                        let call_id = invocation[2].clone();
                        *invocation = json!(["error", { "type": self.kind }, call_id]);
                    }
                }
            }
            response.body = serde_json::to_vec(&body).expect("JSON serialises");
        }
        Ok(response)
    }
}

/// Returns a [`CalSync`] for `fixture.ours`. Its client gets a `kind` error for each
/// `Principal/getAvailability` call.
fn sync_failing_availability(fixture: &Fixture, kind: &'static str) -> CalSync {
    let client = Client::builder()
        .transport(FailingAvailability {
            inner: UreqTransport::new(Duration::from_secs(10)),
            kind,
        })
        .connect(fixture.server.origin(), Credentials::none())
        .unwrap();
    CalSync::new(client, fixture.account_id.clone(), fixture.ours.clone())
}

/// Adds a principal for `email`. When `may_get_availability` is false, the server
/// knows the person but does not disclose the calendar.
fn seed_principal(fixture: &Fixture, name: &str, email: &str, may_get_availability: bool) -> Id {
    let state = fixture.server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&fixture.account_id).unwrap();
    let mut principal = Principal::new(name)
        .with_type("individual")
        .with_email(email);
    if !may_get_availability {
        principal.capabilities.insert(
            "urn:ietf:params:jmap:calendars".to_owned(),
            json!({ "mayGetAvailability": false }),
        );
    }
    account.seed_principal(principal)
}

fn ask(fixture: &Fixture, users: &[&str]) -> Result<Vec<FreeBusy>, SyncError> {
    let users: Vec<String> = users.iter().map(|user| (*user).to_owned()).collect();
    fixture.sync().free_busy(&users, WINDOW_START, WINDOW_END)
}

#[test]
fn an_attendees_events_come_back_as_a_vfreebusy_naming_them() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");

    let answers = ask(&fixture, &["bob@example.com"]).unwrap();

    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].user, "bob@example.com");
    let ics = &answers[0].icalendar;
    assert!(ics.starts_with("BEGIN:VFREEBUSY\r\n"), "{ics}");
    assert!(
        ics.contains("\r\nATTENDEE:mailto:bob@example.com\r\n"),
        "{ics}",
    );
    assert!(
        ics.contains("\r\nFREEBUSY;FBTYPE=BUSY:20260901T090000Z/20260901T100000Z\r\n"),
        "{ics}",
    );
    // The answer states the requested window, so an empty answer still identifies
    // its question.
    assert!(ics.contains("\r\nDTSTART:20260901T000000Z\r\n"), "{ics}");
    assert!(ics.contains("\r\nDTEND:20260902T000000Z\r\n"), "{ics}");
}

#[test]
fn each_attendee_gets_a_component_of_their_own() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    seed_principal(&fixture, "Carol Example", "carol@example.com", true);
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");

    let answers = ask(&fixture, &["bob@example.com", "carol@example.com"]).unwrap();

    let users: Vec<&str> = answers.iter().map(|answer| answer.user.as_str()).collect();
    assert_eq!(users, vec!["bob@example.com", "carol@example.com"]);
}

/// The address comes from the meeting editor, and the three EDS backends that
/// answer this vfunc all treat it as bare. A `mailto:` prefix does not change the
/// person, and a search for `mailto:bob@example.com` as an email address finds nobody.
#[test]
fn a_mailto_prefixed_attendee_resolves_to_the_same_principal() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");

    let answers = ask(&fixture, &["MAILTO:bob@example.com"]).unwrap();

    assert_eq!(answers.len(), 1);
    // The answer repeats the address as EDS wrote it, because the caller uses it
    // to find the row of the answer.
    assert_eq!(answers[0].user, "MAILTO:bob@example.com");
    assert!(
        answers[0]
            .icalendar
            .contains("\r\nATTENDEE:mailto:bob@example.com\r\n"),
        "{}",
        answers[0].icalendar,
    );
}

/// A person that the server does not know is not an error. This is the usual case
/// of an invitation to a person outside the organisation. The person gets no
/// component, and the scheduler shows the row as unknown.
#[test]
fn an_address_no_principal_matches_is_silently_absent() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");

    let answers = ask(&fixture, &["stranger@example.net", "bob@example.com"]).unwrap();

    let users: Vec<&str> = answers.iter().map(|answer| answer.user.as_str()).collect();
    assert_eq!(users, vec!["bob@example.com"]);
}

/// `getAvailability` answers `notFound` for a principal that does not exist and for
/// one that the caller must not see (draft §2.2). Neither is a failure of the
/// operation, so neither must stop the answers for other attendees.
#[test]
fn a_principal_that_will_not_disclose_availability_is_absent_not_an_error() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Private Person", "private@example.com", false);
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");

    let answers = ask(&fixture, &["private@example.com", "bob@example.com"]).unwrap();

    let users: Vec<&str> = answers.iter().map(|answer| answer.user.as_str()).collect();
    assert_eq!(users, vec!["bob@example.com"]);
}

/// The two tests above rely on this distinction. `notFound` is how the draft says
/// "not this principal", and the sync ignores it. The sync reports each *other*
/// server failure: a server that cannot answer and a server that answers "nobody is
/// busy" must look different, because the user books a meeting from the second.
#[test]
fn a_server_failure_that_is_not_not_found_is_reported() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    let sync = jmap_cal_sync::CalSync::new(
        fixture.client(),
        Id::new("no-such-account"),
        fixture.ours.clone(),
    );

    let error = sync
        .free_busy(&["bob@example.com".to_owned()], WINDOW_START, WINDOW_END)
        .expect_err("the account does not exist");

    assert!(matches!(error, SyncError::Client(_)), "{error:?}");
}

/// The test above causes its failure in `Principal/query`, before the
/// `getAvailability` call. This test proves the same rule for the later call. After
/// the sync finds a principal, it must report a failure of `getAvailability` other
/// than `notFound`, and not read it as "nobody to disclose".
#[test]
fn a_getavailability_failure_that_is_not_not_found_is_reported() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);

    let error = sync_failing_availability(&fixture, "serverFail")
        .free_busy(&["bob@example.com".to_owned()], WINDOW_START, WINDOW_END)
        .expect_err("a serverFail is not an answer about the address");

    assert!(matches!(error, SyncError::Client(_)), "{error:?}");
}

/// An empty list of attendees is not a failure, and it costs no request: the
/// meeting editor asks this on each keystroke that adds a row.
#[test]
fn no_users_at_all_is_an_empty_answer_and_no_request() {
    let fixture = Fixture::start();
    let before = fixture.server.api_requests();

    assert!(ask(&fixture, &[]).unwrap().is_empty());
    assert_eq!(fixture.server.api_requests(), before);
}

/// An attendee with no busy periods still gets a component. The `jmap-ical` tests
/// show why this is different from no component.
#[test]
fn a_free_attendee_still_gets_a_component() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);

    let answers = ask(&fixture, &["bob@example.com"]).unwrap();

    assert_eq!(answers.len(), 1);
    assert!(
        !answers[0].icalendar.contains("\r\nFREEBUSY"),
        "{}",
        answers[0].icalendar,
    );
}

/// Availability is about a person, not about one calendar. Thus the sync asks the
/// account, not the calendar of this `CalSync`, and an event in the *other*
/// calendar of the account also makes the person busy.
#[test]
fn availability_spans_the_account_not_just_this_calendar() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    fixture.seed(&fixture.theirs, "Their meeting", "2026-09-01T14:00:00");

    let answers = ask(&fixture, &["bob@example.com"]).unwrap();

    assert!(
        answers[0]
            .icalendar
            .contains("FREEBUSY;FBTYPE=BUSY:20260901T140000Z/20260901T150000Z"),
        "{}",
        answers[0].icalendar,
    );
}

/// Each attendee costs two calls and no more. The meeting editor asks again each
/// time the user drags the window, so a third round trip per row is too slow.
#[test]
fn an_attendee_costs_one_query_and_one_availability_call() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    let before = fixture.server.method_calls().len();

    ask(&fixture, &["bob@example.com"]).unwrap();

    assert_eq!(
        &fixture.server.method_calls()[before..],
        ["Principal/query", "Principal/getAvailability"],
    );
}
