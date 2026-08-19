// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::free_busy` against the mock server: the whole of what
//! `get_free_busy_sync` means, minus the marshalling.
//!
//! Two JMAP calls per attendee — `Principal/query` to turn the address the
//! meeting editor holds into a principal id, then `Principal/getAvailability`
//! for that principal's busy periods — and one `VFREEBUSY` per attendee we got
//! an answer for. The interesting cases are all about the attendees we did
//! *not* get an answer for, because "no answer" and "free" must never come out
//! looking the same.

mod common;

use common::Fixture;
use jmap_cal_sync::{FreeBusy, SyncError};

use jmap_proto::Id;
use jmap_proto::principals::Principal;
use serde_json::json;

const WINDOW_START: &str = "2026-09-01T00:00:00Z";
const WINDOW_END: &str = "2026-09-02T00:00:00Z";

/// Seeds a principal for `email`; `may_get_availability` false models a
/// server that knows the person but will not disclose their calendar.
fn seed_principal(fixture: &Fixture, name: &str, email: &str, may_get_availability: bool) -> Id {
    let state = fixture.server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&fixture.account_id).unwrap();
    let mut principal = Principal {
        principal_type: Some("individual".to_owned()),
        name: name.to_owned(),
        email: Some(email.to_owned()),
        ..Principal::default()
    };
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
    // The window asked about, restated, so an empty answer still says which
    // question it answers.
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

/// The address is what the meeting editor holds, and the three EDS backends
/// that answer this vfunc all treat it as bare. A `mailto:` in front of it is
/// still the same person, and looking up `mailto:bob@example.com` as an email
/// address finds nobody.
#[test]
fn a_mailto_prefixed_attendee_resolves_to_the_same_principal() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");

    let answers = ask(&fixture, &["MAILTO:bob@example.com"]).unwrap();

    assert_eq!(answers.len(), 1);
    // Echoed back as EDS spelled it: the caller pairs the answer with the row
    // it asked about.
    assert_eq!(answers[0].user, "MAILTO:bob@example.com");
    assert!(
        answers[0]
            .icalendar
            .contains("\r\nATTENDEE:mailto:bob@example.com\r\n"),
        "{}",
        answers[0].icalendar,
    );
}

/// A person the server has never heard of is not an error — it is the ordinary
/// case of inviting someone outside the organisation. They get no component,
/// and the scheduler shows their row as unknown.
#[test]
fn an_address_no_principal_matches_is_silently_absent() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "Bob Example", "bob@example.com", true);
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");

    let answers = ask(&fixture, &["stranger@example.net", "bob@example.com"]).unwrap();

    let users: Vec<&str> = answers.iter().map(|answer| answer.user.as_str()).collect();
    assert_eq!(users, vec!["bob@example.com"]);
}

/// `getAvailability` answers `notFound` both for a principal that does not
/// exist and for one the caller may not see (draft §2.2). Neither is a
/// failure of the operation, so it must not sink the attendees that did
/// answer.
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

/// The discrimination the two tests above rest on. `notFound` is the draft's
/// way of saying "not this principal", and is swallowed; every *other* server
/// failure is reported, because a server that could not answer and a server
/// that answered "nobody is busy" must not look the same — the user books a
/// meeting on the strength of the second.
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

/// Nobody to ask about is not a failure, and costs no request: the meeting
/// editor asks this on every keystroke that adds a row.
#[test]
fn no_users_at_all_is_an_empty_answer_and_no_request() {
    let fixture = Fixture::start();
    let before = fixture.server.api_requests();

    assert!(ask(&fixture, &[]).unwrap().is_empty());
    assert_eq!(fixture.server.api_requests(), before);
}

/// An attendee with nothing on gets a component all the same — see the
/// `jmap-ical` suite for why that is not the same as no component.
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

/// Availability is a question about a person, not about one calendar, so it is
/// asked of the account rather than of the calendar this `CalSync` syncs — an
/// event in the account's *other* calendar still makes them busy.
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

/// Two calls per attendee and no more — the meeting editor re-asks this
/// whenever the window is dragged, so a third round trip per row would be felt.
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
