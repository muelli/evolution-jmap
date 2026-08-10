// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, calendar: `evolution-calendar-factory` loading
//! `libecalbackendjmap.so`, opening a calendar from a `.source` keyfile, and
//! serving a write through it to the mock JMAP server.
//!
//! The twin of `address-book.rs`, and deliberately so: the two backends are
//! mirrors of each other, which is exactly why one of them can carry a bug
//! the other's tests would have caught. Everything here is checked from the
//! two ends and nothing in between — the client program says what EDS gave a
//! libecal consumer, the mock says what the backend asked the server for.

use jmap_functional::{Session, observations, required_path};
use jmap_proto::calendars::NDay;

/// The event the client writes. The summary is passed on its command line
/// and looked for in the mock's store, so the two ends cannot disagree about
/// it by a typo; the start is the JSCalendar spelling of the `DTSTART` in
/// `tests/functional/cal-client.c`.
const SUMMARY: &str = "Sprint planning";
const START: &str = "2026-01-15T13:00:00";

/// And where it happens — the `LOCATION` in `tests/functional/cal-client.c`,
/// which has to reach the server as an entry in a JSCalendar `locations` map
/// (RFC 8984 §4.2.5) rather than as nothing at all. The key is the one
/// `jmap-ical` invents for a component that carries none, since EDS has never
/// seen this event before.
const LOCATION: &str = "Room 42";

/// And what it is filed under — the `CATEGORIES` in
/// `tests/functional/cal-client.c`, which has to reach the server as a
/// JSCalendar `keywords` Set (RFC 8984 §4.2.9). Two tags on one line, because
/// libical re-renders a multi-valued `CATEGORIES` as one property per value: a
/// mapping that read only the first would send a set of one, and the save after
/// it would delete the rest. Sorted, since that is the order a set is held in on
/// both sides.
const KEYWORDS: [&str; 2] = ["offsite", "planning"];

/// And whether it blocks the time it occupies — the `TRANSP` in
/// `tests/functional/cal-client.c`, which has to reach the server as a
/// JSCalendar `freeBusyStatus` (RFC 8984 §4.4.2). The transparent state
/// deliberately: both formats default to the other one, so only this direction
/// distinguishes a state that crossed from a component that lost the line.
const TRANSP: &str = "TRANSPARENT";
const FREE_BUSY_STATUS: &str = "free";

/// And how important it is — the `PRIORITY` in `tests/functional/cal-client.c`,
/// which has to reach the server as a JSCalendar `priority` (RFC 8984 §4.4.1).
/// The one mapped property that is a number rather than text on both sides, so
/// this is the leg that says a numeric property survives EDS's cache; 1 rather
/// than the 0 both formats treat as no value at all.
const PRIORITY: &str = "1";

/// The length of that event, which the client states as a `DTEND` — the way
/// Evolution's editor does — an hour and a half after the start. Nothing but
/// this test says the two forms end up alike on the server.
const DURATION: &str = "PT1H30M";

/// The second event the client writes: an all-day one, `VALUE=DATE` on both
/// ends, which is the only way iCalendar says "a day rather than a time of
/// day". On the server it has to arrive as JSCalendar's `showWithoutTime`,
/// starting at the top of the day and lasting one — otherwise every other
/// client reading the account sees a midnight appointment.
const ALL_DAY_SUMMARY: &str = "Team offsite";
const ALL_DAY_START: &str = "2026-02-01T00:00:00";
const ALL_DAY_DURATION: &str = "P1D";

/// The third event: one in a named zone, built by the client through the
/// libical setters the way Evolution's editor builds it — so the `TZID` that
/// reaches the backend is libical's own
/// `/freeassociation.sourceforge.net/Europe/Berlin`, which is not an RFC 8984
/// §1.4.9 `TimeZoneId` and which nothing outside libical resolves.
///
/// This is the leg no test below real EDS can stand in for. The mapping can
/// translate that identifier only from the `VTIMEZONE` beside it, and whether
/// one travels with the component is `marshal::icalendar_from_instances`'s
/// business — so the mapping's own tests, which supply the identifier and the
/// definition by hand, cannot say whether a zone the user picked in Evolution
/// ever reaches the server. A `time_zone` of `None` here is exactly the bug
/// that had shipped: the appointment on the server floats, and every other
/// client shows it at the wrong hour.
///
/// The start is the wall-clock time in that zone, which is what JSCalendar's
/// `start` means beside a `timeZone` (RFC 8984 §4.1.1) — not the UTC instant.
const ZONED_SUMMARY: &str = "Berlin review";
const ZONED_START: &str = "2026-01-15T16:00:00";
const ZONED_TIME_ZONE: &str = "Europe/Berlin";

/// The fourth event: a weekly one with a single occurrence deleted, which EDS
/// hands to the backend as an `EXDATE` on the master component. On the server
/// that has to be an entry in `recurrenceOverrides` saying the instance is
/// `excluded` — the only thing JSCalendar has for it. An `EXDATE` the mapping
/// drops is an appointment the user cancelled and everybody else still sees.
const RECURRING_SUMMARY: &str = "Weekly standup";
const RECURRING_EXCLUDED: &str = "2026-01-29T13:00:00";

/// And the other half of "not that one": an occurrence the user renamed
/// rather than deleted, which is what "Edit this occurrence" does. EDS hands
/// the backend a second `VEVENT` with the same `UID` and a `RECURRENCE-ID`
/// naming the instance it replaces (RFC 5545 §3.8.4.4); JSCalendar says the
/// same thing with a patch under that instant in `recurrenceOverrides`
/// (RFC 8984 §4.3.4). Nothing below this file says the two ends agree about
/// it through real EDS — the mapping's own tests stop at the component.
const RECURRING_EDITED: &str = "2026-01-22T13:00:00";
const RECURRING_EDITED_SUMMARY: &str = "Weekly standup (demo)";

/// And "not that one" a second time, reached the way a user reaches it. The
/// `EXDATE` above is written into the component the client creates, so it holds
/// the *mapping* to account and says nothing about EDS: Evolution's "Delete
/// this occurrence" calls `e_cal_client_remove_object_sync` with a
/// `RECURRENCE-ID` and `E_CAL_OBJ_MOD_THIS`, and what `ECalMetaBackend` makes
/// of that is a **save of the master** — not a removal — which is a code path
/// nothing in this tree had ever asked EDS to take. The fourth occurrence of
/// the weekly series, so that it is neither the excluded one nor the edited
/// one and a mix-up cannot pass.
const RECURRING_REMOVED: &str = "2026-02-05T13:00:00";

/// And the third thing that menu offers, "Edit this and future occurrences",
/// which is not an exception to the series at all: `ECalMetaBackend` answers it
/// by **splitting the series in two** — the master's rule is truncated to stop
/// before the named instance, and that instance onwards becomes a *second
/// event* under a UID EDS invents, handed to the backend as a create. So it is
/// the only one of the three that reaches the backend as two writes, and the
/// only one where the mapping's job is an ordinary event rather than an
/// override. `RANGE=THISANDFUTURE` never appears — EDS has resolved it into
/// plain components before the backend sees anything, which is what makes
/// `jmap-ical` skipping that parameter on read the harmless choice it was
/// assumed to be.
///
/// The fifth occurrence, after all three exceptions the series carries by then.
const RECURRING_SPLIT: &str = "2026-02-12T13:00:00";
const RECURRING_SPLIT_SUMMARY: &str = "Weekly standup (new plan)";

/// What the split leaves behind, as EDS spells the two rules back in its own
/// cache. `COUNT=6` becomes four occurrences before the split and two from it
/// on: a truncated rule the backend's save undid would leave the old series
/// still recurring over the days the new event now owns — the same appointment
/// twice, under two titles — and a new series that kept `COUNT=6` would run six
/// weeks past where the user cut it.
///
/// Both keep the `BYDAY` the series was created with — the day of the week is
/// not something a split changes — which is the client-side half of the
/// question the `byDay` assertions below ask of the server.
const SERIES_RRULE: &str = "FREQ=WEEKLY;COUNT=4;BYDAY=TH";
const SPLIT_RRULE: &str = "FREQ=WEEKLY;COUNT=2;BYDAY=TH";
const SPLIT_DTSTART: &str = "20260212T130000Z";

/// The same two exclusions as EDS spells them back in its own cache: the one
/// the client wrote with the event and the one it removed afterwards, both in
/// the series' UTC. Named rather than counted, because two exclusions of which
/// one names the wrong day is one cancelled appointment that comes back and
/// another that was never cancelled at all.
const RECURRING_EXDATES: [&str; 2] = ["20260129T130000Z", "20260205T130000Z"];

/// And the sixth event, which is the one question every case above leaves
/// open: a series in one named zone with a single occurrence moved into
/// another. RFC 5545 §3.2.19 puts a zone on the *property*, so a detached
/// instance states its own `TZID` and need not share the series'; RFC 8984
/// §4.4.3 says the same thing by letting a `recurrenceOverrides` patch carry
/// `timeZone`. The mapping learned both last, and its own tests supply the
/// identifiers by hand — so nothing yet says that a second zone, named by one
/// instance of a component Evolution actually hands over, is defined in the
/// envelope the backend builds and translated on the way out.
///
/// The move is five hours and a different clock, not a nudge: an override that
/// arrived as a bare `start` — the bug that had shipped — puts the occurrence
/// at 08:00 *Berlin* instead of 08:00 New York, and every other client reading
/// the account shows it there.
const ZONED_RECURRING_SUMMARY: &str = "Berlin standup";
const ZONED_RECURRING_START: &str = "2026-03-05T10:00:00";
const ZONED_RECURRING_DURATION: &str = "PT1H";

/// The occurrence that moved: keyed on its start as the *rules* generate it,
/// which is the series' clock, and carrying the start and the zone it was moved
/// to. Both halves of the patch are asserted together, because either alone
/// passes for the other going wrong.
const ZONED_MOVED_INSTANCE: &str = "2026-03-12T10:00:00";
const ZONED_MOVED_START: &str = "2026-03-12T08:00:00";
const ZONED_MOVED_TIME_ZONE: &str = "America/New_York";

/// And what EDS itself kept of that instance, which is the other end of the
/// same claim: a `DTSTART` still on the moved clock. The value is exact; the
/// `TZID` is only required to *name* the zone, because how libical spells an
/// identifier for a builtin zone is libical's business and has changed between
/// releases — `/freeassociation.sourceforge.net/America/New_York` and a plain
/// `America/New_York` both end the same way, and a series' zone silently
/// applied to the instance ends in `Europe/Berlin`.
const ZONED_MOVED_DTSTART: &str = "20260312T080000";

/// The keyfile from `docs/examples/jmap-mock-calendar.source`, with the
/// mock's ephemeral port filled in. Kept as a literal here rather than read
/// from `docs/` so that a change to the documented recipe fails this test
/// loudly instead of quietly retargeting it.
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

#[test]
fn evolution_opens_the_calendar_and_a_write_reaches_the_server() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // No `[Resource] Identity=` in the keyfile above, so the backend asks the
    // server for the account's default calendar. Seeding one flagged default
    // is what makes that question answerable.
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        state
            .account_mut(&account_id)
            .expect("the mock's default account")
            .seed_calendar("Personal", true);
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/calendar"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let output = session.run(
        &client,
        &[
            "jmap-functional",
            SUMMARY,
            ALL_DAY_SUMMARY,
            RECURRING_SUMMARY,
            RECURRING_EDITED_SUMMARY,
            RECURRING_SPLIT_SUMMARY,
            ZONED_SUMMARY,
            ZONED_RECURRING_SUMMARY,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Checked before the exit status, for the reason `address-book.rs` gives:
    // a read-only calendar turns every later failure into "Permission
    // denied", a message about the write that is really about the connect.
    //
    // It is deliberately a broad net. `e_cal_client_connect_sync` succeeds
    // even when the backend's `connect_sync` failed — `ECalMetaBackend` opens
    // the calendar and schedules the connect — so a calendar the backend
    // could not open reaches the client looking exactly like one it opened
    // and forgot to claim writable. Both are this assertion's business.
    //
    // Unless the client never got this far, in which case the failure is
    // earlier than anything here — the module missing from the factory's
    // directory, say — and the exit status is what says so.
    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the calendar, with {}\n{report}",
            output.status
        )
    });
    // Asserted before `readonly` even though the client prints them in this
    // order anyway, because this one is the cause and that one is a symptom
    // of it: the source's connection status is set to connected by
    // `e_cal_meta_backend_ensure_connected_sync` only when the backend's
    // `connect_sync` returned TRUE, so a calendar the backend could not open
    // — the case `readonly` cannot distinguish — fails here first, saying
    // which of the two happened.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );

    assert_eq!(readonly, "0", "EDS opened the calendar read-only\n{report}");

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("events-before"),
        Some(&"0"),
        "a fresh cache against an empty calendar should hold nothing\n{report}"
    );

    for key in [
        "added",
        "added-all-day",
        "added-zoned",
        "added-recurring",
        "added-zoned-recurring",
    ] {
        let added = seen
            .get(key)
            .unwrap_or_else(|| panic!("the client reported no {key} event\n{report}"));
        assert!(
            !added.is_empty(),
            "EDS added an event with no UID ({key})\n{report}"
        );
    }

    // Read back through EDS: what the meta backend kept of the write.
    assert_eq!(
        seen.get("read-back-summary"),
        Some(&SUMMARY),
        "the event EDS handed back is not the one that went in\n{report}"
    );
    // And the place it happens at, which crosses the mapping as a whole property
    // rather than as a line of text: `locations` is a map of objects, so a
    // LOCATION that does not come back here is one the round trip lost.
    assert_eq!(
        seen.get("read-back-location"),
        Some(&LOCATION),
        "the event EDS handed back happens nowhere\n{report}"
    );
    // And the tags, which cross the mapping as a set rather than as a line: the
    // client joins every CATEGORIES value it finds, so a set that lost a member
    // between the write and EDS's cache shows up here as a shorter list.
    assert_eq!(
        seen.get("read-back-categories")
            .map(|values| values.split(',').collect::<Vec<_>>()),
        Some(KEYWORDS.to_vec()),
        "the event EDS handed back lost a tag\n{report}"
    );
    // And whether it blocks time. An empty string here is a component EDS handed
    // back with no TRANSP on it, which reads as the OPAQUE both formats default
    // to — so the state the client asked for would be gone and the next save
    // would write the default over it.
    assert_eq!(
        seen.get("read-back-transp"),
        Some(&TRANSP),
        "the event EDS handed back blocks time after all\n{report}"
    );
    // And how important it is. An empty string here is a component EDS handed back
    // with no PRIORITY on it, which reads as the undefined importance both formats
    // default to — so the number the client asked for would be gone.
    assert_eq!(
        seen.get("read-back-priority"),
        Some(&PRIORITY),
        "the event EDS handed back lost its priority\n{report}"
    );
    // What EDS made of the edit, read back through the client rather than off
    // the server: `ECalMetaBackend` holds a series and its detached instances
    // as one object, so a component set that lost the override here would have
    // the *next* save undo it, whatever the server holds at this moment.
    assert_eq!(
        seen.get("edited-occurrence-summary"),
        Some(&RECURRING_EDITED_SUMMARY),
        "EDS did not keep the occurrence the client edited\n{report}"
    );
    // And what EDS made of the removal, in the same cache and for the same
    // reason: the master it kept has to carry an `EXDATE` for every occurrence
    // that no longer happens — the one the client wrote into the event and the
    // one it removed through EDS afterwards. Sorted before comparing, because
    // the order libical hands two exclusions back is not what this is about.
    let exdates = seen
        .get("recurring-exdates")
        .unwrap_or_else(|| panic!("the client reported no exclusions\n{report}"));
    let mut exdates: Vec<&str> = exdates
        .split(',')
        .filter(|value| !value.is_empty())
        .collect();
    exdates.sort_unstable();
    assert_eq!(
        exdates, RECURRING_EXDATES,
        "EDS's cache does not hold exactly the two occurrences that were \
         cancelled\n{report}"
    );

    // And what EDS made of the split, in the same cache and for the same reason.
    // Both halves are asserted, because either one on its own passes for a
    // split that went wrong in the other: a truncated master with no new event
    // is a fortnight of the series the user renamed and lost, and a new event
    // beside an untruncated master is every one of those days twice.
    assert_eq!(
        seen.get("series-rrule"),
        Some(&SERIES_RRULE),
        "EDS's cache does not hold the series truncated at the split\n{report}"
    );
    assert_eq!(
        seen.get("split-dtstart"),
        Some(&SPLIT_DTSTART),
        "the series EDS split off does not start at the occurrence the split \
         was asked for\n{report}"
    );
    assert_eq!(
        seen.get("split-rrule"),
        Some(&SPLIT_RRULE),
        "the series EDS split off does not recur over what is left of the \
         original\n{report}"
    );
    assert_eq!(
        seen.get("split-exdates"),
        Some(&""),
        "the series EDS split off carries exclusions belonging to days before \
         it starts\n{report}"
    );

    // And what EDS made of the occurrence the client moved into another zone,
    // read back from the same cache and for the same reason as the two above:
    // whatever the server holds at this moment, a component set that lost the
    // instance's own zone would have the *next* save write it back on the
    // series' clock.
    assert_eq!(
        seen.get("zoned-occurrence-dtstart"),
        Some(&ZONED_MOVED_DTSTART),
        "EDS did not keep the occurrence at the wall-clock time it was moved \
         to\n{report}"
    );
    let moved_tzid = seen.get("zoned-occurrence-tzid").unwrap_or_else(|| {
        panic!("the client reported no zone for the moved occurrence\n{report}")
    });
    assert!(
        moved_tzid.ends_with(ZONED_MOVED_TIME_ZONE),
        "EDS kept the moved occurrence on {moved_tzid:?} rather than on the zone \
         it was moved to, so its wall-clock start now names another instant\n{report}"
    );

    // Eight objects for six events: `ECalCache` keys on (uid, rid), so each of
    // the two detached instances is a row of its own beside the series it
    // belongs to, and the split added a fifth event. Seven would mean the
    // moved occurrence never landed in the cache; five, that the split's new
    // event did not either.
    assert_eq!(
        seen.get("events-after"),
        Some(&"8"),
        "the added events are not all in the calendar they were added to\n{report}"
    );

    // And the other end: what the server was actually asked to do. The read
    // path is deliberately not asserted here — `ECalMetaBackend` schedules its
    // refresh rather than running it, so whether `CalendarEvent/query` has
    // happened by now is a race. The write is synchronous.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "CalendarEvent/set"),
        "the write never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let account = state
        .account(&account_id)
        .expect("the mock's default account");
    let events: Vec<_> = account
        .calendar_events
        .iter()
        .map(|(_, event)| event)
        .collect();
    assert_eq!(
        events.len(),
        6,
        "the server holds {} events, not six",
        events.len()
    );

    // Looked up by title rather than by position: the store is keyed on
    // server-assigned ids, and which of the two comes first says nothing.
    let by_title = |title: &str| {
        events
            .iter()
            .find(|event| event.title.as_deref() == Some(title))
            .unwrap_or_else(|| {
                panic!("no event titled {title:?} reached the server, only {events:?}")
            })
    };

    let event = by_title(SUMMARY);
    assert_eq!(
        event.start.as_deref(),
        Some(START),
        "the event on the server starts at the wrong time: {event:?}"
    );
    assert_eq!(
        event.duration.as_deref(),
        Some(DURATION),
        "the event on the server has the wrong length, so EDS's DTEND did not \
         survive the trip: {event:?}"
    );
    assert!(
        event
            .calendar_ids
            .as_ref()
            .is_some_and(|calendars| calendars.values().any(|included| *included)),
        "the event on the server is in no calendar: {event:?}"
    );
    // The place, as the server holds it: a one-entry map of a Location object,
    // not the string the component carried. Nothing below real EDS says whether
    // a `LOCATION` a libecal consumer wrote survives the trip through the
    // meta backend's cache to get here.
    assert_eq!(
        event.locations,
        Some(
            [(
                "l1".to_owned(),
                serde_json::json!({"@type": "Location", "name": LOCATION})
            )]
            .into()
        ),
        "the place the client named did not reach the server: {event:?}"
    );
    // The tags, as the server holds them: an RFC 8984 §1.4.3 Set of both, not the
    // first of a line libical split into two properties on the way here.
    assert_eq!(
        event
            .keywords
            .as_ref()
            .map(|tags| tags.keys().map(String::as_str).collect::<Vec<_>>()),
        Some(KEYWORDS.to_vec()),
        "the tags the client wrote did not reach the server: {event:?}"
    );
    // And the transparency, as the server holds it: the JSCalendar spelling of
    // the state, not the iCalendar one the component carried.
    assert_eq!(
        event.free_busy_status.as_deref(),
        Some(FREE_BUSY_STATUS),
        "the event reached the server blocking time, so the TRANSP the client \
         wrote was lost: {event:?}"
    );
    // And the importance, as the server holds it: the integer both formats spell
    // the same way, which is the reading a mapping that dropped the property would
    // leave as nothing at all.
    assert_eq!(
        event
            .priority
            .map(|priority| priority.to_string())
            .as_deref(),
        Some(PRIORITY),
        "the priority the client wrote did not reach the server: {event:?}"
    );

    // The all-day one, and the property that is the whole point of it: without
    // `showWithoutTime` the server holds a midnight appointment, which is what
    // every other client would then show.
    let all_day = by_title(ALL_DAY_SUMMARY);
    assert_eq!(
        all_day.show_without_time,
        Some(true),
        "the all-day event reached the server as a timed one: {all_day:?}"
    );
    assert_eq!(
        all_day.start.as_deref(),
        Some(ALL_DAY_START),
        "the all-day event starts on the wrong day: {all_day:?}"
    );
    assert_eq!(
        all_day.duration.as_deref(),
        Some(ALL_DAY_DURATION),
        "the all-day event is not a day long: {all_day:?}"
    );
    assert_eq!(
        all_day.time_zone, None,
        "a day has no zone (RFC 8984 §4.1.5): {all_day:?}"
    );

    // And the zoned one, which is the only assertion in this file that depends
    // on what the backend puts in the envelope *besides* the components EDS
    // handed it: the `TZID` on this event is libical's own, so without the
    // `VTIMEZONE` defining it the mapping has no name for the zone and sends
    // none. Start and zone are asserted together because either alone passes
    // for the other going wrong — a wall-clock start with no zone is an
    // appointment an hour or two off for everybody, and a zone on a start that
    // was silently converted to UTC is the same error stated twice.
    let zoned = by_title(ZONED_SUMMARY);
    assert_eq!(
        zoned.time_zone.as_deref(),
        Some(ZONED_TIME_ZONE),
        "the zone the event was created in did not reach the server, so the \
         envelope carried no definition for libical's identifier: {zoned:?}"
    );
    assert_eq!(
        zoned.start.as_deref(),
        Some(ZONED_START),
        "the zoned event does not start at the wall-clock time it was created \
         at: {zoned:?}"
    );
    assert_eq!(
        zoned.duration.as_deref(),
        Some(DURATION),
        "the zoned event has the wrong length: {zoned:?}"
    );

    // And the recurring one, whose EXDATE has to have become an override. The
    // rule is asserted alongside it because an event that lost its recurrence
    // has nothing for an exclusion to be an exception to.
    let recurring = by_title(RECURRING_SUMMARY);
    let rules = recurring
        .recurrence_rules
        .as_ref()
        .unwrap_or_else(|| panic!("the recurring event has no rule: {recurring:?}"));
    assert_eq!(rules[0].frequency, "weekly", "{recurring:?}");
    // Four rather than the six it was created with: the split truncated it, and
    // the count is the only thing on the server that says where the old series
    // now ends. Six here is the old series still recurring over the fortnight
    // the new one owns, which every other client reading the account would show
    // as two appointments a week apart under two titles.
    assert_eq!(rules[0].count, Some(4), "{recurring:?}");
    // The day the rule repeats on, as the NDay objects RFC 8984 §4.3.3 spells
    // it with. A rule that arrived without them is a weekly series pinned to
    // whatever day its start happens to fall on, which is the same event only
    // for as long as nobody moves the start.
    assert_eq!(
        rules[0].by_day.as_deref(),
        Some(&[NDay::new("th")][..]),
        "the day the series repeats on did not reach the server: {recurring:?}"
    );
    // All three exceptions in one map, because they share it: an override
    // written for one of them that dropped another is a deletion or a rename
    // undone, and asserting them one at a time would not notice.
    assert_eq!(
        recurring.recurrence_overrides,
        Some(
            [
                (
                    RECURRING_EXCLUDED.to_owned(),
                    serde_json::json!({"excluded": true}),
                ),
                (
                    RECURRING_EDITED.to_owned(),
                    serde_json::json!({"title": RECURRING_EDITED_SUMMARY}),
                ),
                (
                    RECURRING_REMOVED.to_owned(),
                    serde_json::json!({"excluded": true}),
                ),
            ]
            .into()
        ),
        "the deleted and the edited occurrences did not all reach the server as \
         overrides, so every other client shows a cancelled appointment or the \
         series' own title on a day the user changed: {recurring:?}"
    );

    // And the event the split made, which is an ordinary event on this side —
    // the whole point of what EDS did with `THIS_AND_FUTURE`. Its rule and its
    // start are what say the series was cut where the user asked; the absent
    // overrides are what say the two cancellations stayed with the half of the
    // series they belong to, rather than being copied onto days after the split
    // where the user never cancelled anything.
    let split = by_title(RECURRING_SPLIT_SUMMARY);
    assert_eq!(
        split.start.as_deref(),
        Some(RECURRING_SPLIT),
        "the event the split made does not start at the occurrence it was cut \
         at: {split:?}"
    );
    assert_eq!(
        split.duration.as_deref(),
        Some(DURATION),
        "the event the split made is not as long as the occurrences it \
         replaces: {split:?}"
    );
    let split_rules = split
        .recurrence_rules
        .as_ref()
        .unwrap_or_else(|| panic!("the event the split made has no rule: {split:?}"));
    assert_eq!(split_rules[0].frequency, "weekly", "{split:?}");
    assert_eq!(split_rules[0].count, Some(2), "{split:?}");
    assert_eq!(
        split_rules[0].by_day.as_deref(),
        Some(&[NDay::new("th")][..]),
        "the event the split made does not repeat on the day the series did: \
         {split:?}"
    );
    assert_eq!(
        split.recurrence_overrides, None,
        "the event the split made carries exceptions from before it starts: \
         {split:?}"
    );

    // And the zoned series, whose one override is the only place in this file
    // where two named zones meet. The series' own zone is asserted first
    // because the override's key is a wall-clock time *on it*: a series that
    // arrived floating or in UTC would make `2026-03-12T10:00:00` name a
    // different instant, and the override would then be attached to an
    // occurrence the rules never generated.
    let zoned_recurring = by_title(ZONED_RECURRING_SUMMARY);
    assert_eq!(
        zoned_recurring.time_zone.as_deref(),
        Some(ZONED_TIME_ZONE),
        "the zone the series was created in did not reach the server: \
         {zoned_recurring:?}"
    );
    assert_eq!(
        zoned_recurring.start.as_deref(),
        Some(ZONED_RECURRING_START),
        "the zoned series does not start at the wall-clock time it was created \
         at: {zoned_recurring:?}"
    );
    assert_eq!(
        zoned_recurring.duration.as_deref(),
        Some(ZONED_RECURRING_DURATION),
        "the zoned series has the wrong length: {zoned_recurring:?}"
    );
    let zoned_rules = zoned_recurring
        .recurrence_rules
        .as_ref()
        .unwrap_or_else(|| panic!("the zoned series has no rule: {zoned_recurring:?}"));
    assert_eq!(zoned_rules[0].frequency, "weekly", "{zoned_recurring:?}");
    assert_eq!(zoned_rules[0].count, Some(3), "{zoned_recurring:?}");
    // The whole point of the event: the moved occurrence, carrying both the
    // wall-clock start the user put it at and the clock that start is on. A
    // patch of `{"start": …}` alone — which is what the mapping sent before it
    // learned `timeZone` — is a five-hour error the server cannot see and no
    // other client can correct.
    assert_eq!(
        zoned_recurring.recurrence_overrides,
        Some(
            [(
                ZONED_MOVED_INSTANCE.to_owned(),
                serde_json::json!({
                    "start": ZONED_MOVED_START,
                    "timeZone": ZONED_MOVED_TIME_ZONE,
                }),
            )]
            .into()
        ),
        "the occurrence the user moved into another zone did not reach the \
         server on that zone, so every other client shows it five hours from \
         where it was put: {zoned_recurring:?}"
    );
}
