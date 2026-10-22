// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! Calendar synchronisation, in the shape that `ECalMetaBackend` requires.
//!
//! One EDS calendar source is one JMAP calendar, and this crate contains all of
//! its synchronisation logic. It lists the events, renders an event as an
//! iCalendar object, and finds the changes since a state string. It also
//! converts an edit made in Evolution into a `CalendarEvent/set`. Each entry
//! point agrees with one vfunc: [`CalSync::list_existing`] with
//! `list_existing_sync`, [`CalSync::save_component`] with
//! `save_component_sync`, and so on.
//!
//! This crate intentionally has no knowledge of GObject or the Evolution
//! headers. Thus the tests of the important half of the backend can run against
//! `jmap-mockd` on any machine. The subclass above this crate does only
//! lifecycle and marshalling. This crate is the calendar counterpart of
//! `jmap-book-sync`, and has almost the same shape, so that the two crates show
//! one design.

pub mod color;
pub mod error;
pub mod freebusy;
pub mod patch;

use std::collections::BTreeMap;

use jmap_client::{ChangeSet, Client};
use jmap_ical::{
    event_to_ical, ical_to_event, maps_recurrence_rule, maps_time_zone, prune_time_zones,
    unstateable_until,
};
use jmap_proto::calendars::{
    CalendarEvent, CalendarEventQueryFilter, RecurrenceRule, calendar_event_set_error,
};
use jmap_proto::{Id, State};
use serde_json::{Map, Value};

pub use error::{SyncError, Unscheduled, Unsendable};
pub use freebusy::FreeBusy;

/// One event in the form that the meta backend uses: an identifier, a change
/// token and the object.
///
/// This is the payload of an `ECalMetaBackendInfo`. `uid` is the JMAP id. The
/// crate docs of `jmap-ical` tell why EDS uses it as the key, and not the
/// JSCalendar `uid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentInfo {
    pub uid: String,
    pub revision: String,
    pub icalendar: String,
}

/// The copy of an event that a local edit started from.
///
/// EDS keeps the `extra` string of a cached component through an offline edit.
/// Store [`ComponentInfo::icalendar`] there, and give it back here on the save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base<'a> {
    /// The iCalendar object of the event as the user saw it before the edit.
    pub icalendar: &'a str,
    /// What to do with a property that the server and the user both changed.
    pub on_conflict: OnConflict,
}

/// What a save does with a property that the user and a different client both
/// changed after the base copy (`EConflictResolution`). A property that only one
/// side changed is not a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnConflict {
    /// Send the user's value (`E_CONFLICT_RESOLUTION_KEEP_LOCAL`).
    #[default]
    KeepLocal,
    /// Keep the server's value, and send the other edits (`E_CONFLICT_RESOLUTION_KEEP_SERVER`).
    KeepServer,
    /// Write nothing and fail with [`SyncError::OutOfSync`] (`E_CONFLICT_RESOLUTION_FAIL`).
    Fail,
}

/// What changed in the calendar since a given state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// The state to pass to the next [`CalSync::get_changes`].
    pub new_state: State,
    /// Events that were created or modified, already rendered.
    pub changed: Vec<ComponentInfo>,
    /// Identifiers of events that are no longer in *this* calendar: destroyed
    /// events and events moved to a different calendar.
    pub removed: Vec<String>,
}

/// Whether one change can tell the meeting guests about the change.
///
/// This is the per-change half of the scheduling rule in
/// `docs/MEETING-SCHEDULING.md`. [`Itip::Suppress`] is the EDS flag
/// `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`, which EDS sets when the user
/// chooses not to send an update. It has an effect only while the account
/// switch ([`CalSync::with_server_scheduling`]) is on. While the switch is off,
/// the server never gets the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Itip {
    /// The server sends the messages that the change requires.
    #[default]
    Send,
    /// Ask the server to send nothing for this change.
    Suppress,
    /// The change comes from a received iTIP message or an import, not from the user.
    ///
    /// The server sends at most the user's answer to another organizer, and only
    /// when `reply` is true. See [`CalSync::save_component_with`].
    Received {
        /// False when EDS set `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`.
        reply: bool,
    },
}

/// The patch of an edit from `base`, with the conflicts resolved as `base` asks.
fn patch_since(
    base: Base<'_>,
    current: &CalendarEvent,
    edited: &CalendarEvent,
    uid: &str,
) -> Result<Map<String, Value>, SyncError> {
    let base_copy = match ical_to_event(base.icalendar) {
        Ok(copy) => copy,
        Err(error) => {
            // A bad base must not block every save of this event.
            tracing::warn!(uid, %error, "the base copy of the event does not parse");
            return Ok(patch::diff(current, edited));
        }
    };
    let conflicts = patch::conflicts(&base_copy, current, edited);
    if conflicts.is_empty() {
        return Ok(patch::diff_since(&base_copy, current, edited));
    }
    tracing::info!(uid, ?conflicts, on_conflict = ?base.on_conflict, "event edit conflict");
    match base.on_conflict {
        OnConflict::KeepLocal => Ok(patch::diff_since(&base_copy, current, edited)),
        OnConflict::KeepServer => Ok(patch::diff_since_except(
            &base_copy, current, edited, &conflicts,
        )),
        OnConflict::Fail => Err(SyncError::OutOfSync(uid.to_owned())),
    }
}

/// Returns `error` from a create, edit or delete as an [`Unscheduled`] refusal
/// when the call asked the server to schedule. Other errors stay as they are.
///
/// `event` is the event that was sent, if there is one. It gives the guests
/// that the server cannot reach.
fn refused(error: jmap_client::Error, schedule: bool, event: Option<&CalendarEvent>) -> SyncError {
    if !schedule {
        return error.into();
    }
    match &error {
        jmap_client::Error::Set(set)
            if set.error_type == calendar_event_set_error::NO_SUPPORTED_SCHEDULE_METHODS =>
        {
            SyncError::Unscheduled(Unscheduled::Unreachable {
                addresses: event.map(unreachable_addresses).unwrap_or_default(),
            })
        }
        jmap_client::Error::Method(method)
            if method.error_type == jmap_proto::error::method::INVALID_ARGUMENTS =>
        {
            SyncError::Unscheduled(Unscheduled::NotImplemented)
        }
        _ => error.into(),
    }
}

/// The calendar addresses of the participants and the organizer of `event` that
/// are not `mailto:`, sorted and without duplicates. A server that delivers
/// only by iMIP cannot reach these guests. A participant that an override adds
/// for one occurrence is not included.
fn unreachable_addresses(event: &CalendarEvent) -> Vec<String> {
    let participants = event
        .participants
        .iter()
        .flatten()
        .filter_map(|(_, participant)| participant.get("calendarAddress").and_then(Value::as_str));
    participants
        .chain(event.organizer_calendar_address.as_deref())
        .filter(|address| {
            !address
                .get(..7)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        })
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Whether this account is the scheduling origin of the stored `event`.
///
/// The server's `isOrigin` decides when it is present. Otherwise the account is
/// the origin when the event has no organizer or when `own` holds the organizer.
fn organised_here(event: &CalendarEvent, own: &[String]) -> bool {
    event.is_origin.unwrap_or_else(|| {
        event
            .organizer_calendar_address
            .as_deref()
            .is_none_or(|organizer| own.contains(&organizer.to_ascii_lowercase()))
    })
}

/// Whether a received `event` is an answer to an organizer that is not this account.
///
/// False when `own` is empty, because the client then cannot identify the origin.
fn answers(event: &CalendarEvent, own: &[String]) -> bool {
    event
        .organizer_calendar_address
        .as_deref()
        .is_some_and(|organizer| !own.is_empty() && !own.contains(&organizer.to_ascii_lowercase()))
}

/// Whether the patch entry `key` changes the guests or the organizer, also in an override.
fn names_guests(key: &str, value: &Value) -> bool {
    let guests = |key: &str| {
        key.split('/')
            .any(|segment| segment == "participants" || segment == "organizerCalendarAddress")
    };
    if guests(key) {
        return true;
    }
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| names_guests(key, value)),
        _ => false,
    }
}

/// Synchronises one JMAP calendar.
pub struct CalSync {
    client: Client,
    account_id: Id,
    calendar_id: Id,
    server_schedules: bool,
}

impl CalSync {
    pub fn new(client: Client, account_id: Id, calendar_id: Id) -> Self {
        Self {
            client,
            account_id,
            calendar_id,
            server_schedules: false,
        }
    }

    /// Lets the server send the iTIP messages. The calendar backend always turns this on.
    ///
    /// When on, each create, edit and delete sends `sendSchedulingMessages`
    /// (draft-ietf-jmap-calendars §5.9.2), unless the change is
    /// [`Itip::Suppress`]. The server then sends mail to the guests, or to the
    /// organizer when this account is only an attendee. When off (the
    /// default), the server never gets the request.
    ///
    /// The flag is sent also when the event has no participants. Such an event
    /// is not scheduled, so the server has nobody to send to. Thus a delete
    /// does not have to load the event first to examine it.
    pub fn with_server_scheduling(mut self, on: bool) -> Self {
        self.server_schedules = on;
        self
    }

    /// Whether the server is asked to send meeting invitations.
    pub fn server_schedules(&self) -> bool {
        self.server_schedules
    }

    /// Whether this change goes out with `sendSchedulingMessages`.
    fn schedules(&self, itip: Itip) -> bool {
        self.server_schedules && itip == Itip::Send
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn account_id(&self) -> &Id {
        &self.account_id
    }

    pub fn calendar_id(&self) -> &Id {
        &self.calendar_id
    }

    /// Returns every event in this calendar and the state of that list
    /// (`list_existing_sync`).
    ///
    /// No time range applies. `ECalMetaBackend` keeps a full local cache and
    /// answers range queries from it, so a range here would hide events and not
    /// decrease work.
    pub fn list_existing(&self) -> Result<(State, Vec<ComponentInfo>), SyncError> {
        let ids = self.client.event_query_every_id(
            &self.account_id,
            CalendarEventQueryFilter::in_calendar(self.calendar_id.clone()),
        )?;
        let response = self.client.event_get(&self.account_id, &ids)?;
        let events = response
            .list
            .iter()
            .map(ComponentInfo::render)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((response.state, events))
    }

    /// Returns one event by identifier (`load_component_sync`).
    ///
    /// This does not check that the event is in this calendar. EDS asks by the
    /// identifier that it has. [`CalSync::get_changes`] reports an event that
    /// moved out as removed, so loads do not fail for it.
    pub fn load_component(&self, uid: &str) -> Result<ComponentInfo, SyncError> {
        ComponentInfo::render(&self.fetch(uid)?)
    }

    /// Stores an iCalendar object (`save_component_sync`).
    ///
    /// With no `existing_uid`, this is a create, unless this calendar already
    /// holds an event with the same JSCalendar `uid` (then it is an edit of
    /// that event). On a create, the component `UID` is a name that Evolution
    /// made locally, never a JMAP id, so it does not become the id. It goes to
    /// the JSCalendar `uid`, which has the same meaning as an iCalendar `UID`.
    /// Thus the server keeps the identity that iTIP messages already use. With
    /// `existing_uid`, this is an edit: a PatchObject that names only what the
    /// round trip kept (see [`patch`]).
    ///
    /// Only a create can fail because of the component itself, because the
    /// server has no value to use in its place. Thus a recurrence rule that
    /// this mapping cannot state causes [`SyncError::Unsendable`], and the
    /// event is not filed without the rule. See the comment on that check.
    pub fn save_component(
        &self,
        icalendar: &str,
        existing_uid: Option<&str>,
    ) -> Result<ComponentInfo, SyncError> {
        self.save_component_with(icalendar, existing_uid, Itip::Send)
    }

    /// [`CalSync::save_component`], with a value that tells if the guests can
    /// get messages.
    ///
    /// If the server cannot reach a guest, it refuses the full change
    /// (`noSupportedScheduleMethods`, §5.9.2) and stores nothing. That refusal
    /// is the error of this save. Evolution knows that the server sends the
    /// messages, so a save without scheduling would leave the guests without an
    /// invitation.
    ///
    /// [`Itip::Received`] applies stricter rules, because the data comes from
    /// another person. The server schedules only the user's answer to an
    /// organizer that is known not to be this account. The change never sets the
    /// organizer. A change to the guests or the organizer of a meeting that this
    /// account organises fails with [`SyncError::OwnMeeting`].
    pub fn save_component_with(
        &self,
        icalendar: &str,
        existing_uid: Option<&str>,
        itip: Itip,
    ) -> Result<ComponentInfo, SyncError> {
        self.save_component_since(None, icalendar, existing_uid, itip)
    }

    /// [`CalSync::save_component_with`] for an edit of `base`, the component that the user edited.
    ///
    /// The edit patches only the properties that differ from `base`. Each other
    /// property keeps the value on the server, also when a different client changed it.
    /// A property that both changed follows [`Base::on_conflict`].
    pub fn save_component_since(
        &self,
        base: Option<Base<'_>>,
        icalendar: &str,
        existing_uid: Option<&str>,
        itip: Itip,
    ) -> Result<ComponentInfo, SyncError> {
        let schedule = self.schedules(itip);
        let mut event = ical_to_event(icalendar)?;
        // A create of a meeting that this calendar already holds: the
        // invitation that a guest accepts from their mail. Evolution sends it
        // with the meeting `UID`, which its cache (keyed by JMAP id) does not
        // know. But the server already delivered the meeting here, and it holds
        // a `uid` only once (Stalwart refuses a second copy with
        // `invalidProperties`). Thus this is an edit of that copy. The edit
        // also sends the guest's answer to the organizer.
        let held;
        let existing_uid = match existing_uid {
            Some(uid) => Some(uid),
            None => {
                let named = event.uid.as_deref().or(event.id.as_ref().map(Id::as_str));
                held = match named {
                    Some(named) => self.holding(named)?,
                    None => None,
                };
                held.as_deref()
            }
        };
        let Some(uid) = existing_uid else {
            let schedule = match itip {
                // Schedule only an answer to an organizer that is not this account.
                Itip::Received { reply } => {
                    let own = self.own_addresses()?;
                    reply && self.server_schedules && answers(&event, &own)
                }
                Itip::Send | Itip::Suppress => schedule,
            };
            let local = event.id.take();
            event.uid = event.uid.take().or_else(|| local.map(|id| id.to_string()));
            event.calendar_ids = Some(BTreeMap::from([(self.calendar_id.clone(), true)]));
            // A recurrence that this mapping cannot state is the only thing
            // that a create refuses fully and does not file without.
            //
            // The zone below is removed and not refused, because a wall-clock
            // time without a zone still shows the appointment that the user
            // typed. A recurrence has no such alternative. An event created
            // without its recurrence is a different event (one occurrence
            // instead of a series), and no message tells the user. To send the
            // rule unchanged is worse, because a server does not have to accept
            // the property. A strict server refuses the full
            // `CalendarEvent/set`. A lenient server stores an `until` that is
            // not an RFC 8984 §4.3.3 LocalDateTime, which [`event_to_ical`]
            // cannot draw. Then Evolution shows one appointment, but the server
            // runs the series, invites its guests and fires its alarms.
            //
            // Thus the save fails and Evolution tells the user. The user can
            // then state the series in a form that maps: a repeat count, not an
            // end date. This is less good than a recurrence that works, but
            // better than a meeting series that occurs only once without a
            // message. The affected rule is `UNTIL` with a `TZID` that the
            // document does not define, or defines in a shape that the zone
            // evaluator of `jmap-ical` does not accept. RFC 5545 §3.3.10
            // requires a UTC instant there, and RFC 8984 §4.3.3 requires a
            // local time in the event zone. Only the document's own `VTIMEZONE`
            // converts between them, if the document has one. See
            // [`maps_recurrence_rule`], [`unsendable_recurrence`] for the
            // message to the user, and [`patch::diff`] for the edit, which does
            // not change the property.
            if let Some(rule) = event
                .recurrence_rule
                .iter()
                .find(|rule| !maps_recurrence_rule(rule))
            {
                return Err(SyncError::Unsendable(unsendable_recurrence(
                    rule,
                    event.time_zone.as_deref(),
                )));
            }
            // A zone for which the document gives no JSCalendar spelling. On an
            // edit, the server zone stays (see [`patch`]). On a create, there
            // is no server zone, so the appointment is filed floating. A
            // floating wall-clock time shows correctly for the user who typed
            // it. An event that the server refused shows nothing.
            //
            // This applies only when no spelling exists. An IANA name has a
            // spelling. A zone that the document *defines* also has one: the
            // RFC 8984 §1.4.8 custom identifier, sent with the §4.7.2
            // `timeZones` entry that defines it. Thus an appointment with a
            // zone from an Exchange invitation (not from a database) gets to
            // the server as the correct event. See [`maps_time_zone`], and
            // `read_time_zones` in `jmap_ical`, which reads the definition from
            // the `VTIMEZONE`.
            if !maps_time_zone(&event) {
                event.time_zone = None;
                // The zone definition goes too: a `timeZones` entry that
                // nothing refers to is a claim about a zone that the event is
                // not in. Only the event's own entry goes. An occurrence that
                // the user moved into a different zone still names that zone.
                // If the full map went, that override would name a `TimeZoneId`
                // that nothing defines, and a server can refuse the full save
                // for that.
                prune_time_zones(&mut event);
            }
            let account_id = self.account_id().to_string();
            let calendar_id = self.calendar_id().to_string();
            tracing::debug!(account_id, calendar_id, schedule, "creating calendar event");
            let stored =
                match self
                    .client
                    .event_create_with_scheduling(&self.account_id, &event, schedule)
                {
                    Ok(stored) => stored,
                    Err(error) => {
                        tracing::warn!(
                            account_id,
                            calendar_id,
                            %error,
                            "calendar event create failed"
                        );
                        return Err(refused(error, schedule, Some(&event)));
                    }
                };
            // RFC 8620 §5.3 requires the server to report only the properties
            // that it set itself, so `stored` can contain only `id` (a real
            // deployment does this, see `tests/terse_create.rs`). Thus render
            // from a new load, not from `stored`, so that EDS gets what the
            // server filed, not only what the server echoed.
            let id = stored
                .id
                .as_ref()
                .ok_or_else(|| {
                    SyncError::protocol("CalendarEvent/set created an event without an id")
                })?
                .to_string();
            return self.load_component(&id);
        };

        let current = self.fetch(uid)?;
        let mut patch = match base {
            Some(base) => patch_since(base, &current, &event, uid)?,
            None => patch::diff(&current, &event),
        };
        let schedule = match itip {
            Itip::Received { reply } => {
                let own = self.own_addresses()?;
                if organised_here(&current, &own) {
                    if patch.iter().any(|(key, value)| names_guests(key, value)) {
                        return Err(SyncError::OwnMeeting);
                    }
                    false
                } else {
                    // Received data never changes the organizer.
                    patch.remove("organizerCalendarAddress");
                    reply && self.server_schedules && !own.is_empty()
                }
            }
            Itip::Send | Itip::Suppress => schedule,
        };
        if patch.is_empty() {
            return ComponentInfo::render(&current);
        }
        let account_id = self.account_id().to_string();
        let calendar_id = self.calendar_id().to_string();
        tracing::debug!(
            account_id,
            calendar_id,
            uid,
            schedule,
            "updating calendar event"
        );
        if let Err(error) = self.client.event_update_with_scheduling(
            &self.account_id,
            &Id::from(uid),
            Value::Object(patch),
            schedule,
        ) {
            tracing::warn!(
                account_id,
                calendar_id,
                uid,
                %error,
                "calendar event update failed"
            );
            return Err(refused(error, schedule, Some(&event)));
        }
        self.load_component(uid)
    }

    /// The calendar addresses of this account's participant identities, in lower case.
    fn own_addresses(&self) -> Result<Vec<String>, SyncError> {
        let identities = self.client.participant_identities(&self.account_id)?;
        Ok(identities
            .iter()
            .flat_map(|identity| {
                let send_to = identity.send_to.iter().flat_map(|map| map.values());
                identity.calendar_address.iter().chain(send_to)
            })
            .map(|address| address.to_ascii_lowercase())
            .collect())
    }

    /// The JMAP id of the event in this calendar whose JSCalendar `uid` is
    /// `uid`, if such an event exists.
    ///
    /// Only this calendar: this calendar does not edit an event in a different
    /// calendar. Thus the create continues, and the server decides.
    fn holding(&self, uid: &str) -> Result<Option<String>, SyncError> {
        let filter = CalendarEventQueryFilter {
            uid: Some(uid.to_owned()),
            ..CalendarEventQueryFilter::in_calendar(self.calendar_id.clone())
        };
        let ids = self.client.event_query(&self.account_id, filter)?.ids;
        Ok(ids.into_iter().next().map(|id| id.to_string()))
    }

    /// Destroys an event (`remove_component_sync`).
    pub fn remove_component(&self, uid: &str) -> Result<(), SyncError> {
        self.remove_component_with(uid, Itip::Send)
    }

    /// [`CalSync::remove_component`], with a value that tells if the guests can
    /// get messages.
    pub fn remove_component_with(&self, uid: &str, itip: Itip) -> Result<(), SyncError> {
        let schedule = match itip {
            Itip::Received { reply } => {
                let own = self.own_addresses()?;
                if organised_here(&self.fetch(uid)?, &own) {
                    return Err(SyncError::OwnMeeting);
                }
                reply && self.server_schedules && !own.is_empty()
            }
            Itip::Send | Itip::Suppress => self.schedules(itip),
        };
        let account_id = self.account_id().to_string();
        let calendar_id = self.calendar_id().to_string();
        tracing::debug!(
            account_id,
            calendar_id,
            uid,
            schedule,
            "removing calendar event"
        );
        if let Err(error) =
            self.client
                .event_destroy_with_scheduling(&self.account_id, &Id::from(uid), schedule)
        {
            tracing::warn!(
                account_id,
                calendar_id,
                uid,
                %error,
                "calendar event destroy failed"
            );
            return Err(refused(error, schedule, None));
        }
        Ok(())
    }

    /// What changed since `since` (`get_changes_sync`).
    ///
    /// If the state is too old for the server, this fails with a
    /// [`SyncError::is_cannot_calculate_changes`] error. The caller then lists
    /// the full calendar.
    pub fn get_changes(&self, since: &State) -> Result<Changes, SyncError> {
        self.classify(
            self.client
                .all_changes(&self.account_id, "CalendarEvent", since)?,
        )
    }

    /// Converts a raw `/changes` delta into the two lists that the meta backend
    /// uses.
    ///
    /// `CalendarEvent/changes` covers the full account, so the main work is to
    /// decide what an event that is *not* in this calendar means. The
    /// difference between created and updated makes this possible without the
    /// local cache. An **updated** event that is not in this calendar possibly
    /// moved out. It must be reported as removed, or Evolution continues to
    /// show an appointment that the calendar does not contain. A **created**
    /// event that is not in this calendar was never in it, so this calendar
    /// ignores it.
    ///
    /// The delta is already normalised: [`jmap_client::Client::all_changes`]
    /// decides the status of an id that several pages name. Thus no event is
    /// both a candidate and a removal.
    fn classify(&self, delta: ChangeSet) -> Result<Changes, SyncError> {
        let mut removed: Vec<String> = delta.destroyed.iter().map(Id::to_string).collect();
        let candidates: Vec<Id> = delta.created.union(&delta.updated).cloned().collect();
        let mut changed = Vec::new();

        if !candidates.is_empty() {
            let response = self.client.event_get(&self.account_id, &candidates)?;
            for event in &response.list {
                let Some(id) = &event.id else {
                    return Err(SyncError::protocol(
                        "CalendarEvent/get returned an event without an id",
                    ));
                };
                if self.holds(event)? {
                    changed.push(ComponentInfo::render(event)?);
                } else if delta.updated.contains(id) {
                    removed.push(id.to_string());
                }
            }
            // Removed between the /changes call and the /get. This is important
            // only for an event that existed before.
            removed.extend(
                response
                    .not_found
                    .iter()
                    .filter(|id| delta.updated.contains(*id))
                    .map(Id::to_string),
            );
        }

        Ok(Changes {
            new_state: delta.new_state,
            changed,
            removed,
        })
    }

    /// Whether `event` is filed in the calendar that this instance
    /// synchronises.
    ///
    /// `calendarIds` is a set (RFC 8620 §1.5), so each member that is present
    /// is `true`. A server that writes non-membership as a `false` entry, and
    /// does not omit the key, violates this. `messages_since` in
    /// `jmap-mail-sync` refuses the same `mailboxIds` violation and does not
    /// guess its meaning. No safe interpretation exists, so this function also
    /// refuses it and does not count it as absence.
    fn holds(&self, event: &CalendarEvent) -> Result<bool, SyncError> {
        match event
            .calendar_ids
            .as_ref()
            .and_then(|calendars| calendars.get(&self.calendar_id).copied())
        {
            Some(false) => Err(SyncError::protocol(format!(
                "CalendarEvent/get returned {} with a false-valued calendarIds entry for {}",
                event
                    .id
                    .as_ref()
                    .map(Id::to_string)
                    .unwrap_or_else(|| "<no id>".to_string()),
                self.calendar_id
            ))),
            Some(true) => Ok(true),
            None => Ok(false),
        }
    }

    fn fetch(&self, uid: &str) -> Result<CalendarEvent, SyncError> {
        let id = Id::from(uid);
        let response = self
            .client
            .event_get(&self.account_id, std::slice::from_ref(&id))?;
        response
            .list
            .into_iter()
            .next()
            .ok_or_else(|| SyncError::NotFound(uid.to_owned()))
    }
}

/// Why a create was refused because of its recurrence.
///
/// Two answers exist, because only one of the two possible statements gives the
/// user an action. When the end of the rule cannot be stated (see
/// [`unstateable_until`]), the reason contains the instant and the zone.
/// Together they identify the appointment to change and the `VTIMEZONE` to
/// examine. Each other refusal is the general one: the mapping knows that it
/// cannot write the rule back, but no detail helps a person who reads a dialog.
///
/// The zone name is important. The document's own `VTIMEZONE` does the
/// conversion, so an end date as a UTC instant is the case that *works*. The
/// remaining case is an entry whose zone is missing or not readable. Without
/// the zone name, the user cannot tell this error from the error that this code
/// gave before for the ordinary case.
///
/// `jmap_backend_cal::ops` makes the sentence for the user from these two
/// values. Gettext is bound there, so only there can the string be translated.
/// That is why this function returns a reason and not text.
fn unsendable_recurrence(rule: &RecurrenceRule, time_zone: Option<&str>) -> Unsendable {
    match (unstateable_until(rule), time_zone) {
        (Some(until), Some(zone)) => Unsendable::RecurrenceEnd {
            until: until.to_owned(),
            zone: zone.to_owned(),
        },
        _ => Unsendable::Recurrence,
    }
}

impl ComponentInfo {
    /// Renders an event and calculates its revision from the result.
    fn render(event: &CalendarEvent) -> Result<Self, SyncError> {
        let uid = event
            .id
            .as_ref()
            .ok_or_else(|| {
                SyncError::protocol("CalendarEvent/get returned an event without an id")
            })?
            .to_string();
        let icalendar = event_to_ical(event);
        Ok(Self {
            revision: revision_of(&icalendar),
            uid,
            icalendar,
        })
    }
}

/// The change token for a rendered event.
///
/// The JSCalendar `updated` timestamp is the obvious candidate, but it is
/// wrong. RFC 8984 makes it optional, so if a server omits it, every event
/// looks unchanged permanently. A digest of the component is always available,
/// and it is a *better* token than a timestamp. It changes exactly when a
/// change that EDS can see occurs. Thus a server-side edit to a property that
/// this mapping drops does not invalidate the cache of each client.
///
/// This uses FNV-1a, not `DefaultHasher`, and has its own copy, not one shared
/// with `jmap-book-sync`. The EDS cache keeps revisions and compares them
/// across restarts, and the output of `DefaultHasher` can change between Rust
/// releases.
fn revision_of(icalendar: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in icalendar.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use jmap_proto::error::{MethodError, SetError};

    fn organised_by(organizer: Option<&str>) -> CalendarEvent {
        CalendarEvent {
            organizer_calendar_address: organizer.map(str::to_owned),
            ..CalendarEvent::default()
        }
    }

    #[test]
    fn a_received_event_answers_only_an_organizer_known_not_to_be_this_account() {
        let own = vec!["mailto:olga@example.com".to_owned()];
        let vera = organised_by(Some("mailto:vera@example.com"));
        assert!(answers(&vera, &own));
        assert!(!answers(&vera, &[]), "no identity: the origin is unknown");
        assert!(!answers(
            &organised_by(Some("MAILTO:Olga@Example.com")),
            &own
        ));
        assert!(!answers(&organised_by(None), &own));
    }

    #[test]
    fn the_servers_is_origin_decides_who_organises_a_stored_event() {
        let own = vec!["mailto:olga@example.com".to_owned()];
        let mut vera = organised_by(Some("mailto:vera@example.com"));
        assert!(!organised_here(&vera, &own));
        assert!(!organised_here(&vera, &[]));
        assert!(organised_here(&organised_by(None), &own));
        vera.is_origin = Some(true);
        assert!(organised_here(&vera, &own));
    }

    #[test]
    fn a_guest_or_organizer_change_is_found_also_inside_an_override() {
        assert!(names_guests("participants/p2", &Value::Null));
        assert!(names_guests("organizerCalendarAddress", &Value::Null));
        let inner = serde_json::json!({"2026-09-29T11:00:00": {"participants/p1/roles": {}}});
        assert!(names_guests("recurrenceOverrides", &inner));
        assert!(!names_guests(
            "title",
            &Value::String("participants".to_owned())
        ));
    }

    /// The EDS cache keeps revisions after the process stops, so this test pins
    /// the digest to the published FNV-1a 64-bit test vectors. A change would
    /// make each cached event look changed after an upgrade.
    #[test]
    fn revision_of_is_fnv1a_64() {
        assert_eq!(revision_of(""), "cbf29ce484222325");
        assert_eq!(revision_of("a"), "af63dc4c8601ec8c");
        assert_eq!(revision_of("foobar"), "85944171f73967e8");
    }

    #[test]
    fn a_set_error_other_than_no_supported_schedule_methods_stays_a_client_error() {
        let error = jmap_client::Error::Set(SetError::new("forbidden"));
        let result = refused(error, true, None);
        assert!(
            matches!(result, SyncError::Client(_)),
            "only noSupportedScheduleMethods is a scheduling refusal: {result:?}"
        );
    }

    #[test]
    fn a_method_error_other_than_invalid_arguments_stays_a_client_error() {
        let error = jmap_client::Error::Method(MethodError::new("forbidden"));
        let result = refused(error, true, None);
        assert!(
            matches!(result, SyncError::Client(_)),
            "only invalidArguments reads as sendSchedulingMessages not implemented: {result:?}"
        );
    }
}
