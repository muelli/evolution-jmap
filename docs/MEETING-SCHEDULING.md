<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Meeting invitations: who sends them

The sync layer syncs the guests of a meeting to the server as `participants`
(see `ICAL-MAPPING.md` section 3.8). This file answers the next question: does
the server or Evolution send the invitations, cancellations and replies? The
file records these items:

- what the specifications say
- what the servers that we know do
- how Evolution decides whether it mails the guests itself
- the rule that this backend follows

## 1. What the draft says

The references are to draft-ietf-jmap-calendars-33.

- **No discovery signal.** In section 1.5.1, the value of
  `urn:ietf:params:jmap:calendars` in the session `capabilities` "is an empty
  object". The account value holds only `maxCalendarsPerEvent`, `minDateTime`,
  `maxDateTime`, `maxExpandedQueryDuration`, `maxParticipantsPerEvent`,
  `mayCreateCalendar` and `maxAvailabilityDuration`. Nothing says whether the
  server can send scheduling messages. No other capability in the draft says
  it.
- **The client asks in each call.** In section 5.9, `CalendarEvent/set` takes
  `sendSchedulingMessages: Boolean (default: false)`. "If true then any changes
  to scheduled events will be sent to all the participants (if the server is
  the origin of the event) or back to the origin (otherwise)".
- **After the client asks, the server must deliver or refuse.** Section 5.9.2
  says: "If `sendSchedulingMessages` is true, the server MUST send appropriate
  iTIP [RFC5546] scheduling messages after successfully creating, updating or
  destroying a calendar event." It also says: "If any of the recipients do not
  have a calendar address the server can send to, the server MUST reject the
  change with a noSupportedScheduleMethods SetError." The refusal applies to
  one record. The server does not apply the change.
- **Who is the origin.** In section 5, `isOrigin` (set by the server) is true
  if and only if `organizerCalendarAddress` is null or "the account will
  receive messages sent to the `organizerCalendarAddress` URI". An origin
  server sends REQUEST and CANCEL to the guests. Otherwise it sends REPLY to
  the organizer.
- **A server that does not know the argument.** RFC 8620 section 3.9 says: "In
  case of invalid arguments (unknown/insufficient/wrong type for data, etc.),
  the method MUST return an `invalidArguments` error and terminate." A server
  that does not implement `sendSchedulingMessages` should therefore refuse the
  whole call. It should not ignore the argument.

A conforming server cannot silently drop the request. It does one of three
things:

- It sends the messages.
- It refuses the record (`noSupportedScheduleMethods`).
- It refuses the call (`invalidArguments`).

But the draft gives no way to learn in advance which of these will happen. A
server that is lax about unknown arguments can drop the request with no error
at all. Section 2 shows that Stalwart is lax in exactly this way.

## 2. What the servers do

| | Stalwart | Fastmail | `jmap-mockd` |
|---|---|---|---|
| Scheduling signal in the session | none (empty objects) | none (empty objects) | none |
| `sendSchedulingMessages: true`, reachable guest | sends: the event lands in the calendar of the guest once, and a destroy reaches the guest as a cancellation; a manual probe also saw a notification and an iMIP "Invitation:" mail | sends: an iMIP invitation (HTML with RSVP links and `invite.ics`) within about 40 s; a destroy sends a cancellation | records the iTIP message in a scheduling outbox for each account (no transport) |
| Guest with no address that the server can send to | `noSupportedScheduleMethods`, nothing stored | not probed | `noSupportedScheduleMethods`, nothing stored (only `mailto:` counts as deliverable, as on Stalwart) |
| Unknown `/set` argument | ignored, change applied (against RFC 8620 section 3.9) | not probed | applied normally; in `without_scheduling` mode, `sendSchedulingMessages` is `invalidArguments` and nothing is applied |
| Non-scheduling mode | n/a | n/a | `MockServer::builder().without_scheduling()` |

These tests pin the Stalwart column:

- The delivery row:
  `with_the_switch_on_the_server_delivers_the_meeting_and_its_cancellation_to_the_guest`
  in `rust/crates/jmap-cal-sync/tests/live_server_scheduling.rs`.
- The two rows "nothing stored" and "ignored": the live tests
  `scheduling_for_an_undeliverable_guest_is_refused_and_nothing_is_stored` and
  `an_unknown_set_argument_is_ignored_rather_than_refused` in
  `rust/crates/jmap-client/tests/live_server.rs`.

The Fastmail column comes from a manual probe against a test calendar. No test
pins it. The column of the mock server is
`rust/crates/jmap-client/tests/calendar_scheduling.rs`.

**Probing safely.** A probe must never mail a real person. The undeliverable
guest in the live test is a `urn:uuid:` URI. No transport delivers to such a
URI. A server that tries to schedule therefore has nowhere to send. If a probe
must see a real delivery, use a second temporary account on the same test
server. Never probe with a `mailto:` address on a domain that you do not
control. This includes `.invalid` and `.test` names, because a server can
still queue mail for them.

## 3. How Evolution decides whether to mail guests

These items come from the sources of EDS 3.52 and Evolution 3.52 (GNOME 46).

- `e-cal-util.h` defines `E_CAL_STATIC_CAPABILITY_CREATE_MESSAGES`
  (`"create-messages"`) and `E_CAL_STATIC_CAPABILITY_SAVE_SCHEDULES`
  (`"save-schedules"`). The file documents
  `E_CAL_STATIC_CAPABILITY_ITIP_SUPPRESS_ON_REMOVE_SUPPORTED` in this way: the
  backend sets it if the backend supports `save-schedules` "and it can
  suppress iTip message on component removal ... The backend checks
  `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE` flag when these capabilities are
  present and sends or does not send iTip message accordingly."
- `itip_send_component_begin` in `src/calendar/gui/itip-utils.c` of Evolution
  does these steps:
  1. It first calls `comp_server_send_sync`. This function is
     `e_cal_client_send_objects_sync` ("Give the server a chance to
     manipulate the comp").
  2. For every method except PUBLISH, if the client has `create-messages`, it
     marks the send as complete and stops. Evolution composes no mail.
  3. If the client does not have the capability, Evolution composes a mail.
     `comp_to_list` skips every attendee whose address is in the user list
     that `send_objects` of the backend returned.
- `send_objects_sync` of `ECalMetaBackend` (`e-cal-meta-backend.c`) returns no
  users and the unchanged component. Today, Evolution therefore mails every
  guest.
- The precedent is the CalDAV backend of EDS (`e-cal-backend-caldav.c`). It
  advertises `create-messages`, `save-schedules` and
  `itip-suppress-on-remove-supported` only if both conditions are true:
  - `e_source_webdav_get_calendar_auto_schedule` is set (the option "Server
    handles meeting invitations" of Evolution).
  - The DAV header of the server names `calendar-schedule` or
    `calendar-auto-schedule` (or the backend has not seen the header yet).

  If the support of the server changes, the backend calls
  `e_cal_backend_notify_property_changed` for the capabilities. On save, the
  backend turns scheduling off for one change if the operation flags carry
  `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`. `ECalMetaBackend` keeps those
  flags with an offline change. It replays them on upload. The same rule
  therefore holds for edits that the user made offline.

## 4. The rule

1. **The server always schedules.** Every JMAP calendar server that we know
   schedules. The draft makes scheduling mandatory after the client asks
   (section 5.9.2). Evolution never mails the guests itself, and there is no
   setting that makes it do so.
2. **An old setting has no effect.** A calendar source can still carry
   `[WebDAV Backend] CalendarAutoSchedule=false` from an earlier version. The
   backend ignores it.
3. **The backend advertises the scheduling capabilities.** These are
   `create-messages`, `save-schedules` and
   `itip-suppress-on-remove-supported`. Evolution therefore does not mail the
   guests itself.
   - Creates, edits and deletes send `sendSchedulingMessages: true`.
   - The exception: the operation flags carry
     `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`. This means that the user
     chose not to send.
   - A dismissed reminder never sends the flag. The dismissal changes only
     the alert of the user, so the guests must not get a message.
   - The flag goes out whether or not the event has participants. An event
     without participants is not a scheduled event. The server then has nobody
     to send to. A delete does not need to load the event first to find out.
   - The server selects REQUEST/CANCEL or REPLY from `isOrigin`. The same flag
     therefore serves the edits of the organizer and the accept or decline of
     a guest.
   - A guest answers in the event editor or from the invitation mail. The
     editor edits the copy that the server delivered.
   - The invitation view gives the mailed invitation to `receive_objects` with
     the `UID` of the meeting. The local cache uses JMAP ids as keys, so it does
     not know this `UID`, and `ECalMetaBackend` creates a new event.
   - The server already delivered the meeting into the calendar. Stalwart
     refuses a second event with the same `uid`. Thus, a create with a `UID`
     that this calendar already holds goes into that event. The answer reaches
     the organizer the same way as from the editor.
   - A component from `receive_objects` (an accepted invitation or an import)
     comes from another person. The backend marks it as received. The change
     cannot set the organizer. It cannot change the guests or the organizer of
     a meeting that the user organizes, or delete that meeting. The save fails
     with a permission error. While the backend is offline, `receive_objects`
     fails.
   - Within that, two cases never ask the server to schedule, whatever the
     opflags say. A removal always comes from a received CANCEL (the only
     iTip method `receive_objects` turns into a delete), and RFC 5546 defines
     no response to a CANCEL. A create or edit whose payload is not a real
     invitation (`METHOD` absent or `PUBLISH`, RFC 5546 1.4: a plain import or
     a backup restore) is not something the user answered, so it must not
     silently mail a foreign organizer a reply either. Only a create or edit
     from a real invitation (`METHOD` `REQUEST` or `REPLY`) schedules the
     user's own answer to an organizer that is not the user.
4. **Refusals are errors, not fallbacks.** When the server answers, Evolution
   already knows that the backend sends. A quiet retry without scheduling
   would leave the guests uninvited. The user would not know.
   - `noSupportedScheduleMethods` fails the save. The message names the
     address that the server cannot reach. The server did not apply the change
     (section 5.9.2).
   - `invalidArguments` on a call that carried `sendSchedulingMessages` means
     that the server does not implement it. The save fails in the same way. The
     message tells the user to save the change without sending.

## 5. Where this lives

- `jmap-client`: `event_create_with_scheduling`, `event_update_with_scheduling`
  and `event_destroy_with_scheduling` send the argument only if the caller
  asks.
- `jmap-cal-sync`:
  - `CalSync::with_server_scheduling` lets the sync layer ask at all. It is
    off by default. The calendar backend turns it on at connect. The backend
    decides for each change.
  - `save_component_with` and `remove_component_with` take `Itip::Suppress`
    for `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`.
  - They take `Itip::Received` for a change from `receive_objects`. The
    calendar backend sets a mark in the operation flags that
    `receive_objects_sync` gives to EDS, and the save reads it. EDS keeps
    these flags with a change that it queues offline, so the mark stays when
    EDS uploads the change later. The account's `ParticipantIdentity`
    addresses tell if the user is the organizer. `remove_component_with`
    ignores `Itip::Received`'s `reply` field entirely: a removal it describes
    is always a received CANCEL, which never earns a reply. For a create or
    edit, `jmap_cal_sync::is_import` reads `receive_objects_sync`'s raw
    payload for a top-level `METHOD`; the calendar backend sets a second mark
    when it is absent or `PUBLISH`, and `Itip::Received`'s `reply` is forced
    false whenever that mark is set, whatever the opflags otherwise say.
  - A call asked the server to schedule, and the server refused it. The
    refusal comes back as `SyncError::Unscheduled`:
    - `Unreachable` for `noSupportedScheduleMethods`. It names the
      non-`mailto:` addresses of the event. The server does not say which
      guest it could not reach. A delete names nobody, because it does not
      load the event.
    - `NotImplemented` for `invalidArguments`.
  - A create first asks the server for an event in this calendar with the same
    `uid`. If there is one, the create patches it (the invitation-mail answer
    in rule 3).
  - Tests: `tests/scheduling.rs` (mock) and `tests/live_server_scheduling.rs`
    (two accounts on one real server). With server scheduling on, the calendar
    of the guest gets the meeting exactly once and then its cancellation.
    With server scheduling off in the sync layer, it gets nothing. When a guest accepts, from the editor or
    from the invitation mail, the calendar keeps one copy and the answer
    reaches the organizer.
- `jmap-backend-cal`: `scheduling.rs` holds the rule.
  - `get_backend_property` of the backend adds the three capabilities to the
    answer of the meta backend.
  - `save_component_sync` and `remove_component_sync` map their operation
    flags to `Itip`.
  - `ops::to_gerror` turns the two refusals into translated messages. They say
    that the backend did not save the change:
    - `Unreachable` becomes `E_CLIENT_ERROR_INVALID_ARG` (the meeting must
      change).
    - `NotImplemented` becomes `E_CLIENT_ERROR_NOT_SUPPORTED`. The message
      tells the user to save the change without sending.
  - Tests: `tests/scheduling.rs` and `tests/ops.rs` (in process). Also the
    `calendar-scheduling` leg of `jmap-functional`. It uses a real
    `evolution-calendar-factory` and an `ECalClient`. It checks the
    capabilities for a new source and for a source with an old
    `CalendarAutoSchedule=false`. It also checks which creates and removes
    reach the scheduling outbox of the mock server.
