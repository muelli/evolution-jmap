<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Meeting invitations: who sends them

A meeting's guests are synced to the server as `participants` (see
`ICAL-MAPPING.md` section 3.8). This file answers the next question: does the
server send the invitations, cancellations and replies, or does Evolution? It
records what the specifications say, what the servers we know actually do,
how Evolution decides whether to mail guests itself, and the rule this
backend follows.

## 1. What the draft says

References are to draft-ietf-jmap-calendars-33 (October 2026); sections 1.5.1
and 5.9.2 read the same in -28 and -32.

- **No discovery signal.** Section 1.5.1: the `urn:ietf:params:jmap:calendars`
  value in the session `capabilities` "is an empty object", and the account
  value holds only `maxCalendarsPerEvent`, `minDateTime`, `maxDateTime`,
  `maxExpandedQueryDuration`, `maxParticipantsPerEvent` and
  `mayCreateCalendar`. Nothing says whether the server can send scheduling
  messages. No other capability in the draft does either.
- **Asking is per call.** Section 5.9: `CalendarEvent/set` takes
  `sendSchedulingMessages: Boolean (default: false)`. "If true then any changes
  to scheduled events will be sent to all the participants (if the server is
  the origin of the event) or back to the origin (otherwise)".
- **Once asked, the server must deliver or refuse.** Section 5.9.2: "If
  `sendSchedulingMessages` is true, the server MUST send appropriate iTIP
  [RFC5546] scheduling messages after successfully creating, updating or
  destroying a calendar event." And: "If any of the recipients do not have a
  calendar address the server can send to, the server MUST reject the change
  with a noSupportedScheduleMethods SetError." The refusal is per record and the
  change is not applied.
- **Who is the origin.** Section 5, `isOrigin` (server-set) is true if and only
  if `organizerCalendarAddress` is null or "the account will receive messages
  sent to the `organizerCalendarAddress` URI". An origin server sends REQUEST
  and CANCEL to guests; otherwise it sends REPLY to the organizer.
- **A server that never heard of the argument.** RFC 8620 section 3.9: "In case
  of invalid arguments (unknown/insufficient/wrong type for data, etc.), the
  method MUST return an `invalidArguments` error and terminate." So a server
  that does not implement `sendSchedulingMessages` should refuse the whole
  call, not ignore the argument.

So a conforming server cannot silently drop the request: it sends, refuses
the record (`noSupportedScheduleMethods`), or refuses the call
(`invalidArguments`). But the draft gives no way to learn in advance which of
these will happen, and a server that is lax about unknown arguments can drop
the request with no error at all (section 2 shows Stalwart is lax in exactly
this way).

## 2. What the servers do

| | Stalwart 0.16 | Fastmail | `jmap-mockd` |
|---|---|---|---|
| Scheduling signal in the session | none (empty objects) | none (empty objects) | none |
| `sendSchedulingMessages: true`, reachable guest | sends: the event lands in the guest's calendar, plus a notification and an iMIP "Invitation:" mail | sends: an iMIP invitation (HTML with RSVP links and `invite.ics`) within ~40 s; a destroy sends a cancellation | records the iTIP message in a per-account scheduling outbox (no transport) |
| Guest with no address the server can send to | `noSupportedScheduleMethods`, nothing stored | not probed | `noSupportedScheduleMethods`, nothing stored |
| Unknown `/set` argument | ignored, change applied (against RFC 8620 section 3.9) | not probed | not probed |
| Non-scheduling mode | n/a | n/a | none yet; needed to test the fallback |

Stalwart's two rows marked "nothing stored" and "ignored" are pinned by the
live tests
`scheduling_for_an_undeliverable_guest_is_refused_and_nothing_is_stored` and
`an_unknown_set_argument_is_ignored_rather_than_refused` in
`rust/crates/jmap-client/tests/live_server.rs`. Fastmail's row comes from the
maintainer's own probe against a test calendar. The mock's row is
`rust/crates/jmap-client/tests/calendar_scheduling.rs`.

**Probing safely.** A probe must never mail a real person. The undeliverable
guest in the live test is a `urn:uuid:` URI, which no transport delivers to,
so a server that does try to schedule has nowhere to send. A probe that has to
see a real delivery uses a second throwaway account on the same test server.
Never probe with a `mailto:` on a domain you do not control, including
`.invalid` and `.test` names, since a server may still queue mail for them.

## 3. How Evolution decides whether to mail guests

EDS 3.52 and Evolution 3.52 (GNOME 46) sources.

- `e-cal-util.h` defines `E_CAL_STATIC_CAPABILITY_CREATE_MESSAGES`
  (`"create-messages"`) and `E_CAL_STATIC_CAPABILITY_SAVE_SCHEDULES`
  (`"save-schedules"`). `E_CAL_STATIC_CAPABILITY_ITIP_SUPPRESS_ON_REMOVE_SUPPORTED`
  is documented there as: set when the backend supports `save-schedules` "and
  it can suppress iTip message on component removal ... The backend checks
  `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE` flag when these capabilities are
  present and sends or does not send iTip message accordingly."
- Evolution's `src/calendar/gui/itip-utils.c`, `itip_send_component_begin`,
  first calls `comp_server_send_sync`, which is
  `e_cal_client_send_objects_sync` ("Give the server a chance to manipulate
  the comp"). Then, for anything but PUBLISH, if the client has
  `create-messages` it marks the send complete and stops: Evolution composes
  no mail. Without the capability it goes on to compose one, and
  `comp_to_list` skips every attendee whose address is in the user list the
  backend's `send_objects` returned.
- `ECalMetaBackend`'s own `send_objects_sync` (`e-cal-meta-backend.c`) returns
  no users and the component unchanged, so today Evolution mails every guest.
- The precedent is EDS's CalDAV backend (`e-cal-backend-caldav.c`). It
  advertises `create-messages`, `save-schedules` and
  `itip-suppress-on-remove-supported` only when
  `e_source_webdav_get_calendar_auto_schedule` is set (Evolution's "Server
  handles meeting invitations" option) and the server's DAV header names
  `calendar-schedule` or `calendar-auto-schedule` (or the backend has not
  seen the header yet). When the server's support changes it calls
  `e_cal_backend_notify_property_changed` for the capabilities. On save it
  turns scheduling off for one change when the operation flags carry
  `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`.
  `ECalMetaBackend` keeps those flags with an offline change and replays them
  on upload, so the same rule holds for edits made offline.

## 4. The rule

1. **Default: the server schedules.** Every JMAP calendar server we know
   schedules, the draft makes it mandatory once asked, and the draft offers no
   way to ask in advance. The backend therefore assumes scheduling unless told
   otherwise.
2. **The switch is the user's, per account.** Like CalDAV's "Server handles
   meeting invitations", a source setting decides, on by default. Because a
   lax server can drop `sendSchedulingMessages` without an error (section 2),
   no response can prove that guests were reached; the switch is the only
   reliable signal, and the user guide must say to turn it off for a server
   that does not send invitations.
3. **Switch on.** The backend advertises `create-messages`, `save-schedules`
   and `itip-suppress-on-remove-supported`, so Evolution does not mail guests
   itself. Creates, edits and deletes of events with participants send
   `sendSchedulingMessages: true`, except when the operation flags carry
   `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE` (the user chose not to send).
   The server picks REQUEST/CANCEL or REPLY from `isOrigin`, so the same flag
   serves the organizer's edits and a guest's accept or decline.
4. **Switch off.** No capability is advertised, `sendSchedulingMessages` is
   never sent, guests are still synced as data, and Evolution mails them as it
   does today.
5. **Refusals are errors, not fallbacks.** By the time the server answers,
   Evolution has already been told the backend sends, so a quiet retry without
   scheduling would leave guests uninvited and the user unaware.
   `noSupportedScheduleMethods` fails the save with a message naming the
   address the server cannot reach; the change was not applied (section 5.9.2).
   `invalidArguments` on a call that carried `sendSchedulingMessages` means
   the server does not implement it: the save fails the same way and tells the
   user to turn the switch off.
