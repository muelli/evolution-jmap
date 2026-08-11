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
//!
//! Two legs, and they ask opposite questions, so they run different client
//! programs. The first creates every event it looks at, which is what a user
//! making an appointment does; the second starts from an event the *server*
//! held before EDS ever connected, carrying members no iCalendar line can
//! state, and asks what survives the round trip out to a component and back
//! through a save. Only an event nobody here created can ask that.

use std::collections::BTreeMap;

use jmap_functional::{Session, observations, required_path};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, NDay};

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

/// And who may see it — the `CLASS` in `tests/functional/cal-client.c`, which has
/// to reach the server as a JSCalendar `privacy` (RFC 8984 §4.4.3). The one mapped
/// property whose iCalendar value libical holds as an enum rather than as text, so
/// this is the leg that says a property EDS's cache can only keep by *recognising*
/// it survives the trip. CONFIDENTIAL because it is the value whose two spellings
/// differ, so the pair below also says the translation happened rather than a
/// string being copied.
const CLASS: &str = "CONFIDENTIAL";
const PRIVACY: &str = "secret";

/// And when the user is reminded of it — the `VALARM` in
/// `tests/functional/cal-client.c`, which has to reach the server as an entry in
/// a JSCalendar `alerts` map (RFC 8984 §4.5.2). The one mapped property that is a
/// child *component*, so this is the leg that says `ECalMetaBackend`'s cache
/// keeps a component nested inside the one it was handed — and that the key of
/// the entry, which rides on the alarm's RFC 9074 §6 `UID`, is the one the client
/// wrote rather than the one `jmap-ical` invents for an alarm that names none.
const ALARM_UID: &str = "k1";
const ALARM_TRIGGER: &str = "-PT15M";

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

/// And the tags that one occurrence carries — the `CATEGORIES` on the detached
/// component in `tests/functional/cal-client.c`, which has to reach the server as
/// a `keywords` patch in that instance's override (RFC 8984 §4.3.4). The series
/// carries none, so this set is the whole of the difference between them; an
/// instance whose tags EDS's cache dropped reads back as one the user just
/// unfiled, and the save after it would tell the server so. Sorted, since that is
/// the order a set is held in on both sides.
const RECURRING_EDITED_KEYWORDS: [&str; 2] = ["cancelled", "offsite"];

/// And when that one occurrence reminds — the `VALARM` inside the detached
/// component in `tests/functional/cal-client.c`, which has to reach the server as
/// an `alerts` patch in that instance's override. The series carries no reminder,
/// so this one is the whole of the difference between them, and it is the deepest
/// thing this leg asks EDS's cache to keep: a child component of the child
/// component the instance itself is.
///
/// Keyed by the alarm's RFC 9074 §6 `UID`, because that is what the entry is named
/// by: an alarm EDS handed back without it would arrive under a positional key
/// this mapping invented, which is the same reminder saved under a name nobody
/// chose. The offset is negative and stated as text at both ends, since a reminder
/// an hour *after* the meeting is the failure a lost sign looks like.
const RECURRING_EDITED_ALERT_KEY: &str = "k1";
const RECURRING_EDITED_ALARM_TRIGGER: &str = "-PT1H";

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

/// The event the second leg starts from — one the *server* holds, which is the
/// only way to ask the question that leg asks. Every event above is created
/// through EDS, so its `locations` and `virtualLocations` hold exactly what an
/// iCalendar line can state and there is nothing for a round trip to lose.
///
/// The title is asserted from both ends, so a mix-up cannot pass; the start and
/// the length are `CalendarEvent::simple`'s shape, since neither is what this
/// leg is about.
const PLACED_TITLE: &str = "Design review";
const PLACED_START: &str = "2026-04-09T10:00:00";
const PLACED_DURATION: &str = "PT1H";

/// The place that event happens at, under a key only a server would choose, and
/// with a member no `LOCATION` line has room for.
///
/// RFC 8984 §4.2.5's Location holds a `description`, `coordinates`,
/// `locationTypes` and more besides its `name`; RFC 5545 §3.6.1 gives a `VEVENT`
/// one line of text. That gap is why the save patches `locations/<key>/name`
/// rather than replacing the property — and the `description` is what says it
/// did: a save that replaced `locations` would delete a note the user was never
/// shown, and this leg would see it gone from the server.
const PLACED_LOCATION_KEY: &str = "srv-loc";
const PLACED_LOCATION_NAME: &str = "Room 42";
const PLACED_LOCATION_DESCRIPTION: &str = "third floor, past the lift";

/// And where it may be joined online, likewise under the server's own key and
/// likewise holding a `description` the `CONFERENCE` line cannot carry.
///
/// This is the entry that makes the `X-JMAP-KEY` load-bearing, and the reason the
/// leg edits it too. RFC 7986 §5.11 admits several `CONFERENCE` lines, so
/// `jmap-ical` reads the key off the line and the save patches
/// `virtualLocations/<key>/uri`; a key EDS dropped between the load and the save
/// leaves the mapping holding a key it invented, which names no entry on the
/// server, and the edit reaches nothing. A `LOCATION` cannot ask that — RFC 5545
/// §3.6.1 allows one, so the save finds the single entry in the server's own map
/// whatever the line carries.
///
/// The `name` rides on the line as a `LABEL`, which is the one parameter here
/// that says a *standard* parameter came back too — a cache that kept only what
/// libical has an enum for would answer that one and drop the key.
const PLACED_CONFERENCE_KEY: &str = "srv-conf";
const PLACED_CONFERENCE_URI: &str = "https://meet.example/design-review";
const PLACED_CONFERENCE_NAME: &str = "Video bridge";
const PLACED_CONFERENCE_DESCRIPTION: &str = "dial in from the lobby phone";

/// And the document the event points at — the third of the three maps the save
/// patches into, and the one whose line libical does not leave as text.
///
/// An `ATTACH` is why this is a separate question from the `CONFERENCE` above.
/// RFC 5545 §3.8.1.1 gives the property a value type of its own, and libical
/// parses it into an `icalattach` rather than holding the string: the parameters
/// ride beside a value the library has re-built, which is exactly the shape in
/// which a cache loses one. Whether EDS carries the `X-JMAP-KEY` on *this*
/// property was inferred from the two above until this leg measured it.
///
/// The `title` is the member with no room on the line at all, so it plays the
/// part `description` plays for the two places: a save that replaced `links`
/// would delete it. The `contentType` and the `size` are shown — an `FMTTYPE`
/// and a `SIZE` — and still never written back, since they are the server's
/// description of what it holds rather than a field the user was offered; they
/// are asserted after the save for that reason, not as decoration.
///
/// This is the resource the user does **not** touch. It is drawn first — the
/// entries go out in key order, and `srv-att` sorts before
/// [`PLACED_EDITED_ATTACH_KEY`] — so it is also the line a save that meant "the
/// attachment" rather than "that one" would reach.
const PLACED_ATTACH_KEY: &str = "srv-att";
const PLACED_ATTACH_HREF: &str = "https://files.example/design-review/agenda.pdf";
const PLACED_ATTACH_TITLE: &str = "Agenda, with the numbers";
const PLACED_ATTACH_MEDIA_TYPE: &str = "application/pdf";
const PLACED_ATTACH_SIZE: u64 = 20480;

/// And the second document, which is the case the key exists for.
///
/// One resource per event is the shape in which a lost `X-JMAP-KEY` merely
/// *fails*: whatever the line carries, there is one entry in the server's map and
/// the save either names it or names nothing. RFC 5545 §3.8.1.1 admits any number
/// of `ATTACH` lines and RFC 8984 §4.2.7 a whole map of Links, so with two the
/// same loss *corrupts* — a line whose key EDS dropped, or carried over from its
/// neighbour, is a save that re-addresses a document the user never touched and
/// loses the edit they made. Nothing below real EDS can ask that: every fixture in
/// `jmap-ical` and `jmap-cal-sync` writes the parameter by hand, so the line is
/// paired with its entry there by construction.
///
/// This is the one the user re-addresses, and deliberately the one drawn
/// *second*: a save that took the first `ATTACH` for the resource being edited
/// reaches [`PLACED_ATTACH_KEY`] instead, which the assertions on both ends then
/// see as the wrong document moved and the right one left where it was.
///
/// A media type and a size of its own, both different from the first entry's, so
/// that a parameter reported against the wrong line is a difference rather than a
/// coincidence.
const PLACED_EDITED_ATTACH_KEY: &str = "srv-att-2";
const PLACED_EDITED_ATTACH_HREF: &str = "https://files.example/design-review/slides.odp";
const PLACED_EDITED_ATTACH_TITLE: &str = "Slides, as presented";
const PLACED_EDITED_ATTACH_MEDIA_TYPE: &str = "application/vnd.oasis.opendocument.presentation";
const PLACED_EDITED_ATTACH_SIZE: u64 = 51200;

/// The place the user retypes into the appointment editor's Location field. A
/// different room, so a value half-written shows up as neither.
const REPLACED_LOCATION_NAME: &str = "Room 7";

/// And the address the event is joined at afterwards. Not something Evolution
/// 3.52 offers a control for — see `tests/functional/cal-edit-client.c` — so this
/// is the edit another client on the same account makes; the mapping has a path
/// for it, and this is what says the path works through real EDS. A different
/// host as well as a different path, so a URI half-rewritten is neither.
const REPLACED_CONFERENCE_URI: &str = "https://call.example/rescheduled";

/// And the address of the second document afterwards — the edit an attachment can
/// take through a libecal consumer, since `href` is the one member of a Link this
/// mapping writes back. A different host as well as a different path, so a URI
/// half-rewritten is neither.
///
/// The client is told which line to re-address by the address it already carries,
/// not by its position or by the key: that is how a user picks the attachment
/// they meant, and it keeps the program free of any notion of what the mapping
/// writes on a line. See `tests/functional/cal-edit-client.c`.
const REPLACED_ATTACH_HREF: &str = "https://docs.example/design-review/slides-2.odp";

/// Put the event the second leg starts from into the mock's store, and hand back
/// the id the server filed it under — which is also the `UID` EDS keys its cache
/// on, since that is what `jmap-ical` writes there.
///
/// Seeded straight into the store rather than written through EDS, because the
/// shape under test is one no iCalendar document can state: an event created
/// through EDS arrives with a place whose whole content is the name on the line,
/// leaving nothing for the save to preserve — or to lose.
fn seed_placed_event(server: &jmap_mock::MockServer) -> Id {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().expect("mock state lock");
    let account = state
        .account_mut(&account_id)
        .expect("the mock's default account");
    let calendar = account.seed_calendar("Personal", true);

    let id = account.calendar_events.alloc_id();
    let mut event = CalendarEvent::simple(calendar, PLACED_TITLE, PLACED_START, PLACED_DURATION);
    event.id = Some(id.clone());
    // What a server assigns; the mock's own `CalendarEvent/set` fills the same
    // shape in, and seeding bypasses it.
    event.uid = Some(format!("urn:example:event:{}", id.as_str()));
    event.locations = Some(
        [(
            PLACED_LOCATION_KEY.to_owned(),
            serde_json::json!({
                "@type": "Location",
                "name": PLACED_LOCATION_NAME,
                "description": PLACED_LOCATION_DESCRIPTION,
            }),
        )]
        .into(),
    );
    event.virtual_locations = Some(
        [(
            PLACED_CONFERENCE_KEY.to_owned(),
            serde_json::json!({
                "@type": "VirtualLocation",
                "uri": PLACED_CONFERENCE_URI,
                "name": PLACED_CONFERENCE_NAME,
                "description": PLACED_CONFERENCE_DESCRIPTION,
            }),
        )]
        .into(),
    );
    // Two resources rather than one, because that is the shape in which a key
    // lost between the drawing and the save costs the user a document rather
    // than an edit — see [`PLACED_EDITED_ATTACH_KEY`].
    event.links = Some(
        [
            (
                PLACED_ATTACH_KEY.to_owned(),
                serde_json::json!({
                    "@type": "Link",
                    "href": PLACED_ATTACH_HREF,
                    "contentType": PLACED_ATTACH_MEDIA_TYPE,
                    "size": PLACED_ATTACH_SIZE,
                    "title": PLACED_ATTACH_TITLE,
                }),
            ),
            (
                PLACED_EDITED_ATTACH_KEY.to_owned(),
                serde_json::json!({
                    "@type": "Link",
                    "href": PLACED_EDITED_ATTACH_HREF,
                    "contentType": PLACED_EDITED_ATTACH_MEDIA_TYPE,
                    "size": PLACED_EDITED_ATTACH_SIZE,
                    "title": PLACED_EDITED_ATTACH_TITLE,
                }),
            ),
        ]
        .into(),
    );
    account.calendar_events.seed_with_id(id.clone(), event);
    id
}

/// What the client reported of one `ATTACH` line: the address libical parsed out
/// of it, and the two parameters it carries that libical has an enum for.
///
/// Compared whole rather than one observation at a time, because the question is
/// what stands on *one* line — a media type reported against the wrong resource
/// is exactly the failure this leg exists to catch, and three separate
/// assertions would each be satisfied by the other line's value.
#[derive(Debug, PartialEq, Eq)]
struct Resource<'a> {
    href: &'a str,
    media_type: &'a str,
    size: &'a str,
}

/// The line the server's entry `key` was drawn onto, as the client reported it.
///
/// Found by the `X-JMAP-KEY` the line carries and never by where it stands: the
/// order two `ATTACH` properties come back in is `ECalMetaBackend`'s business and
/// not something this leg means to pin down, and reading the second line as "the
/// second entry" would turn a swap — the very corruption two resources make
/// possible — into a pass.
///
/// `None` for a component holding no line under that key, which is what a dropped
/// key looks like from here: the client reports every line it found, so a key
/// nothing answers to is either absent or on the wrong property.
fn resource<'a>(
    seen: &BTreeMap<&'a str, &'a str>,
    prefix: &str,
    key: &str,
) -> Option<Resource<'a>> {
    let lines: usize = seen
        .get(format!("{prefix}-attaches").as_str())?
        .parse()
        .ok()?;
    let at = |index: usize, member: &str| {
        seen.get(format!("{prefix}-attach-{index}{member}").as_str())
            .copied()
    };
    let index = (1..=lines).find(|index| at(*index, "-key") == Some(key))?;
    Some(Resource {
        href: at(index, "")?,
        media_type: at(index, "-fmttype")?,
        size: at(index, "-size")?,
    })
}

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
    // And who may see it. An empty string here is a component EDS handed back with
    // no CLASS on it, which reads as the PUBLIC both formats default to — so an
    // event the user hid would come back visible, and the next save would write
    // that over the server's own classification.
    assert_eq!(
        seen.get("read-back-class"),
        Some(&CLASS),
        "the event EDS handed back lost its classification\n{report}"
    );
    // And when it reminds the user. An empty string here is a component EDS handed
    // back with no VALARM in it at all — the reminder the client set would be gone
    // from Evolution's own view of the appointment, and the next save would delete
    // it from the server too.
    assert_eq!(
        seen.get("read-back-alarm-trigger"),
        Some(&ALARM_TRIGGER),
        "the event EDS handed back lost its reminder\n{report}"
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
    // And the tags on that same instance, which the cache has to keep *per
    // component*: the set stated there is the only statement of what that one
    // occurrence is filed under, so a cache holding the series' tags in its place
    // would read back as the user having refiled the day they renamed. Sorted,
    // for the reason the series' own tags are.
    let mut occurrence_tags: Vec<&str> = seen
        .get("edited-occurrence-categories")
        .unwrap_or_else(|| panic!("the client reported no tags on the occurrence\n{report}"))
        .split(',')
        .filter(|tag| !tag.is_empty())
        .collect();
    occurrence_tags.sort_unstable();
    assert_eq!(
        occurrence_tags, RECURRING_EDITED_KEYWORDS,
        "EDS did not keep the tags on the occurrence the client edited\n{report}"
    );
    // And the reminder on that same instance, kept in the same per-component way
    // one nesting level further down: an empty string here is a cache that handed
    // the alarm back on the series, or dropped it, and either way the next save
    // tells the server the user cleared a reminder they had just set.
    assert_eq!(
        seen.get("edited-occurrence-alarm-trigger"),
        Some(&RECURRING_EDITED_ALARM_TRIGGER),
        "EDS did not keep the reminder on the occurrence the client edited\n{report}"
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
    // And the classification, as the server holds it: the JSCalendar spelling of
    // the value, not the iCalendar one the component carried.
    assert_eq!(
        event.privacy.as_deref(),
        Some(PRIVACY),
        "the classification the client wrote did not reach the server: {event:?}"
    );
    // And the reminder, as the server holds it: an Alert object under the key the
    // alarm's own UID named, not the one `jmap-ical` invents for an alarm that
    // carries none — so this also says libecal kept that UID beside the
    // X-EVOLUTION-ALARM-UID it adds of its own.
    assert_eq!(
        event.alerts,
        Some(
            [(
                ALARM_UID.to_owned(),
                serde_json::json!({
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": ALARM_TRIGGER},
                    "action": "display",
                })
            )]
            .into()
        ),
        "the reminder the client set did not reach the server: {event:?}"
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
                    // The tags and the reminder beside the title, because the
                    // component states all three on that one instance and the
                    // override is what carries any of them: a patch holding only
                    // the title is the user's filing of that occurrence, or the
                    // reminder they set on it, lost between EDS and the server.
                    serde_json::json!({
                        "title": RECURRING_EDITED_SUMMARY,
                        "keywords": RECURRING_EDITED_KEYWORDS
                            .iter()
                            .map(|tag| ((*tag).to_owned(), serde_json::json!(true)))
                            .collect::<serde_json::Map<_, _>>(),
                        "alerts": {
                            RECURRING_EDITED_ALERT_KEY: {
                                "@type": "Alert",
                                "action": "display",
                                "trigger": {
                                    "@type": "OffsetTrigger",
                                    "offset": RECURRING_EDITED_ALARM_TRIGGER,
                                },
                            },
                        },
                    }),
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

#[test]
fn retyping_a_place_through_eds_patches_the_entry_the_server_chose() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_EDIT_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let event_id = seed_placed_event(&server);
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/calendar-replace"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let output = session.run(
        &client,
        &[
            "jmap-functional",
            event_id.as_str(),
            REPLACED_LOCATION_NAME,
            REPLACED_CONFERENCE_URI,
            // Which of the two documents is being re-addressed, named by the
            // address it carries — see [`REPLACED_ATTACH_HREF`].
            PLACED_EDITED_ATTACH_HREF,
            REPLACED_ATTACH_HREF,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // The connect first, for the reason the leg above spells out: a calendar the
    // backend never opened turns every later failure into a message about the
    // wrong thing.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the calendar read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("read-summary"),
        Some(&PLACED_TITLE),
        "the event EDS handed back is not the one the mock was seeded with\n{report}"
    );

    // What a libecal consumer was shown of the server's two places. The values
    // say the drawing arrived at all; the keys say a save can reach back into the
    // entry it was drawn from, which is the claim this leg exists for and one no
    // fixture in `jmap-ical` or `jmap-cal-sync` can make — they supply the
    // component by hand, so the parameter is there by construction.
    assert_eq!(
        seen.get("read-location"),
        Some(&PLACED_LOCATION_NAME),
        "the place the server holds did not reach the LOCATION line\n{report}"
    );
    // Reported rather than relied on: the save finds the one entry RFC 5545
    // §3.6.1 lets a `LOCATION` stand for in the server's own map, so a key lost
    // here would cost nothing yet. It is asserted because the day the mapping
    // draws a second place — RFC 8984 §4.2.5 allows any number — it will read
    // this parameter, and a silent change in what EDS carries should fail then
    // rather than corrupt.
    assert_eq!(
        seen.get("read-location-key"),
        Some(&PLACED_LOCATION_KEY),
        "EDS did not carry the X-JMAP-KEY the LOCATION went out with\n{report}"
    );
    assert_eq!(
        seen.get("read-locations"),
        Some(&"1"),
        "EDS holds a LOCATION line other than the one the mapping wrote\n{report}"
    );
    assert_eq!(
        seen.get("read-conference"),
        Some(&PLACED_CONFERENCE_URI),
        "the place the event may be joined at did not reach the CONFERENCE \
         line\n{report}"
    );
    // This one is load-bearing, and it is the observation the leg was written
    // for: the mapping finds the server's virtual location by the key on the
    // line, so an edit of the line below reaches the entry only if EDS carried
    // this parameter through its cache.
    assert_eq!(
        seen.get("read-conference-key"),
        Some(&PLACED_CONFERENCE_KEY),
        "EDS did not carry the X-JMAP-KEY the CONFERENCE went out with, so no \
         save can name the entry the server chose\n{report}"
    );
    assert_eq!(
        seen.get("read-conferences"),
        Some(&"1"),
        "EDS holds a CONFERENCE line other than the one the mapping wrote\n{report}"
    );
    // And the standard parameter beside the invented one, which is a separate
    // question: RFC 7986 §5.11's LABEL is a parameter libical has an enum for,
    // and a cache that kept only the parameters it recognises would answer this
    // one and drop the key above.
    assert_eq!(
        seen.get("read-conference-label"),
        Some(&PLACED_CONFERENCE_NAME),
        "EDS did not carry the LABEL naming the conference\n{report}"
    );

    // And the two documents the event points at, read through libical's own
    // `icalattach` rather than as a string — see `cal-edit-client.c`. Each is
    // looked up by the `X-JMAP-KEY` on its line, which is both how the assertion
    // stays free of the order EDS hands the lines back in and the observation
    // this half of the leg was written for: `ATTACH` is not a text property, so a
    // cache that round-tripped the value and dropped what stood beside it would
    // show up here and nowhere else in this file. A key that reached the wrong
    // line answers to the wrong address, which is a mismatch here rather than a
    // missing entry.
    //
    // The address says the drawing arrived; the key says a save can name the entry
    // it came from, which for a `links` entry is the only way back, since the save
    // compares the address the server stated against the one on the line. The
    // `FMTTYPE` and the `SIZE` beside them make the same argument the conference's
    // `LABEL` does: a cache keeping only what libical has an enum for would answer
    // those two and drop the key.
    assert_eq!(
        seen.get("read-attaches"),
        Some(&"2"),
        "EDS does not hold exactly the two ATTACH lines the mapping wrote\n{report}"
    );
    assert_eq!(
        resource(&seen, "read", PLACED_ATTACH_KEY),
        Some(Resource {
            href: PLACED_ATTACH_HREF,
            media_type: PLACED_ATTACH_MEDIA_TYPE,
            size: &PLACED_ATTACH_SIZE.to_string(),
        }),
        "the first resource the server holds did not reach an ATTACH line under \
         the key the server chose\n{report}"
    );
    assert_eq!(
        resource(&seen, "read", PLACED_EDITED_ATTACH_KEY),
        Some(Resource {
            href: PLACED_EDITED_ATTACH_HREF,
            media_type: PLACED_EDITED_ATTACH_MEDIA_TYPE,
            size: &PLACED_EDITED_ATTACH_SIZE.to_string(),
        }),
        "the second resource the server holds did not reach an ATTACH line under \
         the key the server chose, so a save can only guess which document the \
         user re-addressed\n{report}"
    );

    // And what EDS holds after the save: the place the user typed, on the line it
    // was typed onto, still carrying the key. One line, because a save that
    // *added* a place would leave two.
    assert_eq!(
        seen.get("read-back-location"),
        Some(&REPLACED_LOCATION_NAME),
        "the place the user typed did not survive the save\n{report}"
    );
    assert_eq!(
        seen.get("read-back-location-key"),
        Some(&PLACED_LOCATION_KEY),
        "the save left the server's entry under a key nobody chose\n{report}"
    );
    assert_eq!(
        seen.get("read-back-locations"),
        Some(&"1"),
        "the save left the old place on the event beside the new one\n{report}"
    );
    assert_eq!(
        seen.get("read-back-conference"),
        Some(&REPLACED_CONFERENCE_URI),
        "the address the event is joined at did not survive the save\n{report}"
    );
    assert_eq!(
        seen.get("read-back-conference-key"),
        Some(&PLACED_CONFERENCE_KEY),
        "the save refiled the conference under another key\n{report}"
    );
    assert_eq!(
        seen.get("read-back-conferences"),
        Some(&"1"),
        "the save left the old address on the event beside the new one\n{report}"
    );
    assert_eq!(
        seen.get("read-back-attaches"),
        Some(&"2"),
        "the save left the old resource on the event beside the new one, or lost \
         the one the user did not touch\n{report}"
    );
    assert_eq!(
        resource(&seen, "read-back", PLACED_EDITED_ATTACH_KEY),
        Some(Resource {
            href: REPLACED_ATTACH_HREF,
            media_type: PLACED_EDITED_ATTACH_MEDIA_TYPE,
            size: &PLACED_EDITED_ATTACH_SIZE.to_string(),
        }),
        "the address of the document the user re-addressed did not survive the \
         save, still under the key the server chose\n{report}"
    );
    // And the document the user did not touch, which is what having two of them
    // is for: it comes back at the address it went out with, under its own key.
    // A save that reached the wrong entry shows up here as the resource that was
    // never edited pointing somewhere else.
    assert_eq!(
        resource(&seen, "read-back", PLACED_ATTACH_KEY),
        Some(Resource {
            href: PLACED_ATTACH_HREF,
            media_type: PLACED_ATTACH_MEDIA_TYPE,
            size: &PLACED_ATTACH_SIZE.to_string(),
        }),
        "the resource the user never touched did not come back as it went out\n{report}"
    );

    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "CalendarEvent/set"),
        "the new place never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let event = state
        .account(&account_id)
        .expect("the mock's default account")
        .calendar_events
        .get(&event_id)
        .unwrap_or_else(|| panic!("the save removed the event the mock was seeded with"));

    // The whole of the claim, on the server's side: the entry the server chose,
    // under the key it chose, renamed — and still holding the description the
    // line had no room for. A save that named `locations` rather than
    // `locations/<key>/name` passes every assertion above this one and fails
    // this one, and what it costs the user is a note they never saw and could
    // not have meant to delete.
    assert_eq!(
        event.locations,
        Some(
            [(
                PLACED_LOCATION_KEY.to_owned(),
                serde_json::json!({
                    "@type": "Location",
                    "name": REPLACED_LOCATION_NAME,
                    "description": PLACED_LOCATION_DESCRIPTION,
                }),
            )]
            .into()
        ),
        "the place the user retyped did not reach the server as a patch of the \
         entry it was drawn from: {event:?}"
    );
    // And the entry beside it, which is the half of the leg the round trip can
    // actually break: the address is the new one, under the key the server chose,
    // and the `description` and the `name` it never showed are still there. The
    // mapping found that entry by the `X-JMAP-KEY` and nothing else, so a cache
    // that dropped the parameter leaves this holding the old address — a change
    // the user made and every other client keeps missing.
    assert_eq!(
        event.virtual_locations,
        Some(
            [(
                PLACED_CONFERENCE_KEY.to_owned(),
                serde_json::json!({
                    "@type": "VirtualLocation",
                    "uri": REPLACED_CONFERENCE_URI,
                    "name": PLACED_CONFERENCE_NAME,
                    "description": PLACED_CONFERENCE_DESCRIPTION,
                }),
            )]
            .into()
        ),
        "the new address did not reach the server as a patch of the entry the \
         line was drawn from: {event:?}"
    );
    // And the resources, which ask the narrowest question of the three: only
    // `href` goes back, and only for the one entry the user re-addressed. The
    // `title` had no room on the line and would be gone had the save named
    // `links`; the `contentType` and the `size` were *shown* and are still the
    // server's own, so a save that wrote them back — reading an editor's re-typed
    // line as "the media type was cleared" — fails here even though every
    // observation the client made would still pass.
    //
    // Both entries in one assertion, because the pair is the claim: a save that
    // patched `links/srv-att/href` instead has moved a document the user never
    // opened and left theirs where it was, and either entry alone would still
    // read as somebody's edit landing.
    assert_eq!(
        event.links,
        Some(
            [
                (
                    PLACED_ATTACH_KEY.to_owned(),
                    serde_json::json!({
                        "@type": "Link",
                        "href": PLACED_ATTACH_HREF,
                        "contentType": PLACED_ATTACH_MEDIA_TYPE,
                        "size": PLACED_ATTACH_SIZE,
                        "title": PLACED_ATTACH_TITLE,
                    }),
                ),
                (
                    PLACED_EDITED_ATTACH_KEY.to_owned(),
                    serde_json::json!({
                        "@type": "Link",
                        "href": REPLACED_ATTACH_HREF,
                        "contentType": PLACED_EDITED_ATTACH_MEDIA_TYPE,
                        "size": PLACED_EDITED_ATTACH_SIZE,
                        "title": PLACED_EDITED_ATTACH_TITLE,
                    }),
                ),
            ]
            .into()
        ),
        "the resource the user re-addressed did not reach the server as a patch \
         of the entry it was drawn from, leaving the other where it was: {event:?}"
    );
    // And the title, which says the save patched the event rather than replacing
    // it with what the one component EDS handed over could state.
    assert_eq!(
        event.title.as_deref(),
        Some(PLACED_TITLE),
        "the save renamed the event: {event:?}"
    );
}
