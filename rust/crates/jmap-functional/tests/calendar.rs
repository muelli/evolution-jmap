// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar functional tests: `evolution-calendar-factory` loads
//! `libecalbackendjmap.so`, opens a calendar from a `.source` keyfile, and
//! sends writes through it to the mock JMAP server.
//!
//! This file is the twin of `address-book.rs`, because the two backends are
//! mirrors of each other. Each test checks only the two ends. The client
//! program reports what EDS gave a libecal consumer. The mock reports what the
//! backend sent to the server.
//!
//! There are five legs, and each runs its own client mode or program:
//!
//! 1. The client creates every event it examines, as a user does.
//! 2. The client starts from an event that the server holds, with members that
//!    no iCalendar line can state, and edits it.
//! 3. The client reads events in zones that only the server can name, then
//!    renames one and moves its start.
//! 4. The client defines a zone that no database holds and creates an event in
//!    it.
//! 5. The client creates a series in such a zone and moves one occurrence into
//!    a second such zone.

use std::collections::BTreeMap;

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, NDay};

/// The first event that the client writes. The client receives the summary on
/// its command line, and the test finds it in the mock's store. The start is
/// the JSCalendar form of the `DTSTART` in `tests/functional/cal-client.c`.
const SUMMARY: &str = "Sprint planning";
const START: &str = "2026-01-15T13:00:00";

/// The `LOCATION` in `tests/functional/cal-client.c`. It must reach the server
/// as an entry in the JSCalendar `locations` map (RFC 8984 §4.2.5). The key is
/// the one that `jmap-ical` makes for a component that has no key.
const LOCATION: &str = "Room 42";

/// The `CATEGORIES` in `tests/functional/cal-client.c`, which must reach the
/// server as a JSCalendar `keywords` set (RFC 8984 §4.2.9). There are two tags
/// because libical writes a multi-valued `CATEGORIES` as one property per
/// value. A mapping that reads only the first loses the rest. Sorted,
/// because both sides keep a set in that order.
const KEYWORDS: [&str; 2] = ["offsite", "planning"];

/// The `TRANSP` in `tests/functional/cal-client.c`, which must reach the server
/// as a JSCalendar `freeBusyStatus` (RFC 8984 §4.4.2). Both formats default to
/// the opaque state, so only the transparent state shows that the value crossed.
const TRANSP: &str = "TRANSPARENT";
const FREE_BUSY_STATUS: &str = "free";

/// The `PRIORITY` in `tests/functional/cal-client.c`, which must reach the
/// server as a JSCalendar `priority` (RFC 8984 §4.4.1). This is the only mapped
/// numeric property. The value is 1 because both formats read 0 as no value.
const PRIORITY: &str = "1";

/// The `CLASS` in `tests/functional/cal-client.c`. It must reach the server as a
/// JSCalendar `privacy` (RFC 8984 §4.4.3). libical keeps this value as an enum,
/// so EDS's cache keeps it only if it recognises it. CONFIDENTIAL has different
/// spellings in the two formats, so the pair also shows that a translation
/// occurred.
const CLASS: &str = "CONFIDENTIAL";
const PRIVACY: &str = "secret";

/// The `VALARM` in `tests/functional/cal-client.c`, which must reach the server
/// as an entry in the JSCalendar `alerts` map (RFC 8984 §4.5.2). This is the only
/// mapped child component, so it shows that `ECalMetaBackend`'s cache keeps a
/// nested component. The entry key comes from the alarm's RFC 9074 §4 `UID`, not
/// from the key that `jmap-ical` makes for an alarm without one.
const ALARM_UID: &str = "k1";
const ALARM_TRIGGER: &str = "-PT15M";

/// The length of the first event. The client states it as a `DTEND`, as
/// Evolution's editor does, 90 minutes after the start.
const DURATION: &str = "PT1H30M";

/// The second event: an all-day event, `VALUE=DATE` on both ends. On the server
/// it must be JSCalendar's `showWithoutTime`, start at midnight and last one
/// day. If not, other clients show a midnight appointment.
const ALL_DAY_SUMMARY: &str = "Team offsite";
const ALL_DAY_START: &str = "2026-02-01T00:00:00";
const ALL_DAY_DURATION: &str = "P1D";

/// The third event, in a named zone. The client builds it with libical's
/// builtin zone, as Evolution's editor does, so the `TZID` that reaches the
/// backend is libical's own `/freeassociation.sourceforge.net/Europe/Berlin`.
/// That is not an RFC 8984 §1.4.8 `TimeZoneId`.
///
/// The mapping can translate that identifier only from the `VTIMEZONE` beside
/// it, and `marshal::icalendar_from_instances` decides whether that `VTIMEZONE`
/// travels with the component. The mapping's own tests supply both by hand, so
/// only real EDS can show the result. A `time_zone` of `None` here is a known
/// shipped bug: the appointment floats on the server.
///
/// The start is the wall-clock time in that zone, which is what JSCalendar's
/// `start` means beside a `timeZone` (RFC 8984 §5.1.1). It is not the UTC
/// instant.
const ZONED_SUMMARY: &str = "Berlin review";
const ZONED_START: &str = "2026-01-15T16:00:00";
const ZONED_TIME_ZONE: &str = "Europe/Berlin";

/// The fourth event: a weekly series with one occurrence deleted, which EDS
/// gives to the backend as an `EXDATE` on the master. On the server it must be
/// a `recurrenceOverrides` entry that says the instance is `excluded`.
const RECURRING_SUMMARY: &str = "Weekly standup";
const RECURRING_EXCLUDED: &str = "2026-01-29T13:00:00";

/// An occurrence that the user renamed ("Edit this occurrence"). EDS gives the
/// backend a second `VEVENT` with the same `UID` and a `RECURRENCE-ID`
/// (RFC 5545 §3.8.4.4). JSCalendar states the same thing as a patch under that
/// instant in `recurrenceOverrides` (RFC 8984 §4.3.5).
const RECURRING_EDITED: &str = "2026-01-22T13:00:00";
const RECURRING_EDITED_SUMMARY: &str = "Weekly standup (demo)";

/// The `CATEGORIES` on the detached component in
/// `tests/functional/cal-client.c`. It must reach the server as a `keywords`
/// patch in that instance's override (RFC 8984 §4.3.5). The series has no tags,
/// so this set is the full difference. Sorted, because both sides keep a set in
/// that order.
const RECURRING_EDITED_KEYWORDS: [&str; 2] = ["cancelled", "offsite"];

/// The `VALARM` inside the detached component in
/// `tests/functional/cal-client.c`. It must reach the server as an `alerts`
/// patch in that instance's override. The series has no alarm, so this is the
/// deepest thing that EDS's cache must keep: a child of a child component.
///
/// The key is the alarm's RFC 9074 §4 `UID`. An alarm without it arrives under
/// a key that the mapping makes. The offset is negative, because a lost sign
/// puts the reminder after the meeting.
const RECURRING_EDITED_ALERT_KEY: &str = "k1";
const RECURRING_EDITED_ALARM_TRIGGER: &str = "-PT1H";

/// An occurrence that the user deletes through EDS. Evolution's "Delete this
/// occurrence" calls `e_cal_client_remove_object_sync` with a `RECURRENCE-ID`
/// and `E_CAL_OBJ_MOD_THIS`, and `ECalMetaBackend` makes this a save of the
/// master, not a removal. This is the fourth occurrence of the series, so it
/// is different from the excluded and the edited occurrences.
const RECURRING_REMOVED: &str = "2026-02-05T13:00:00";

/// The occurrence where the user selects "Edit this and future occurrences".
/// `ECalMetaBackend` splits the series in two. It truncates the master's rule
/// before this instance. Then it gives the backend a create for a second event
/// with a new UID. `RANGE=THISANDFUTURE` never reaches the backend, so
/// `jmap-ical` can safely ignore that parameter on read.
///
/// This is the fifth occurrence, after all three exceptions.
const RECURRING_SPLIT: &str = "2026-02-12T13:00:00";
const RECURRING_SPLIT_SUMMARY: &str = "Weekly standup (new plan)";

/// The two rules after the split, as EDS's cache states them. `COUNT=6`
/// becomes four occurrences before the split and two from it. If the backend
/// lost the truncation, the old series overlaps the new event. If the new
/// series kept `COUNT=6`, it runs past the end that the user set.
///
/// Both rules keep the `BYDAY` of the original series. This is the client side
/// of the `byDay` assertions on the server.
const SERIES_RRULE: &str = "FREQ=WEEKLY;COUNT=4;BYDAY=TH";
const SPLIT_RRULE: &str = "FREQ=WEEKLY;COUNT=2;BYDAY=TH";
const SPLIT_DTSTART: &str = "20260212T130000Z";

/// The two exclusions as EDS's cache states them, in UTC: the one the client
/// wrote with the event and the one it removed after. The test compares the
/// values, not only the count, so an exclusion on the wrong day fails.
const RECURRING_EXDATES: [&str; 2] = ["20260129T130000Z", "20260205T130000Z"];

/// The sixth event: a series in one named zone, with one occurrence moved into
/// a different zone. RFC 5545 §3.2.19 puts a zone on the property, so a
/// detached instance can have its own `TZID`. RFC 8984 §4.3.5 lets a
/// `recurrenceOverrides` patch carry `timeZone`. This shows that the backend
/// defines the second zone in the envelope and translates it.
///
/// The move is to a different clock, five hours later. An override that
/// arrives as a bare `start` (a known shipped bug) puts the occurrence at 08:00
/// Berlin time, not 08:00 New York time.
const ZONED_RECURRING_SUMMARY: &str = "Berlin standup";
const ZONED_RECURRING_START: &str = "2026-03-05T10:00:00";
const ZONED_RECURRING_DURATION: &str = "PT1H";

/// The moved occurrence. Its key is its start as the rules generate it (on the
/// series' clock), and its patch carries the new start and zone. The test
/// asserts both together, because either alone can hide an error in the other.
const ZONED_MOVED_INSTANCE: &str = "2026-03-12T10:00:00";
const ZONED_MOVED_START: &str = "2026-03-12T08:00:00";
const ZONED_MOVED_TIME_ZONE: &str = "America/New_York";

/// The `DTSTART` that EDS keeps for the moved instance. The value must be
/// exact. The `TZID` must only end with the zone name, because libical's
/// spelling of a builtin zone identifier changes between releases. If the
/// instance incorrectly uses the series' zone, it ends in `Europe/Berlin`.
const ZONED_MOVED_DTSTART: &str = "20260312T080000";

/// The event that the second leg starts from, which the server holds before
/// EDS connects. Events that EDS creates contain only what an iCalendar line
/// can state, so a round trip cannot lose anything from them.
///
/// The test asserts the title from both ends. The length is
/// `CalendarEvent::simple`'s default. The start and the zone below are part of
/// the test.
const PLACED_TITLE: &str = "Design review";
const PLACED_START: &str = "2026-04-09T10:00:00";
const PLACED_DURATION: &str = "PT1H";

/// The zone of that start: a named zone, not the `Etc/UTC` that
/// `CalendarEvent::simple` sets.
///
/// UTC is RFC 5545 §3.3.5's form 2: `jmap-ical` writes a `DTSTART` ending in
/// `Z`, which needs no zone definition. A named zone is form 3: the mapping
/// writes `DTSTART;TZID=Europe/Berlin` and **no `VTIMEZONE`** (see `dated` in
/// `jmap-ical`). It expects the consumer to resolve the IANA name from
/// libical's builtin table, although RFC 5545 §3.2.19 requires the document to
/// define each `TZID`. Only real EDS can test this, because the fixtures in
/// `jmap-ical` and `jmap-cal-sync` compare text with text.
///
/// 2026-04-09 is in CEST. A consumer that resolves the name gives 08:00 UTC.
/// A consumer that floats the value gives 10:00 on any machine, because libical
/// does not adjust a floating time when it converts it.
const PLACED_ZONE: &str = "Europe/Berlin";

/// The wall-clock start that EDS must show, and the instant that a consumer
/// must calculate. `dtstart_parts` in `tests/functional/cal-client.c` gives the
/// reason for two values. A lost zone and a converted start are different
/// errors, and each value alone can hide the other.
const PLACED_DTSTART: &str = "20260409T100000";
const PLACED_DTSTART_UTC: &str = "20260409T080000Z";

/// The location of that event, under a key that the server chose, and with a
/// member that a `LOCATION` line cannot hold.
///
/// An RFC 8984 §4.2.5 Location has a `description` and other members beside
/// its `name`. RFC 5545 §3.6.1 gives a `VEVENT` one line of text. Thus the save
/// patches `locations/<key>/name` and does not replace the property. If the save
/// replaces `locations`, the `description` disappears from the server.
const PLACED_LOCATION_KEY: &str = "srv-loc";
const PLACED_LOCATION_NAME: &str = "Room 42";
const PLACED_LOCATION_DESCRIPTION: &str = "third floor, past the lift";

/// The virtual location of the event, also under a key that the server chose
/// and with a `description` that the `CONFERENCE` line cannot hold.
///
/// This entry makes the `X-JMAP-KEY` necessary, so the leg also edits it.
/// RFC 7986 §5.11 permits more than one `CONFERENCE` line. Thus `jmap-ical`
/// reads the key from the line, and the save patches
/// `virtualLocations/<key>/uri`. If EDS drops the key, the mapping makes a new
/// key that matches no entry on the server, and the edit has no effect. A
/// `LOCATION` cannot test this: RFC 5545 §3.6.1 permits only one, so the save
/// finds the single entry without the key.
///
/// The `name` goes on the line as a `LABEL`. This shows that EDS also keeps a
/// standard parameter. A cache that keeps only parameters with a libical enum
/// keeps the `LABEL` and drops the key.
const PLACED_CONFERENCE_KEY: &str = "srv-conf";
const PLACED_CONFERENCE_URI: &str = "https://meet.example/design-review";
const PLACED_CONFERENCE_NAME: &str = "Video bridge";
const PLACED_CONFERENCE_DESCRIPTION: &str = "dial in from the lobby phone";

/// The first document that the event links to. Of the three maps that the
/// save patches, this is the one whose line libical does not keep as text.
///
/// RFC 5545 §3.8.1.1 gives `ATTACH` its own value type, and libical parses it
/// into an `icalattach`. The parameters stay beside a rebuilt value, and a
/// cache can lose them there. This leg measures whether EDS keeps the
/// `X-JMAP-KEY` on this property.
///
/// The `title` has no place on the line, so a save that replaced `links`
/// deletes it. The `contentType` and `size` show as `FMTTYPE` and `SIZE` but
/// the save never writes them back, because they describe the server's file.
/// The test asserts them after the save for this reason.
///
/// The user does **not** edit this resource. Entries go out in key order, and
/// `srv-att` sorts before [`PLACED_EDITED_ATTACH_KEY`]. Thus this is also the
/// line that a save would change if it took the first attachment.
const PLACED_ATTACH_KEY: &str = "srv-att";
const PLACED_ATTACH_HREF: &str = "https://files.example/design-review/agenda.pdf";
const PLACED_ATTACH_TITLE: &str = "Agenda, with the numbers";
const PLACED_ATTACH_MEDIA_TYPE: &str = "application/pdf";
const PLACED_ATTACH_SIZE: u64 = 20480;

/// The second document, which is the reason for the key.
///
/// With one resource per event, a lost `X-JMAP-KEY` only makes the save fail.
/// RFC 5545 §3.8.1.1 permits many `ATTACH` lines and RFC 8984 §4.2.7 a map of
/// Links. With two, a lost or swapped key makes the save change the wrong
/// document. The fixtures in `jmap-ical` and `jmap-cal-sync` write the
/// parameter by hand, so only real EDS can test this.
///
/// The user edits this one, and it is drawn second. A save that took the first
/// `ATTACH` changes [`PLACED_ATTACH_KEY`] instead, and both ends see it.
///
/// Its media type and size are different from the first entry's, so a
/// parameter on the wrong line causes a difference.
const PLACED_EDITED_ATTACH_KEY: &str = "srv-att-2";
const PLACED_EDITED_ATTACH_HREF: &str = "https://files.example/design-review/slides.odp";
const PLACED_EDITED_ATTACH_TITLE: &str = "Slides, as presented";
const PLACED_EDITED_ATTACH_MEDIA_TYPE: &str = "application/vnd.oasis.opendocument.presentation";
const PLACED_EDITED_ATTACH_SIZE: u64 = 51200;

/// The picture of the event: the same map, but a different iCalendar
/// property.
///
/// RFC 8984 §4.2.7 keeps one `links` map. iCalendar has two properties. A
/// document is RFC 5545 §3.8.1.1's `ATTACH`, and a picture is RFC 7986 §5.10's
/// `IMAGE`. `jmap-ical` uses the `icon` `rel` (RFC 8984 §1.4.11) to tell them
/// apart, so this entry has one and the two above do not.
///
/// The mapping writes `VALUE=URI` on this line because §5.10 requires that
/// parameter. Thus libical returns a URI here, not an `icalattach`. That the
/// `X-JMAP-KEY` survives EDS's cache on `ATTACH` does not show that it
/// survives on this property.
///
/// The `display` is the only Link member with its own standard parameter:
/// RFC 7986 §6.1's `DISPLAY`. Only `href` goes back today, but §6.1 tells a
/// reader to show no image when it does not know the `DISPLAY` value. Thus the
/// test measures what the round trip does to it.
///
/// §5.10 permits no `SIZE` on an `IMAGE`, so the `size` has no place on the
/// line. The save must keep it because the user never saw it. The two `ATTACH`
/// entries show their `size`, so a save that wrote back all it could name
/// keeps it there and loses it here.
const PLACED_IMAGE_KEY: &str = "srv-img";
const PLACED_IMAGE_HREF: &str = "https://files.example/design-review/whiteboard.png";
const PLACED_IMAGE_TITLE: &str = "Whiteboard, at the end";
const PLACED_IMAGE_MEDIA_TYPE: &str = "image/png";
const PLACED_IMAGE_SIZE: u64 = 8192;
/// The `display` value in each format. RFC 8984 §1.4.11 and RFC 7986 §6.1 use
/// the same words with different case, so the leg asserts each spelling at its
/// own end.
const PLACED_IMAGE_DISPLAY: &str = "graphic";
const PLACED_IMAGE_DISPLAY_ICAL: &str = "GRAPHIC";

/// The location that the user types in the appointment editor. It is a
/// different room, so a partial write matches neither value.
const REPLACED_LOCATION_NAME: &str = "Room 7";

/// The new address of the virtual location. Evolution 3.52 has no control for
/// it (see `tests/functional/cal-edit-client.c`), so this is an edit that
/// another client makes. The host and the path both change, so a partial
/// rewrite matches neither value.
const REPLACED_CONFERENCE_URI: &str = "https://call.example/rescheduled";

/// The new address of the second document. `href` is the only Link member that
/// this mapping writes back. The host and the path both change, so a partial
/// rewrite matches neither value.
///
/// The client finds the line to change by its current address, not by its
/// position or key, as a user does. See `tests/functional/cal-edit-client.c`.
const REPLACED_ATTACH_HREF: &str = "https://docs.example/design-review/slides-2.odp";

/// The new address of the picture: the same edit on the other property.
///
/// It is in the same save as the two above. `jmap-ical` reads `ATTACH` and
/// `IMAGE` into one map and writes them from one map. A mapping that paired
/// lines with entries by count (not by key) mixes them only when both are
/// present.
const REPLACED_IMAGE_HREF: &str = "https://pics.example/design-review/whiteboard-2.png";

/// The zone of the third leg's first event: an identifier that no database
/// names, which the event defines itself (RFC 8984 §4.7.2).
///
/// This is the other half of what [`PLACED_ZONE`] measures. §1.4.8 lets a
/// `TimeZoneId` be an IANA name or a custom identifier. A custom identifier
/// starts with a solidus, and the event's `timeZones` defines it. A server
/// makes such an identifier for the zone in an Exchange or Outlook invitation.
/// Only the definition says what the zone is, and `jmap-ical` writes it as a
/// `VTIMEZONE`.
///
/// The tail is `Europe-Berlin`, not `Europe/Berlin`, because libical's
/// `icaltimezone_get_builtin_timezone` finds a location in its own table. An
/// IANA tail could resolve from that table, and the leg would pass without the
/// definition.
const DEFINED_ZONE: &str = "/example.com/Europe-Berlin";

/// The zone of the third leg's second event: an identifier that no database
/// names and that the event does **not** define.
///
/// `jmap-ical` writes it as a bare `TZID` that nobody can resolve. The
/// alternatives are worse: no parameter states no zone, and UTC states the
/// wrong instant. Thus a consumer floats it. This is the documented cost of the
/// fallback in `jmap-ical`'s `dated`, and this leg measures it.
///
/// It is a second `example.com` zone, not [`DEFINED_ZONE`] again, because both
/// events travel in one refresh. With one identifier, the first event's
/// definition could resolve the second event.
const UNDEFINED_ZONE: &str = "/example.com/Somewhere-Else";

/// The titles of the third leg's two events, and their start.
///
/// Both events start at the same wall-clock time, so the difference between
/// their instants comes only from the zone definition. The date is in CEST, so
/// the defined zone's daylight observance gives an offset of two hours. A
/// reader that used only the standard observance is one hour wrong.
const DEFINED_ZONE_TITLE: &str = "Design review";
const UNDEFINED_ZONE_TITLE: &str = "Design review, nowhere in particular";
const ZONE_START: &str = "2026-04-09T10:00:00";
const ZONE_DURATION: &str = "PT1H";

/// The title that the user types over [`DEFINED_ZONE_TITLE`]. This edit does
/// not touch the zone: Evolution cannot redefine a zone, and this mapping does
/// not send a redefinition.
///
/// The first edit changes the title, not the start, so any change to the zone
/// comes from the save itself. A later save moves the start: see
/// [`ZONE_MOVED_DTSTART`].
const ZONE_RETYPED_TITLE: &str = "Design review, with the numbers";

/// The new start after the user moves the appointment. The client writes this
/// value on the existing `DTSTART` and does not change the `TZID`.
///
/// The rename shows that an edit that does not touch the clock keeps `start`.
/// This edit shows that an edit of the clock keeps the zone. A mapping that
/// sends `timeZone` each time `start` changes fails only this edit.
/// `patch::diff` reads an unnamed zone from the component on both sides. Thus
/// the guard that keeps it out of the patch runs only when the patch has a
/// date-time.
///
/// The date is in November, after the definition's `STANDARD` transition. This
/// shows that EDS's timezone store keeps the yearly rules of both observances,
/// not only one fixed offset.
const ZONE_MOVED_DTSTART: &str = "20261105T143000";

/// The new end: the same clock, 90 minutes later.
///
/// It is a `DTEND`, because Evolution's editor calls
/// `e_cal_component_set_dtend`. RFC 5545 §3.6.1 does not permit both, so the
/// client removes the `DURATION` that the mapping wrote. This is the only test
/// of the `DTEND` branch of `read_duration` through real EDS. The two
/// wall-clock times are in a zone that only the server can resolve.
const ZONE_MOVED_DTEND: &str = "20261105T160000";

/// The values that the server must hold after the move: the new wall-clock
/// start and the length between the two lines above.
///
/// `read_duration` calculates the length from two wall-clock times, but copies
/// the start. Thus a wrong subtraction gives a wrong length at the correct
/// time, which no `start` assertion finds.
const ZONE_MOVED_START: &str = "2026-11-05T14:30:00";
const ZONE_MOVED_DURATION: &str = "PT1H30M";

/// The instant of the moved appointment: 14:30 in the zone's `STANDARD`
/// observance is 13:30 UTC.
///
/// The offset is one hour, not the two hours of the April start. A zone that
/// became a fixed April offset gives 12:30Z.
const ZONE_MOVED_DTSTART_UTC: &str = "20261105T133000Z";

/// The value from libical alone: the moved wall-clock time with a `Z` added,
/// for the reason that [`FLOATING_DTSTART_UTC`] gives.
const ZONE_MOVED_FLOATING_UTC: &str = "20261105T143000Z";

/// The wall-clock start that EDS must show for both events, as an iCalendar
/// `DATE-TIME`.
const ZONE_DTSTART: &str = "20260409T100000";

/// The instant that the defined zone gives for that start. The `DAYLIGHT`
/// observance applies on 2026-04-09, so 10:00 local is 08:00 UTC. No zone
/// database holds [`DEFINED_ZONE`], so only a reader of the server's
/// definition can calculate it.
const DEFINED_ZONE_DTSTART_UTC: &str = "20260409T080000Z";

/// The value from a reader that cannot resolve the identifier. It is the
/// wall-clock time with a `Z` added. libical does not adjust a floating time
/// when it converts it, so the result is the same on all machines.
///
/// Both events give this value from `i_cal_component_get_dtstart`. EDS does not
/// return a `VTIMEZONE` with the event, so libical alone cannot resolve a custom
/// identifier. For the undefined event this is also the final value.
const FLOATING_DTSTART_UTC: &str = "20260409T100000Z";

/// The RFC 8984 §4.7.2 definition that the server sends for [`DEFINED_ZONE`]:
/// Central European Time, as a server that converts a `VTIMEZONE` states it.
///
/// It has both observances and each has a yearly rule, so a 1970 definition
/// applies in 2026. Each also has a `TZNAME`, which a reader shows.
fn defined_zone() -> serde_json::Value {
    zone_definition(DEFINED_ZONE)
}

/// The same definition under a given identifier, so that the sending leg and
/// the receiving leg expect the same text.
fn zone_definition(tzid: &str) -> serde_json::Value {
    zone_definition_offset(tzid, "+0100", "+0200", "CET", "CEST")
}

/// The zone that [`CLIENT_MOVED_ZONE_VTIMEZONE`] describes: the same rules with
/// different offsets and names.
fn moved_zone_definition() -> serde_json::Value {
    zone_definition_offset(
        CLIENT_MOVED_ZONE,
        CLIENT_MOVED_ZONE_STANDARD_OFFSET,
        CLIENT_MOVED_ZONE_DAYLIGHT_OFFSET,
        CLIENT_MOVED_ZONE_STANDARD_NAME,
        CLIENT_MOVED_ZONE_DAYLIGHT_NAME,
    )
}

/// The shared definition: two observances that change to daylight time on the
/// last Sunday of March and back on the last Sunday of October.
///
/// The `offsetFrom` of each observance is the `offsetTo` of the other. The
/// function derives them, so a caller cannot describe a zone that jumps.
fn zone_definition_offset(
    tzid: &str,
    standard_offset: &str,
    daylight_offset: &str,
    standard_name: &str,
    daylight_name: &str,
) -> serde_json::Value {
    serde_json::json!({
        "@type": "TimeZone",
        "tzId": tzid,
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-10-25T03:00:00",
            "offsetFrom": daylight_offset,
            "offsetTo": standard_offset,
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["10"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {standard_name: true},
        }],
        "daylight": [{
            "@type": "TimeZoneRule",
            "start": "1970-03-29T02:00:00",
            "offsetFrom": standard_offset,
            "offsetTo": daylight_offset,
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "yearly",
                "byMonth": ["3"],
                "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
            }],
            "names": {daylight_name: true},
        }],
    })
}

/// The zone that the **fourth** leg's client defines. Nothing else defines it.
///
/// This is the reverse of [`DEFINED_ZONE`]: here a zone that only the client
/// can name must reach the server. Evolution has such a zone after the user
/// accepts an invitation with a `VTIMEZONE` that no database holds. It can send
/// it only through `e_cal_client_add_timezone`, because EDS removes the
/// definition from the component and keeps it in the calendar's timezone
/// store. A backend that read only the component sends a `TZID` without a
/// definition, `jmap-ical`'s `maps_time_zone` refuses it, and the appointment
/// floats.
///
/// It is a third `example.com` zone, so a definition in the mock can only come
/// from the file that this leg wrote.
const CLIENT_ZONE: &str = "/example.com/Somewhere-New";

/// The title, start and length of the fourth leg's event.
///
/// The same values as the third leg's events, and in CEST for the same reason.
/// A definition without its `DAYLIGHT` observance gives an instant one hour
/// wrong.
const CLIENT_ZONE_TITLE: &str = "Design review, in a zone the client brought";
const CLIENT_ZONE_DTSTART: &str = "20260409T100000";
const CLIENT_ZONE_DURATION: &str = "PT1H";

/// The start that the server must hold, in JSCalendar form.
const CLIENT_ZONE_START: &str = "2026-04-09T10:00:00";

/// The definition that the client gives the calendar, as a `VTIMEZONE`.
///
/// It is the iCalendar form of [`zone_definition`], not a second zone. The
/// mock must hold that map, so one description for both makes the assertion
/// test the round trip and not two equal literals.
///
/// There is no `X-LIC-LOCATION`. An IANA name there lets `jmap-ical` translate
/// the zone and libical's builtin table resolve it, and the leg passes without
/// the client's definition.
const CLIENT_ZONE_VTIMEZONE: &str = "BEGIN:VTIMEZONE\r\n\
     TZID:/example.com/Somewhere-New\r\n\
     BEGIN:STANDARD\r\n\
     DTSTART:19701025T030000\r\n\
     TZOFFSETFROM:+0200\r\n\
     TZOFFSETTO:+0100\r\n\
     RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
     TZNAME:CET\r\n\
     END:STANDARD\r\n\
     BEGIN:DAYLIGHT\r\n\
     DTSTART:19700329T020000\r\n\
     TZOFFSETFROM:+0100\r\n\
     TZOFFSETTO:+0200\r\n\
     RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
     TZNAME:CEST\r\n\
     END:DAYLIGHT\r\n\
     END:VTIMEZONE\r\n";

/// The zone that the **fifth** leg moves one occurrence into. It is the second
/// zone that this leg's client defines.
///
/// The fourth leg sends a client-defined zone in a `CalendarEvent/set` create,
/// where the mapping builds the full `timeZones` map. This leg sends an update.
/// There the map on the server belongs to the user. The RFC 8620 §5.3 pointer
/// patch is the only way to add to it without overwriting what a `VTIMEZONE`
/// cannot show. Only real EDS can test this, because it needs the
/// definition in the timezone store and the series' definition on the server:
/// two saves.
///
/// It is a fourth `example.com` zone, so a definition under this identifier
/// can only come from the second file that this leg wrote.
const CLIENT_MOVED_ZONE: &str = "/example.com/Somewhere-Further";

/// The description of that zone in two forms. The `VTIMEZONE` goes to the
/// calendar, and the mock must hold the RFC 8984 §4.7.2 map.
///
/// It is [`CLIENT_ZONE_VTIMEZONE`] with different offsets and names. The
/// observance dates and yearly rules are the same. The fifth leg tests which
/// zone each definition describes, so a mix-up is visible as that one
/// difference.
///
/// No European zone has offsets of three and four hours. Thus 08:00 here is
/// 04:00 UTC, but 06:00 UTC with the series' CEST and 08:00 UTC with no zone.
const CLIENT_MOVED_ZONE_STANDARD_OFFSET: &str = "+0300";
const CLIENT_MOVED_ZONE_DAYLIGHT_OFFSET: &str = "+0400";
const CLIENT_MOVED_ZONE_STANDARD_NAME: &str = "EXT";
const CLIENT_MOVED_ZONE_DAYLIGHT_NAME: &str = "EXST";
const CLIENT_MOVED_ZONE_VTIMEZONE: &str = "BEGIN:VTIMEZONE\r\n\
     TZID:/example.com/Somewhere-Further\r\n\
     BEGIN:STANDARD\r\n\
     DTSTART:19701025T030000\r\n\
     TZOFFSETFROM:+0400\r\n\
     TZOFFSETTO:+0300\r\n\
     RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
     TZNAME:EXT\r\n\
     END:STANDARD\r\n\
     BEGIN:DAYLIGHT\r\n\
     DTSTART:19700329T020000\r\n\
     TZOFFSETFROM:+0300\r\n\
     TZOFFSETTO:+0400\r\n\
     RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
     TZNAME:EXST\r\n\
     END:DAYLIGHT\r\n\
     END:VTIMEZONE\r\n";

/// The series that the fifth leg creates, in [`CLIENT_ZONE`]: the same zone
/// and definition as the fourth leg's event. This leg adds the second zone.
///
/// Weekly with three occurrences, like [`ZONED_RECURRING_SUMMARY`]. The moved
/// occurrence is neither the first nor the last. Thus an override on the wrong
/// instance shows a wrong date, not a rejected save.
const CLIENT_SERIES_TITLE: &str = "Standup, in a zone the client brought";
const CLIENT_SERIES_DTSTART: &str = "20260409T100000";
const CLIENT_SERIES_START: &str = "2026-04-09T10:00:00";
const CLIENT_SERIES_DURATION: &str = "PT1H";
const CLIENT_SERIES_RRULE: &str = "FREQ=WEEKLY;COUNT=3";

/// The occurrence that moves: the second one. Its `RECURRENCE-ID` is a
/// wall-clock time on the series' zone. It moves two hours earlier, on the
/// clock of a zone that only this client can name.
///
/// The override key is that recurrence id in JSCalendar form. If the series
/// reaches the server floating or in UTC, the key names a different instant
/// and the override applies to no generated occurrence.
const CLIENT_MOVED_RECURRENCE_ID: &str = "20260416T100000";
const CLIENT_MOVED_INSTANCE: &str = "2026-04-16T10:00:00";
const CLIENT_MOVED_DTSTART: &str = "20260416T080000";
const CLIENT_MOVED_START: &str = "2026-04-16T08:00:00";

/// The instant of the moved wall-clock time: 08:00 in the moved zone's
/// `DAYLIGHT` observance (+0400) is 04:00 UTC.
///
/// A reader that applies the series' zone to the instance gives 06:00 UTC. A
/// reader that resolves nothing gives 08:00 UTC, because libical adds a `Z` to
/// a floating time and does not adjust it.
const CLIENT_MOVED_DTSTART_UTC: &str = "20260416T040000Z";

/// Put only a calendar in the mock's store, for a leg that creates all of its
/// events.
///
/// The calendar is necessary: the source names no calendar, so the backend
/// files each create in the account's default calendar.
fn seed_empty_calendar(server: &jmap_mock::MockServer) {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().expect("mock state lock");
    state
        .account_mut(&account_id)
        .expect("the mock's default account")
        .seed_calendar("Personal", true);
}

/// Put the third leg's two events in the mock's store (one in a zone that the
/// event defines, one in a zone that nothing defines). Return their server ids
/// in that order.
///
/// Both go in one calendar because the leg reads them in one run: one connect,
/// one refresh, one cache. Two runs share the session's XDG directories and
/// thus the meta backend's cache, which the harness starts empty.
///
/// The test seeds the store directly, not through EDS. A create carries a
/// `timeZones` entry only for a zone that a client gave the calendar (see
/// [`CLIENT_ZONE`]). This leg is about a zone that only the server has.
fn seed_zoned_events(server: &jmap_mock::MockServer) -> (Id, Id) {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().expect("mock state lock");
    let account = state
        .account_mut(&account_id)
        .expect("the mock's default account");
    let calendar = account.seed_calendar("Personal", true);

    let mut seed = |title: &str, zone: &str, definition: Option<serde_json::Value>| {
        let id = account.calendar_events.alloc_id();
        let mut event = CalendarEvent::simple(calendar.clone(), title, ZONE_START, ZONE_DURATION);
        event.id = Some(id.clone());
        event.uid = Some(format!("urn:example:event:{}", id.as_str()));
        event.time_zone = Some(zone.to_owned());
        event.time_zones = definition.map(|definition| [(zone.to_owned(), definition)].into());
        account.calendar_events.seed_with_id(id.clone(), event);
        id
    };

    let defined = seed(DEFINED_ZONE_TITLE, DEFINED_ZONE, Some(defined_zone()));
    let undefined = seed(UNDEFINED_ZONE_TITLE, UNDEFINED_ZONE, None);
    (defined, undefined)
}

/// Put the second leg's event in the mock's store and return its server id.
/// That id is also the `UID` that EDS uses as its cache key, because
/// `jmap-ical` writes it there.
///
/// The test seeds the store directly, because no iCalendar document can state
/// this shape. An event created through EDS has a location that holds only the
/// name on the line, so the save has nothing to keep or lose.
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
    // The value that a server assigns. The mock's `CalendarEvent/set` makes the
    // same value, but seeding does not use it.
    event.uid = Some(format!("urn:example:event:{}", id.as_str()));
    // A named zone, not the UTC that `simple` sets. For this `DTSTART` the
    // mapping writes no zone definition: see [`PLACED_ZONE`].
    event.time_zone = Some(PLACED_ZONE.to_owned());
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
    // Two resources, because with two a lost key changes the wrong document:
    // see [`PLACED_EDITED_ATTACH_KEY`]. Also one picture in the same map, which
    // the `icon` `rel` identifies: see [`PLACED_IMAGE_KEY`].
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
            (
                PLACED_IMAGE_KEY.to_owned(),
                serde_json::json!({
                    "@type": "Link",
                    "href": PLACED_IMAGE_HREF,
                    "rel": "icon",
                    "display": PLACED_IMAGE_DISPLAY,
                    "contentType": PLACED_IMAGE_MEDIA_TYPE,
                    "size": PLACED_IMAGE_SIZE,
                    "title": PLACED_IMAGE_TITLE,
                }),
            ),
        ]
        .into(),
    );
    account.calendar_events.seed_with_id(id.clone(), event);
    id
}

/// What the client reported for one `ATTACH` line: the address that libical
/// parsed, and the two parameters that have a libical enum.
///
/// The test compares the full line, because a media type reported on the wrong
/// resource is the failure that this leg looks for. Separate assertions could
/// each match a value from the other line.
#[derive(Debug, PartialEq, Eq)]
struct Resource<'a> {
    href: &'a str,
    media_type: &'a str,
    size: &'a str,
}

/// The line that the client reported for the server's entry `key`.
///
/// The function finds the line by its `X-JMAP-KEY`, not by its position. The
/// order of two `ATTACH` properties is `ECalMetaBackend`'s choice, and reading
/// by position lets a swap pass.
///
/// `None` if no line has that key, which is how a dropped key looks. The client
/// reports every line, so a missing key is absent or on the wrong property.
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

/// What the client reported for one `IMAGE` line: the address, the media type
/// from `FMTTYPE`, and the purpose from RFC 7986 §6.1's `DISPLAY`.
///
/// The test compares it whole for the same reason as [`Resource`]. The fields
/// are different because §5.10 permits no `SIZE` on an `IMAGE` and permits a
/// `DISPLAY`, which an `ATTACH` cannot have. The address is the string form of
/// the value, not a URL that libical parsed. See
/// `tests/functional/cal-edit-client.c` for the reason.
#[derive(Debug, PartialEq, Eq)]
struct Picture<'a> {
    href: &'a str,
    media_type: &'a str,
    display: &'a str,
}

/// The `IMAGE` line that the client reported for the server's entry `key`,
/// found by the key and not by position, as in [`resource`].
fn picture<'a>(seen: &BTreeMap<&'a str, &'a str>, prefix: &str, key: &str) -> Option<Picture<'a>> {
    let lines: usize = seen
        .get(format!("{prefix}-images").as_str())?
        .parse()
        .ok()?;
    let at = |index: usize, member: &str| {
        seen.get(format!("{prefix}-image-{index}{member}").as_str())
            .copied()
    };
    let index = (1..=lines).find(|index| at(*index, "-key") == Some(key))?;
    Some(Picture {
        href: at(index, "")?,
        media_type: at(index, "-fmttype")?,
        display: at(index, "-display")?,
    })
}

/// The keyfile from `docs/examples/jmap-mock-calendar.source`, with the mock's
/// port added. It is written here and not read from `docs/`, so a change to
/// the documented recipe makes this test fail.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::Calendar, port).to_string()
}

#[test]
fn evolution_opens_the_calendar_and_a_write_reaches_the_server() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // The keyfile has no `[Resource] Identity=`, so the backend asks the server
    // for the default calendar. Seed one with the default flag.
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

    // Check this before the exit status, for the reason in `address-book.rs`.
    // With a read-only calendar, each later failure says "Permission denied",
    // but the cause is the connect.
    //
    // `e_cal_client_connect_sync` succeeds even when the backend's
    // `connect_sync` fails, because `ECalMetaBackend` opens the calendar and
    // schedules the connect. Thus a calendar that the backend could not open
    // looks the same as one that it did not mark writable.
    //
    // If the client stopped before this point, the exit status shows the
    // earlier failure (for example, the module is missing).
    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the calendar, with {}\n{report}",
            output.status
        )
    });
    // Assert this before `readonly`, because this is the cause and `readonly`
    // is a symptom. `e_cal_meta_backend_ensure_connected_sync` sets the
    // connected status only when the backend's `connect_sync` returns TRUE.
    // Thus a calendar that the backend could not open fails here first.
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
    // The location. `locations` is a map of objects, so a missing LOCATION here
    // means that the round trip lost it.
    assert_eq!(
        seen.get("read-back-location"),
        Some(&LOCATION),
        "the event EDS handed back happens nowhere\n{report}"
    );
    // The tags. The client joins all CATEGORIES values, so a lost member gives a
    // shorter list.
    assert_eq!(
        seen.get("read-back-categories")
            .map(|values| values.split(',').collect::<Vec<_>>()),
        Some(KEYWORDS.to_vec()),
        "the event EDS handed back lost a tag\n{report}"
    );
    // The transparency. An empty string means that EDS returned no TRANSP, which
    // reads as the default OPAQUE. The next save then writes the default.
    assert_eq!(
        seen.get("read-back-transp"),
        Some(&TRANSP),
        "the event EDS handed back blocks time after all\n{report}"
    );
    // The priority. An empty string means that EDS returned no PRIORITY, which
    // reads as the default (undefined) value.
    assert_eq!(
        seen.get("read-back-priority"),
        Some(&PRIORITY),
        "the event EDS handed back lost its priority\n{report}"
    );
    // The classification. An empty string means that EDS returned no CLASS,
    // which reads as the default PUBLIC. The next save then writes PUBLIC to the
    // server.
    assert_eq!(
        seen.get("read-back-class"),
        Some(&CLASS),
        "the event EDS handed back lost its classification\n{report}"
    );
    // The reminder. An empty string means that EDS returned no VALARM, and the
    // next save deletes the reminder from the server.
    assert_eq!(
        seen.get("read-back-alarm-trigger"),
        Some(&ALARM_TRIGGER),
        "the event EDS handed back lost its reminder\n{report}"
    );
    // The edited occurrence, as EDS returns it. `ECalMetaBackend` keeps a series
    // and its detached instances as one object. If the cache lost the override
    // here, the next save removes it from the server.
    assert_eq!(
        seen.get("edited-occurrence-summary"),
        Some(&RECURRING_EDITED_SUMMARY),
        "EDS did not keep the occurrence the client edited\n{report}"
    );
    // The tags on the edited instance, which the cache must keep per component.
    // If the cache gave the series' tags instead, the user's change is lost.
    // Sorted, for the same reason as the series' tags.
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
    // The reminder on the edited instance, one level deeper. An empty string
    // means that the cache moved the alarm to the series or dropped it. The next
    // save then clears the reminder on the server.
    assert_eq!(
        seen.get("edited-occurrence-alarm-trigger"),
        Some(&RECURRING_EDITED_ALARM_TRIGGER),
        "EDS did not keep the reminder on the occurrence the client edited\n{report}"
    );
    // The removal, in the same cache. The master must have an `EXDATE` for each
    // cancelled occurrence: the one in the original event and the one that the
    // client removed through EDS. Sorted, because libical's order is not part of
    // the test.
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

    // The split, in the same cache. The test asserts both halves, because each
    // half alone can hide an error in the other. A truncated master with no new
    // event loses two weeks of the series. A new event with an untruncated master
    // doubles those days.
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

    // The occurrence that the client moved into another zone, in the same cache.
    // If the cache lost the instance's zone, the next save writes the instance
    // back on the series' clock.
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
    // the two detached instances is a row beside its series. The split added the
    // fifth event. A lower count means that the cache lost a detached instance or
    // the split's new event.
    assert_eq!(
        seen.get("events-after"),
        Some(&"8"),
        "the added events are not all in the calendar they were added to\n{report}"
    );

    // The server end: the request that the server received. The test does not
    // assert the read path, because `ECalMetaBackend` schedules its refresh, and
    // `CalendarEvent/query` can occur later. The write is synchronous.
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

    // Find each event by title, not by position: the store's keys are
    // server-assigned ids, so the order of the events has no meaning.
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
    // The location on the server: a map with one Location object, not the
    // string that the component had. Only real EDS shows that a `LOCATION` from
    // a libecal consumer survives the meta backend's cache.
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
    // The tags on the server: an RFC 8984 §4.2.9 set of both, not only the
    // first of the two properties that libical made.
    assert_eq!(
        event
            .keywords
            .as_ref()
            .map(|tags| tags.keys().map(String::as_str).collect::<Vec<_>>()),
        Some(KEYWORDS.to_vec()),
        "the tags the client wrote did not reach the server: {event:?}"
    );
    // The transparency on the server, in its JSCalendar spelling.
    assert_eq!(
        event.free_busy_status.as_deref(),
        Some(FREE_BUSY_STATUS),
        "the event reached the server blocking time, so the TRANSP the client \
         wrote was lost: {event:?}"
    );
    // The priority on the server: an integer in both formats. If the mapping
    // dropped the property, this has no value.
    assert_eq!(
        event
            .priority
            .map(|priority| priority.to_string())
            .as_deref(),
        Some(PRIORITY),
        "the priority the client wrote did not reach the server: {event:?}"
    );
    // The classification on the server, in its JSCalendar spelling.
    assert_eq!(
        event.privacy.as_deref(),
        Some(PRIVACY),
        "the classification the client wrote did not reach the server: {event:?}"
    );
    // The reminder on the server: an Alert under the key from the alarm's own
    // UID, not a key that `jmap-ical` makes. This also shows that libecal keeps
    // that UID beside the X-EVOLUTION-ALARM-UID that it adds.
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

    // The all-day event. Without `showWithoutTime` the server holds a midnight
    // appointment, and other clients show that.
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
    assert_eq!(all_day.time_zone, None, "a day has no zone: {all_day:?}");

    // The zoned event. This is the only assertion in this file that depends on
    // what the backend adds to the envelope beside the components. The `TZID`
    // is libical's own, so without the `VTIMEZONE` the mapping cannot name the
    // zone and sends none. The test asserts start and zone together, because
    // each alone can hide an error in the other.
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

    // The recurring event, whose EXDATE must become an override. The test also
    // asserts the rule, because without it an exclusion has no meaning.
    let recurring = by_title(RECURRING_SUMMARY);
    let rules = recurring
        .recurrence_rule
        .as_ref()
        .unwrap_or_else(|| panic!("the recurring event has no rule: {recurring:?}"));
    assert_eq!(rules.frequency, "weekly", "{recurring:?}");
    // Four, not the six of the original rule, because the split truncated it.
    // The count is the only value on the server that shows where the old series
    // ends. Six means that the old series overlaps the new event for two weeks.
    assert_eq!(rules.count, Some(4), "{recurring:?}");
    // The day of the week, as RFC 8984 §4.3.3 NDay objects. Without them, a
    // weekly series follows the day of its start, which changes if the start
    // moves.
    assert_eq!(
        rules.by_day.as_deref(),
        Some(&[NDay::new("th")][..]),
        "the day the series repeats on did not reach the server: {recurring:?}"
    );
    // All three exceptions share one map, so the test asserts them together. An
    // override that dropped another one undoes a deletion or a rename.
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
                    // component states all three on this instance and only the
                    // override carries them. A patch with only the title loses
                    // the user's tags or reminder.
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

    // The event from the split: an ordinary event on this side, because EDS
    // resolved `THIS_AND_FUTURE`. Its rule and start show that the series was
    // cut where the user selected. No overrides shows that the two
    // cancellations stayed with the first half of the series.
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
        .recurrence_rule
        .as_ref()
        .unwrap_or_else(|| panic!("the event the split made has no rule: {split:?}"));
    assert_eq!(split_rules.frequency, "weekly", "{split:?}");
    assert_eq!(split_rules.count, Some(2), "{split:?}");
    assert_eq!(
        split_rules.by_day.as_deref(),
        Some(&[NDay::new("th")][..]),
        "the event the split made does not repeat on the day the series did: \
         {split:?}"
    );
    assert_eq!(
        split.recurrence_overrides, None,
        "the event the split made carries exceptions from before it starts: \
         {split:?}"
    );

    // The zoned series, whose override is the only place in this file where
    // two named zones meet. The test asserts the series' zone first, because the
    // override key is a wall-clock time on it. If the series arrived floating or
    // in UTC, `2026-03-12T10:00:00` names a different instant and the override
    // applies to no generated occurrence.
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
        .recurrence_rule
        .as_ref()
        .unwrap_or_else(|| panic!("the zoned series has no rule: {zoned_recurring:?}"));
    assert_eq!(zoned_rules.frequency, "weekly", "{zoned_recurring:?}");
    assert_eq!(zoned_rules.count, Some(3), "{zoned_recurring:?}");
    // The moved occurrence, with its wall-clock start and its zone. A patch with
    // only `{"start": …}` (the mapping's behaviour before it learned `timeZone`)
    // is an error of five hours that the server cannot detect.
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
            // The document to change, identified by its current address: see
            // [`REPLACED_ATTACH_HREF`].
            PLACED_EDITED_ATTACH_HREF,
            REPLACED_ATTACH_HREF,
            // The picture to change, identified the same way.
            PLACED_IMAGE_HREF,
            REPLACED_IMAGE_HREF,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: if the
    // backend did not open the calendar, each later failure is misleading.
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

    // The start that EDS shows. This is the only document property that this
    // leg checks, and it has three values because the mapping writes a bare
    // `TZID` and no definition. The value shows the wall-clock time. The
    // identifier shows that the line still names the server's zone, which a
    // save must read back. The instant shows that a consumer resolved the
    // identifier without a `VTIMEZONE`, as [`PLACED_ZONE`] describes.
    assert_eq!(
        seen.get("read-dtstart"),
        Some(&PLACED_DTSTART),
        "the wall clock the server holds did not reach the DTSTART line\n{report}"
    );
    assert_eq!(
        seen.get("read-dtstart-tzid"),
        Some(&PLACED_ZONE),
        "EDS does not name the server's own zone on the DTSTART it handed back, \
         so the zone a save reads is not the one the event is in\n{report}"
    );
    assert_eq!(
        seen.get("read-dtstart-utc"),
        Some(&PLACED_DTSTART_UTC),
        "a libecal consumer resolved the event's start to another instant than \
         the one the server means: the TZID the mapping wrote carries no \
         VTIMEZONE, and this is where a zone nobody could resolve shows up\n{report}"
    );

    // What a libecal consumer received of the server's two locations. The
    // values show that they arrived. The keys show that a save can find the
    // entry again, which is the purpose of this leg. Fixtures in `jmap-ical` and
    // `jmap-cal-sync` write the parameter by hand, so they cannot show this.
    assert_eq!(
        seen.get("read-location"),
        Some(&PLACED_LOCATION_NAME),
        "the place the server holds did not reach the LOCATION line\n{report}"
    );
    // Reported but not yet necessary: RFC 5545 §3.6.1 permits one `LOCATION`,
    // so the save finds the single entry without the key. RFC 8984 §4.2.5
    // permits more than one location. When the mapping writes more, it will
    // read this parameter, and a change in EDS must then fail this test.
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
    // This one is necessary, and it is the reason for this leg. The mapping
    // finds the server's virtual location by the key on the line. Thus the edit
    // below reaches the entry only if EDS's cache kept this parameter.
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
    // The standard parameter beside the custom one. RFC 7986 §5.11's LABEL has
    // a libical enum, so a cache that keeps only known parameters keeps this
    // one and drops the key.
    assert_eq!(
        seen.get("read-conference-label"),
        Some(&PLACED_CONFERENCE_NAME),
        "EDS did not carry the LABEL naming the conference\n{report}"
    );

    // The two documents, read through libical's `icalattach`, not as a string
    // (see `cal-edit-client.c`). The test finds each by the `X-JMAP-KEY` on its
    // line, so the order from EDS does not matter. `ATTACH` is not a text
    // property, so a cache that kept the value and dropped the parameters is
    // visible only here. A key on the wrong line gives a wrong address, not a
    // missing entry.
    //
    // The address shows that the entry arrived. The key shows that a save can
    // find the entry, which is the only way back for a `links` entry. The
    // `FMTTYPE` and `SIZE` have libical enums, like the conference's `LABEL`.
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

    // The picture, which the same map sends to a different property. First,
    // the icon link must be an `IMAGE`, not an `ATTACH` (the `ATTACH` count
    // above is still two). Second, the key must survive on a property with a
    // different value type. The mapping writes `VALUE=URI` because RFC 7986
    // §5.10 requires it, so libical makes the value a URI, not an `icalattach`.
    assert_eq!(
        seen.get("read-images"),
        Some(&"1"),
        "EDS does not hold exactly the one IMAGE line the mapping wrote for the \
         link the server marked an icon\n{report}"
    );
    assert_eq!(
        picture(&seen, "read", PLACED_IMAGE_KEY),
        Some(Picture {
            href: PLACED_IMAGE_HREF,
            media_type: PLACED_IMAGE_MEDIA_TYPE,
            display: PLACED_IMAGE_DISPLAY_ICAL,
        }),
        "the picture the server holds did not reach an IMAGE line under the key \
         the server chose\n{report}"
    );

    // What EDS holds after the save: the new location on the same line, with
    // the key. One line, because a save that added a location gives two.
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
    // The document that the user did not edit, which is the reason for having
    // two. It must have its original address and key. A save that changed the
    // wrong entry makes this resource point to a different address.
    assert_eq!(
        resource(&seen, "read-back", PLACED_ATTACH_KEY),
        Some(Resource {
            href: PLACED_ATTACH_HREF,
            media_type: PLACED_ATTACH_MEDIA_TYPE,
            size: &PLACED_ATTACH_SIZE.to_string(),
        }),
        "the resource the user never touched did not come back as it went out\n{report}"
    );
    // The picture at its new address, still an `IMAGE` with its `DISPLAY`. A
    // save that lost the `rel` writes it back as an `ATTACH`, which fails the
    // count above and the lookup here.
    assert_eq!(
        seen.get("read-back-images"),
        Some(&"1"),
        "the save left the old picture on the event beside the new one, or moved \
         it off the IMAGE property\n{report}"
    );
    assert_eq!(
        picture(&seen, "read-back", PLACED_IMAGE_KEY),
        Some(Picture {
            href: REPLACED_IMAGE_HREF,
            media_type: PLACED_IMAGE_MEDIA_TYPE,
            display: PLACED_IMAGE_DISPLAY_ICAL,
        }),
        "the address of the picture the user re-addressed did not survive the \
         save, still under the key the server chose\n{report}"
    );

    // The start after the save, which the client did not change. The save must
    // not restate the clock (as UTC, or in the test machine's zone). If it does,
    // other clients see the appointment move after an edit to a resource.
    assert_eq!(
        seen.get("read-back-dtstart"),
        Some(&PLACED_DTSTART),
        "the save changed the wall clock the event starts at\n{report}"
    );
    assert_eq!(
        seen.get("read-back-dtstart-tzid"),
        Some(&PLACED_ZONE),
        "the save left the event's start on another zone than the server's\n{report}"
    );
    assert_eq!(
        seen.get("read-back-dtstart-utc"),
        Some(&PLACED_DTSTART_UTC),
        "the event starts at another instant after the save than before it\n{report}"
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

    // The full claim on the server side. The server's entry has its key, the
    // new name, and the `description` that the line could not hold. A save
    // that replaced `locations` and did not patch `locations/<key>/name`
    // passes all assertions above and fails this one.
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
    // The virtual location, which the round trip can break: the new address,
    // under the server's key, with the `description` and `name` still there.
    // The mapping found the entry only by the `X-JMAP-KEY`. If the cache dropped
    // the parameter, this keeps the old address.
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
    // The resources: only `href` goes back, and only for the edited entry. The
    // `title` had no place on the line, so a save that replaced `links` deletes
    // it. The `contentType` and `size` were shown but belong to the server. A
    // save that wrote them back fails here, although every client observation
    // passes.
    //
    // Both entries are in one assertion, because the pair is the claim. A save
    // that patched `links/srv-att/href` changed the wrong document, and each
    // entry alone looks like a correct edit.
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
                // The picture, changed the same way, with its other members
                // still the server's own. RFC 7986 §5.10 permits no `SIZE` on
                // an `IMAGE`, so unlike the two entries above the `size` was
                // never shown. A save that wrote back all members it could name
                // keeps it on those entries and deletes it here. The `rel` is
                // also the server's: if the save lost it, other clients see the
                // picture as an `ATTACH`.
                (
                    PLACED_IMAGE_KEY.to_owned(),
                    serde_json::json!({
                        "@type": "Link",
                        "href": REPLACED_IMAGE_HREF,
                        "rel": "icon",
                        "display": PLACED_IMAGE_DISPLAY,
                        "contentType": PLACED_IMAGE_MEDIA_TYPE,
                        "size": PLACED_IMAGE_SIZE,
                        "title": PLACED_IMAGE_TITLE,
                    }),
                ),
            ]
            .into()
        ),
        "the resource the user re-addressed did not reach the server as a patch \
         of the entry it was drawn from, leaving the other where it was: {event:?}"
    );
    // The title shows that the save patched the event and did not replace it
    // with the content of one component.
    assert_eq!(
        event.title.as_deref(),
        Some(PLACED_TITLE),
        "the save renamed the event: {event:?}"
    );
    // The start, on the server side: the seeded wall-clock time and zone. The
    // client observations show the correct instant. This shows that the save
    // did not restate it. Examples of another form are UTC and libical's
    // solidus-prefixed builtin identifier. These give a patch of `start` and
    // `timeZone` that the user did not ask for, even at the same instant.
    assert_eq!(
        (event.start.as_deref(), event.time_zone.as_deref()),
        (Some(PLACED_START), Some(PLACED_ZONE)),
        "the save restated when the event starts: {event:?}"
    );
}

/// The third calendar leg: can a libecal consumer resolve a zone that a
/// *server* named?
///
/// The second leg tests an IANA name, where `jmap-ical` sends no definition and
/// uses libical's builtin table. This leg tests RFC 8984 §1.4.8's custom
/// `TimeZoneId`, which only the event's `timeZones` defines. Two conditions are
/// necessary: `jmap-ical` must write a `VTIMEZONE` that libical accepts, and
/// `jmap-backend-cal`'s marshalling, `ECalMetaBackend` and D-Bus must keep the
/// definition available to the consumer.
///
/// Lower tests cannot measure this. `jmap-ical`'s tests compare text with text,
/// and `jmap-backend-cal`'s marshalling tests ask libical directly, which does
/// not show what EDS does after. Only a consumer that reads the event from EDS
/// tests both.
///
/// **This leg measured where the consumer must look.** EDS does *not* return
/// the `VTIMEZONE` with the event: `ECalMetaBackend` moves the definitions into
/// the calendar's timezone store, and `e_cal_client_get_object_sync` returns
/// the component alone. A consumer that asks libical to resolve the `TZID`
/// floats the start. A consumer that asks the *calendar* for the zone gets it.
/// Thus Evolution's recurrence and alarm code takes a zone-lookup callback. The
/// leg asserts both results, so a change in either is visible.
///
/// A second event uses a zone that is defined nowhere. With one wall-clock time
/// and two zones, the two instants are two hours apart. This difference shows
/// the cost of `jmap-ical`'s documented fallback, which is not a bug.
///
/// **Then the user renames the appointment and moves it.** A zone that stops
/// resolving after the first edit is not resolvable. `jmap-ical` cannot name
/// this zone in JSCalendar terms, so `patch::diff` must not send or clear
/// `timeZone`. The definition in the server's `timeZones`, which no component
/// from EDS mentions, must stay. Fixtures show that this logic exists. Only
/// this leg shows that it works after EDS, its cache and libical have each
/// handled the component.
#[test]
fn the_zone_only_the_server_can_name_means_an_instant_a_save_does_not_move() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_ZONE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let (defined, undefined) = seed_zoned_events(&server);
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/calendar-zone"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let output = session.run(
        &client,
        &[
            "read",
            "jmap-functional",
            ZONE_RETYPED_TITLE,
            ZONE_MOVED_DTSTART,
            ZONE_MOVED_DTEND,
            defined.as_str(),
            undefined.as_str(),
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: if the
    // backend did not open the calendar, each later failure is misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // Identify each event. The client receives two UIDs and reports by position.
    // If the order were reversed, the test would assert the floating instant on
    // the wrong event and pass. This assertion prevents that.
    assert_eq!(
        (seen.get("event-1-summary"), seen.get("event-2-summary")),
        (Some(&DEFINED_ZONE_TITLE), Some(&UNDEFINED_ZONE_TITLE)),
        "the events EDS handed back are not the two the mock was seeded with, in \
         the order they were asked for\n{report}"
    );

    // The event in the zone that the server defined. The wall-clock time shows
    // that the event arrived. The identifier shows that EDS kept the server's
    // zone and did not normalise it, which a save must read back.
    assert_eq!(
        seen.get("event-1-dtstart"),
        Some(&ZONE_DTSTART),
        "the wall clock the server holds did not reach the DTSTART line\n{report}"
    );
    assert_eq!(
        seen.get("event-1-dtstart-tzid"),
        Some(&DEFINED_ZONE),
        "EDS does not name the server's own zone on the DTSTART it handed back\n{report}"
    );
    // The main claim: a consumer resolved an identifier that no zone database
    // holds, to the instant that the server means. Only the event's definition
    // makes that possible, so this assertion covers the full path: `timeZones`
    // from the wire, a `VTIMEZONE` that libical accepts, the marshalling,
    // `ECalMetaBackend`, and D-Bus.
    assert_eq!(
        (seen.get("event-1-zone-known"), seen.get("event-1-zone-utc")),
        (Some(&"1"), Some(&DEFINED_ZONE_DTSTART_UTC)),
        "the calendar could not resolve the zone only the server can name, or \
         resolved it to another instant than the one the server means: the \
         definition the event carried is the only thing that says what this zone \
         is, so this is where a VTIMEZONE that was lost, or that libical would not \
         read whole, shows up\n{report}"
    );
    // Where the zone is *not* available, which is a platform fact that this leg
    // measured. EDS moves the definitions into the calendar's timezone store and
    // returns the component alone. Thus libical, which looks in the component
    // and then in its builtin table, finds nothing and floats the start. A
    // consumer that uses only libical is two hours wrong.
    assert_eq!(
        seen.get("event-1-definitions"),
        Some(&"0"),
        "EDS now hands the VTIMEZONE back beside the event, which is more than it \
         used to and means a plain libical consumer resolves the zone too\n{report}"
    );
    assert_eq!(
        seen.get("event-1-dtstart-utc"),
        Some(&FLOATING_DTSTART_UTC),
        "libical alone no longer floats a start in a zone only the server can \
         name, so what a consumer that does not ask the calendar sees has \
         changed\n{report}"
    );

    // The event in the zone that nothing defines: the same four observations,
    // with one different result. The wall-clock time and the identifier arrive
    // as above. The mapping sends the zone it received, because no parameter
    // means no zone, and UTC means the wrong instant.
    assert_eq!(
        seen.get("event-2-dtstart"),
        Some(&ZONE_DTSTART),
        "the wall clock the server holds did not reach the second DTSTART \
         line\n{report}"
    );
    assert_eq!(
        seen.get("event-2-dtstart-tzid"),
        Some(&UNDEFINED_ZONE),
        "EDS does not name the zone the server chose on the second DTSTART: a \
         zone the mapping cannot resolve is still the zone a save has to send \
         back\n{report}"
    );
    assert_eq!(
        seen.get("event-2-definitions"),
        Some(&"0"),
        "something defined a zone the server said nothing about\n{report}"
    );
    // The cost, as a value. Nothing defines this zone, so the calendar cannot
    // resolve it either, and both routes float the start. The appointment is two
    // hours from the server's instant, on all machines. This is `jmap-ical`'s
    // documented fallback. If a server or this repository improves it, these
    // assertions fail.
    assert_eq!(
        (
            seen.get("event-2-zone-known"),
            seen.get("event-2-zone-utc"),
            seen.get("event-2-dtstart-utc"),
        ),
        (
            Some(&"0"),
            Some(&FLOATING_DTSTART_UTC),
            Some(&FLOATING_DTSTART_UTC)
        ),
        "a zone that is named nowhere and defined nowhere no longer floats, which \
         is a change in what an unresolvable start costs the user\n{report}"
    );

    // After the rename: the new title, at the same time. The wall-clock time and
    // the identifier show that the save did not restate the start. The zone
    // assertions show that the zone still resolves to the server's instant. Only
    // they find a save that kept the identifier but lost its definition.
    assert_eq!(
        seen.get("saved-summary"),
        Some(&ZONE_RETYPED_TITLE),
        "the title the user retyped did not survive the save\n{report}"
    );
    assert_eq!(
        (
            seen.get("saved-dtstart"),
            seen.get("saved-dtstart-tzid"),
            seen.get("saved-zone-known"),
            seen.get("saved-zone-utc"),
        ),
        (
            Some(&ZONE_DTSTART),
            Some(&DEFINED_ZONE),
            Some(&"1"),
            Some(&DEFINED_ZONE_DTSTART_UTC),
        ),
        "renaming the appointment cost the user the zone it is in: after the save \
         EDS states another clock, or another identifier, or the calendar can no \
         longer say what that identifier means\n{report}"
    );
    // The length before the user moves the clock. The mapping writes a
    // `DURATION`, which the client then replaces with the `DTEND` that
    // Evolution's editor writes.
    assert_eq!(
        (seen.get("saved-duration"), seen.get("saved-dtend")),
        (Some(&ZONE_DURATION), Some(&"")),
        "EDS no longer states the seeded event's length as the DURATION the \
         mapping drew\n{report}"
    );

    // The user moves the appointment to November. The wall-clock time is the new
    // one and the identifier is the same, because a move does not change the
    // zone. The zone assertions are the purpose of this step: the calendar still
    // resolves the identifier, to the instant of the zone's *other* observance.
    // A save that sent or cleared the zone fails here.
    assert_eq!(
        seen.get("moved-summary"),
        Some(&ZONE_RETYPED_TITLE),
        "moving the appointment lost the title the previous save gave it\n{report}"
    );
    assert_eq!(
        (
            seen.get("moved-dtstart"),
            seen.get("moved-dtstart-tzid"),
            seen.get("moved-zone-known"),
            seen.get("moved-zone-utc"),
        ),
        (
            Some(&ZONE_MOVED_DTSTART),
            Some(&DEFINED_ZONE),
            Some(&"1"),
            Some(&ZONE_MOVED_DTSTART_UTC),
        ),
        "after the clock was moved EDS states another wall clock than the one the \
         user set, or another identifier, or the calendar resolves that identifier \
         to the wrong instant — the last of which is a zone that survived as the \
         offset April needed rather than as the rules the server sent\n{report}"
    );
    // The two platform facts from the read, checked again after a save moved the
    // clock: EDS still keeps the definition out of the component, so libical
    // alone still floats the moved start. A change here is a platform change.
    assert_eq!(
        (seen.get("moved-definitions"), seen.get("moved-dtstart-utc"),),
        (Some(&"0"), Some(&ZONE_MOVED_FLOATING_UTC)),
        "what a plain libical consumer sees for an event whose clock was moved in \
         a zone only the server can name has changed\n{report}"
    );
    // The length, again in the mapping's form. The user stated a `DTEND`, and a
    // `DURATION` of the same length returns. `read_duration`'s `DTEND` branch and
    // `vevent_of`'s `DURATION` make this round trip. The test measures it here,
    // because the two lines went through EDS's cache in a zone that libical
    // cannot resolve.
    assert_eq!(
        (seen.get("moved-duration"), seen.get("moved-dtend")),
        (Some(&ZONE_MOVED_DURATION), Some(&"")),
        "the length the user stated with two wall clocks did not come back as the \
         DURATION the mapping writes\n{report}"
    );

    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "CalendarEvent/set"),
        "the new title never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let event = state
        .account(&account_id)
        .expect("the mock's default account")
        .calendar_events
        .get(&defined)
        .unwrap_or_else(|| panic!("the save removed the event the mock was seeded with"));

    // The claim on the server side, which no EDS observation can make: the title
    // changed, and the zone properties are as seeded. The mapping reads
    // `timeZone` back from the component but cannot express it. Sending it sends
    // an iCalendar identifier as a JSCalendar `TimeZoneId`, and clearing it
    // floats the appointment. No component from EDS mentions `timeZones`, so it
    // changes only if a save overwrote data that it never had.
    assert_eq!(
        event.title.as_deref(),
        Some(ZONE_RETYPED_TITLE),
        "the server does not hold the title the user typed"
    );
    assert_eq!(
        event.time_zone.as_deref(),
        Some(DEFINED_ZONE),
        "a save moved the event out of the zone the server named for it"
    );
    assert_eq!(
        event.time_zones,
        Some([(DEFINED_ZONE.to_owned(), defined_zone())].into()),
        "a save rewrote or dropped the definition of a zone it never had in \
         its hands"
    );
    // The start, which the second save moved and the first must not have. The
    // assertions above give this meaning: a new `start` with the same
    // `timeZone` and `timeZones` is a move that the user made. A new start with
    // a changed zone is a move that nobody asked for.
    assert_eq!(
        event.start.as_deref(),
        Some(ZONE_MOVED_START),
        "the server does not hold the wall clock the user dragged the \
         appointment to"
    );
    assert_eq!(
        event.duration.as_deref(),
        Some(ZONE_MOVED_DURATION),
        "the length the user stated as a DTEND did not reach the server as the \
         length between it and the DTSTART"
    );
}

/// The fourth leg: a zone that only the **client** can name reaches the server
/// with its definition.
///
/// This is the reverse of the third leg, and unit tests cannot show it. The
/// mapping can write a custom identifier with its `timeZones` entry, and
/// `jmap-backend-cal` can get a definition from the calendar's
/// `ETimezoneCache`. This leg tests the join. When EDS calls
/// `save_component_sync`, the backend's cache must have the client's
/// `VTIMEZONE` under the identifier that the component names. This is a
/// platform fact, and a failure here is better than an appointment that
/// floats.
///
/// The client uses Evolution's route: `e_cal_client_add_timezone` and then a
/// create. EDS does not keep the definition in the component (the third leg
/// measured this on read). A backend that read only the component sees a
/// `TZID` that resolves nowhere, `maps_time_zone` refuses it, and the
/// appointment reaches the server without a zone.
///
/// The necessary assertion is the mock's. A calendar lookup after the save only
/// shows that the store still has what the client put in it. Only the server
/// can show that `timeZones` arrived.
#[test]
fn a_zone_only_the_client_can_name_reaches_the_server_with_its_definition() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_ZONE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    seed_empty_calendar(&server);
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/calendar-zone-create"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);
    let zone_file = session.write_input("client-zone.ics", CLIENT_ZONE_VTIMEZONE);

    let output = session.run(
        &client,
        &[
            "create",
            "jmap-functional",
            zone_file.to_str().expect("the zone file's path is UTF-8"),
            CLIENT_ZONE_TITLE,
            CLIENT_ZONE_DTSTART,
            CLIENT_ZONE_DURATION,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // The identifier that the client read from its file. The client does not
    // receive the identifier. If libical used another name, each `TZID` below
    // has that name, and the leg tests the wrong zone.
    assert_eq!(
        seen.get("zone-tzid"),
        Some(&CLIENT_ZONE),
        "libical named the client's zone something other than its own TZID\n{report}"
    );

    // What EDS returns for the new event. This is not the component that the
    // client wrote: EDS answers a create from what the server stored, so the
    // `TZID` is the zone as the mock returned it. If the backend cannot name the
    // zone, the appointment floats and `created-dtstart-tzid` is empty.
    //
    // The zone assertions are weaker: the client put this zone in the store, so
    // `zone-known` only shows that the store still has it. The instant shows
    // that the result is still 10:00 in the zone's DAYLIGHT observance.
    assert_eq!(
        (
            seen.get("created-summary"),
            seen.get("created-dtstart"),
            seen.get("created-dtstart-tzid"),
        ),
        (
            Some(&CLIENT_ZONE_TITLE),
            Some(&CLIENT_ZONE_DTSTART),
            Some(&CLIENT_ZONE),
        ),
        "what EDS hands back for the event the client created is not the event it \
         wrote\n{report}"
    );
    assert_eq!(
        (seen.get("created-zone-known"), seen.get("created-zone-utc")),
        (Some(&"1"), Some(&DEFINED_ZONE_DTSTART_UTC)),
        "the calendar no longer resolves the zone the client gave it, or resolves \
         it to another instant than its DAYLIGHT observance means\n{report}"
    );
    // The platform fact from the third leg, for an outgoing event: EDS keeps the
    // definition in the calendar's store and returns the component alone.
    assert_eq!(
        seen.get("created-definitions"),
        Some(&"0"),
        "EDS now hands the VTIMEZONE back beside an event the client created, \
         which is more than it used to\n{report}"
    );
    assert_eq!(
        seen.get("created-duration"),
        Some(&CLIENT_ZONE_DURATION),
        "the length the client stated did not come back as the DURATION the \
         mapping writes\n{report}"
    );

    // The claim, on the only side that can make it. `timeZone` alone passes if
    // the backend sent only the identifier, which RFC 8984 §1.4.8 forbids and a
    // server can reject. Thus the test also asserts the full definition, against
    // the description that the `VTIMEZONE` came from.
    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let events = &state
        .account(&account_id)
        .expect("the mock's default account")
        .calendar_events;
    assert_eq!(
        events.len(),
        1,
        "the server holds something other than the one event the client \
         created\n{report}"
    );
    let (_, event) = events
        .iter()
        .next()
        .expect("the one event the client created");

    assert_eq!(
        event.title.as_deref(),
        Some(CLIENT_ZONE_TITLE),
        "the server does not hold the event the client created\n{report}"
    );
    assert_eq!(
        (event.start.as_deref(), event.duration.as_deref()),
        (Some(CLIENT_ZONE_START), Some(CLIENT_ZONE_DURATION)),
        "the wall clock the client wrote did not reach the server whole\n{report}"
    );
    assert_eq!(
        event.time_zone.as_deref(),
        Some(CLIENT_ZONE),
        "the zone the client defined did not reach the server: a create that \
         cannot name the zone files the appointment floating, which is an hour or \
         two out for everybody but the user who typed it\n{report}"
    );
    assert_eq!(
        event.time_zones,
        Some([(CLIENT_ZONE.to_owned(), zone_definition(CLIENT_ZONE))].into()),
        "the identifier arrived without the definition that says what it means, \
         or with one the round trip through EDS's timezone store changed\n{report}"
    );
}

/// The fifth leg: an occurrence moves into a second zone that only the
/// **client** can name, which the save adds to the server's `timeZones` map.
///
/// The fourth leg sends such a zone in a create, where the mapping builds the
/// full map. This leg sends it in an *update*, which a user makes when they
/// move one occurrence of a meeting during travel. Then the map on the server
/// belongs to the user. RFC 8984 §4.7.2 definitions can have `aliases`, `url`
/// and `validUntil`, which a `VTIMEZONE` cannot hold, so a save that restated
/// the property deletes them. Thus `jmap-cal-sync`'s `diff_time_zones` adds one
/// entry through an RFC 8620 §5.3 pointer, and this leg runs it against a
/// server's map.
///
/// One save has three joins, and no unit test can reach them. The moved zone's
/// definition must be in the timezone store when EDS calls
/// `save_component_sync` for `E_CAL_OBJ_MOD_THIS`. This call is different from
/// the create, and EDS gives the backend the master *and* the detached
/// instance. The instance's identifier must reach `recurrenceOverrides`. The
/// series' definition from the create must stay unchanged on the server,
/// because the patch must add and not replace.
///
/// The necessary assertion is again the mock's. A calendar lookup only shows
/// that the store still has what the client put in it. Only the server can
/// show the second entry beside the first.
#[test]
fn an_occurrence_moved_into_a_client_named_zone_reaches_the_server_defined() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_ZONE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    seed_empty_calendar(&server);
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/calendar-zone-series"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);
    let zone_file = session.write_input("series-zone.ics", CLIENT_ZONE_VTIMEZONE);
    let moved_zone_file = session.write_input("moved-zone.ics", CLIENT_MOVED_ZONE_VTIMEZONE);

    let output = session.run(
        &client,
        &[
            "series",
            "jmap-functional",
            zone_file.to_str().expect("the zone file's path is UTF-8"),
            moved_zone_file
                .to_str()
                .expect("the moved zone file's path is UTF-8"),
            CLIENT_SERIES_TITLE,
            CLIENT_SERIES_DTSTART,
            CLIENT_SERIES_DURATION,
            CLIENT_SERIES_RRULE,
            CLIENT_MOVED_RECURRENCE_ID,
            CLIENT_MOVED_DTSTART,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // The identifiers that libical read from the two files, reported for the
    // reason that the fourth leg gives. The leg tests two different zones, so
    // the test checks both.
    assert_eq!(
        (seen.get("zone-tzid"), seen.get("moved-zone-tzid")),
        (Some(&CLIENT_ZONE), Some(&CLIENT_MOVED_ZONE)),
        "libical named one of the client's zones something other than its own \
         TZID\n{report}"
    );

    // EDS returned the instance, which gives the assertions below their
    // meaning. `e_cal_client_get_object_sync` returns the *master* for an
    // instance with no detached copy, and here the master has the same UID and
    // summary and the original clock.
    assert_eq!(
        seen.get("occurrence-detached"),
        Some(&"1"),
        "EDS answered with the series rather than with the occurrence the client \
         moved, so the move reached nothing\n{report}"
    );

    // What EDS kept of the moved occurrence. EDS answers the read from what the
    // backend stored after the save, so this is the instance as the mock
    // returned it. If the save could not name the moved zone, this `TZID` is
    // [`CLIENT_ZONE`], not empty: that failure looks most like success.
    assert_eq!(
        (
            seen.get("occurrence-dtstart"),
            seen.get("occurrence-dtstart-tzid"),
        ),
        (Some(&CLIENT_MOVED_DTSTART), Some(&CLIENT_MOVED_ZONE)),
        "the occurrence EDS hands back is not on the clock, or not in the zone, \
         the client moved it to\n{report}"
    );
    assert_eq!(
        (
            seen.get("occurrence-zone-known"),
            seen.get("occurrence-zone-utc"),
        ),
        (Some(&"1"), Some(&CLIENT_MOVED_DTSTART_UTC)),
        "the calendar no longer resolves the second zone the client gave it, or \
         resolves it to another instant than its DAYLIGHT observance means — the \
         series' own zone would land two hours later\n{report}"
    );
    assert_eq!(
        seen.get("occurrence-duration"),
        Some(&CLIENT_SERIES_DURATION),
        "the moved occurrence came back some other length than the series'\n{report}"
    );

    // The claim, on the only side that can make it.
    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let events = &state
        .account(&account_id)
        .expect("the mock's default account")
        .calendar_events;
    assert_eq!(
        events.len(),
        1,
        "the server holds something other than the one series the client \
         created\n{report}"
    );
    let (_, event) = events
        .iter()
        .next()
        .expect("the one series the client created");

    assert_eq!(
        event.title.as_deref(),
        Some(CLIENT_SERIES_TITLE),
        "the server does not hold the series the client created\n{report}"
    );
    // Assert the series' clock first, because the override key is a wall-clock
    // time on it. If the series arrived floating or in UTC, the key names an
    // instant that the rules never generated.
    assert_eq!(
        (
            event.start.as_deref(),
            event.time_zone.as_deref(),
            event.duration.as_deref(),
        ),
        (
            Some(CLIENT_SERIES_START),
            Some(CLIENT_ZONE),
            Some(CLIENT_SERIES_DURATION),
        ),
        "the save of one occurrence moved the series itself\n{report}"
    );
    assert_eq!(
        event.recurrence_overrides,
        Some(
            [(
                CLIENT_MOVED_INSTANCE.to_owned(),
                serde_json::json!({
                    "start": CLIENT_MOVED_START,
                    "timeZone": CLIENT_MOVED_ZONE,
                }),
            )]
            .into()
        ),
        "the occurrence the user moved did not reach the server on the zone they \
         moved it to\n{report}"
    );
    // The purpose of the leg: both definitions, with the series' definition
    // unchanged beside the new one. A patch that restated the property passes an
    // assertion on the new entry alone and loses the old one, which the §5.3
    // pointer prevents. Thus the test asserts the full map.
    assert_eq!(
        event.time_zones,
        Some(
            [
                (CLIENT_ZONE.to_owned(), zone_definition(CLIENT_ZONE)),
                (CLIENT_MOVED_ZONE.to_owned(), moved_zone_definition()),
            ]
            .into()
        ),
        "the zone the occurrence moved into arrived without the definition that \
         says what it means, or the save overwrote the definition the series was \
         created with\n{report}"
    );
}
