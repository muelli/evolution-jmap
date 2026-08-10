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

/// The event the client writes. The summary is passed on its command line
/// and looked for in the mock's store, so the two ends cannot disagree about
/// it by a typo; the start is the JSCalendar spelling of the `DTSTART` in
/// `tests/functional/cal-client.c`.
const SUMMARY: &str = "Sprint planning";
const START: &str = "2026-01-15T13:00:00";

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

/// The third event: a weekly one with a single occurrence deleted, which EDS
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
const SERIES_RRULE: &str = "FREQ=WEEKLY;COUNT=4";
const SPLIT_RRULE: &str = "FREQ=WEEKLY;COUNT=2";
const SPLIT_DTSTART: &str = "20260212T130000Z";

/// The same two exclusions as EDS spells them back in its own cache: the one
/// the client wrote with the event and the one it removed afterwards, both in
/// the series' UTC. Named rather than counted, because two exclusions of which
/// one names the wrong day is one cancelled appointment that comes back and
/// another that was never cancelled at all.
const RECURRING_EXDATES: [&str; 2] = ["20260129T130000Z", "20260205T130000Z"];

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

    for key in ["added", "added-all-day", "added-recurring"] {
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

    // Five objects for four events: `ECalCache` keys on (uid, rid), so the
    // detached instance is a row of its own beside the series it belongs to,
    // and the split added a fourth event. Four would mean the split's new event
    // never landed in the cache; three, that the edit did not either.
    assert_eq!(
        seen.get("events-after"),
        Some(&"5"),
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
        4,
        "the server holds {} events, not four",
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
        split.recurrence_overrides, None,
        "the event the split made carries exceptions from before it starts: \
         {split:?}"
    );
}
