// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! Calendar synchronisation, in the shape `ECalMetaBackend` asks for.
//!
//! One EDS calendar source is one JMAP calendar, and this crate is the whole
//! of what syncing it means: which events exist, what an event looks like as
//! an iCalendar object, what changed since a state string, and how an edit
//! made in Evolution turns into a `CalendarEvent/set`. Each entry point
//! corresponds to one vfunc — [`CalSync::list_existing`] to
//! `list_existing_sync`, [`CalSync::save_component`] to
//! `save_component_sync`, and so on.
//!
//! It deliberately knows nothing about GObject or the Evolution headers, so
//! the interesting half of the backend is testable against `jmap-mockd` on
//! any machine. The subclass on top is left with lifecycle and marshalling.
//! It is the calendar-side counterpart of `jmap-book-sync`, and follows its
//! shape closely enough that the two read as one design.

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
use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter, RecurrenceRule};
use jmap_proto::{Id, State};
use serde_json::Value;

pub use error::{SyncError, Unsendable};
pub use freebusy::FreeBusy;

/// One event, as the meta backend wants it: an identifier, a change token and
/// the object itself.
///
/// This is the payload of an `ECalMetaBackendInfo`. `uid` is the JMAP id —
/// see the crate docs of `jmap-ical` for why it, and not the JSCalendar
/// `uid`, is what EDS keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentInfo {
    pub uid: String,
    pub revision: String,
    pub icalendar: String,
}

/// What changed in the calendar since a given state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// The state to pass to the next [`CalSync::get_changes`].
    pub new_state: State,
    /// Events that were created or modified, already rendered.
    pub changed: Vec<ComponentInfo>,
    /// Identifiers that are gone from *this* calendar, whether they were
    /// destroyed or merely moved elsewhere.
    pub removed: Vec<String>,
}

/// Synchronises one JMAP calendar.
pub struct CalSync {
    client: Client,
    account_id: Id,
    calendar_id: Id,
}

impl CalSync {
    pub fn new(client: Client, account_id: Id, calendar_id: Id) -> Self {
        Self {
            client,
            account_id,
            calendar_id,
        }
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

    /// Every event in this calendar, with the state that listing is current
    /// as of — `list_existing_sync`.
    ///
    /// No time range is applied. `ECalMetaBackend` keeps a full local cache
    /// and answers ranged queries out of it, so narrowing here would hide
    /// events rather than save work.
    pub fn list_existing(&self) -> Result<(State, Vec<ComponentInfo>), SyncError> {
        let query = self.client.event_query(
            &self.account_id,
            CalendarEventQueryFilter::in_calendar(self.calendar_id.clone()),
        )?;
        let response = self.client.event_get(&self.account_id, &query.ids)?;
        let events = response
            .list
            .iter()
            .map(ComponentInfo::render)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((response.state, events))
    }

    /// One event by identifier — `load_component_sync`.
    ///
    /// Membership of this calendar is not checked: EDS asks by the identity
    /// it was given, and an event that has moved out is reported gone by
    /// [`CalSync::get_changes`] rather than by making loads fail.
    pub fn load_component(&self, uid: &str) -> Result<ComponentInfo, SyncError> {
        ComponentInfo::render(&self.fetch(uid)?)
    }

    /// Store an iCalendar object — `save_component_sync`.
    ///
    /// With no `existing_uid` this is a create: the component's `UID` is a
    /// name Evolution invented locally, never a JMAP id, so it does not
    /// become one. It is not thrown away either — it moves to the JSCalendar
    /// `uid`, which is the property that means what an iCalendar `UID` means,
    /// so the identity any iTIP correspondence already quotes survives the
    /// trip to the server. Otherwise this is an edit, sent as a PatchObject
    /// that names only what the round trip preserved — see [`patch`].
    ///
    /// A create is the one of the two that can fail over the component itself:
    /// there is no server-side value to fall back on, so a recurrence rule this
    /// mapping cannot state is [`SyncError::Unsendable`] rather than an event
    /// filed without it. See the comment on that check.
    pub fn save_component(
        &self,
        icalendar: &str,
        existing_uid: Option<&str>,
    ) -> Result<ComponentInfo, SyncError> {
        let mut event = ical_to_event(icalendar)?;
        let Some(uid) = existing_uid else {
            let local = event.id.take();
            event.uid = event.uid.take().or_else(|| local.map(|id| id.to_string()));
            event.calendar_ids = Some(BTreeMap::from([(self.calendar_id.clone(), true)]));
            // A recurrence this mapping cannot state, which is the one thing a
            // create refuses outright rather than files without.
            //
            // The zone below is dropped rather than refused because a
            // wall-clock time with no zone is still the appointment the person
            // who typed it sees. There is no such reading here: an event
            // created without its recurrence is a different event — one
            // occurrence where the user asked for a series — and nothing says
            // so. Sending the rule as it stands is worse again, because the
            // property is not one a server has to accept: a strict one refuses
            // the whole `CalendarEvent/set`, and a lenient one stores an
            // `until` that is no RFC 8984 §4.3.3 LocalDateTime, which
            // [`event_to_ical`] then cannot draw — so Evolution would show a
            // single appointment while the server ran the series, invited its
            // guests and fired its alarms.
            //
            // So the save fails and Evolution says so. The user can then state
            // the same series a way that does map — a repeat count rather than
            // an end date — which is a worse thing to be asked for than a
            // recurrence that just works, and a better one than a meeting
            // series that quietly happened once. The rule this actually costs
            // is `UNTIL` beside a `TZID` the document does not define, or
            // defines in a shape `jmap-ical`'s zone evaluator will not guess
            // at: RFC 5545 §3.3.10 requires the UTC instant there and RFC 8984
            // §4.3.3 wants a local time in the event's zone, and what converts
            // between them is the document's own `VTIMEZONE` — where there is
            // one. See [`maps_recurrence_rule`], [`unsendable_recurrence`] for
            // what the user is then told, and [`patch::diff`] for the edit,
            // which leaves the property alone instead.
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
            // A zone the document gave no JSCalendar spelling for. On an edit
            // the server's own zone stands (see [`patch`]); on a create there is
            // none to stand, so the appointment is filed floating. A
            // wall-clock time with no zone shows correctly for the user who
            // typed it, and an event the server refused shows nothing at all.
            //
            // Which is the answer only where there is no spelling. An IANA name
            // has one, and so does a zone the document *defines* — RFC 8984
            // §1.4.9's custom identifier, sent beside the §4.7.2 `timeZones`
            // entry that says what it is, which is how an appointment whose zone
            // came from an Exchange invitation rather than a database reaches
            // the server as the event it is. See [`maps_time_zone`], and
            // `jmap_ical`'s `read_time_zones`, which is where the definition is
            // read off the `VTIMEZONE`.
            if !maps_time_zone(&event) {
                event.time_zone = None;
                // With the zone goes its definition: a `timeZones` entry nothing
                // refers to is a claim about a zone the event is not in. Only
                // its own, though — an occurrence the user moved into a zone of
                // its own still names one, and taking the whole map left that
                // override pointing at a `TimeZoneId` nothing defined, which is
                // the shape a server may refuse the entire save over.
                prune_time_zones(&mut event);
            }
            let account_id = self.account_id().to_string();
            let calendar_id = self.calendar_id().to_string();
            tracing::debug!(account_id, calendar_id, "creating calendar event");
            let stored = match self.client.event_create(&self.account_id, &event) {
                Ok(stored) => stored,
                Err(error) => {
                    tracing::warn!(
                        account_id,
                        calendar_id,
                        %error,
                        "calendar event create failed"
                    );
                    return Err(error.into());
                }
            };
            // RFC 8620 §5.3 only requires the server to report properties
            // it set itself, so `stored` may carry nothing but `id` (a real
            // deployment does exactly this — see `tests/terse_create.rs`).
            // Render from a fresh load rather than `stored` directly, so the
            // iCalendar object handed back to EDS always reflects what was
            // actually filed, not merely what a chatty server happened to
            // echo.
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
        let patch = patch::diff(&current, &event);
        if patch.is_empty() {
            return ComponentInfo::render(&current);
        }
        let account_id = self.account_id().to_string();
        let calendar_id = self.calendar_id().to_string();
        tracing::debug!(account_id, calendar_id, uid, "updating calendar event");
        if let Err(error) =
            self.client
                .event_update(&self.account_id, &Id::from(uid), Value::Object(patch))
        {
            tracing::warn!(
                account_id,
                calendar_id,
                uid,
                %error,
                "calendar event update failed"
            );
            return Err(error.into());
        }
        self.load_component(uid)
    }

    /// Destroy an event — `remove_component_sync`.
    pub fn remove_component(&self, uid: &str) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let calendar_id = self.calendar_id().to_string();
        tracing::debug!(account_id, calendar_id, uid, "removing calendar event");
        if let Err(error) = self.client.event_destroy(&self.account_id, &Id::from(uid)) {
            tracing::warn!(
                account_id,
                calendar_id,
                uid,
                %error,
                "calendar event destroy failed"
            );
            return Err(error.into());
        }
        Ok(())
    }

    /// What changed since `since` — `get_changes_sync`.
    ///
    /// Fails with a [`SyncError::is_cannot_calculate_changes`] error if the
    /// state is too old for the server, which the caller answers by listing
    /// the calendar in full.
    pub fn get_changes(&self, since: &State) -> Result<Changes, SyncError> {
        self.classify(
            self.client
                .all_changes(&self.account_id, "CalendarEvent", since)?,
        )
    }

    /// Turn a raw `/changes` delta into the two lists the meta backend takes.
    ///
    /// `CalendarEvent/changes` is account-wide, so most of the work is
    /// deciding what an event that is *not* in this calendar means. The
    /// created/updated distinction is what makes that decidable without
    /// consulting the local cache: an event that shows up as **updated** and
    /// is not ours may have just been moved out, and has to be reported gone
    /// or Evolution keeps showing an appointment the calendar no longer
    /// contains; an event that shows up as **created** and is not ours was
    /// never in this calendar, so it is simply not our business.
    ///
    /// The delta arrives normalised — [`jmap_client::Client::all_changes`] has
    /// already decided what an id named by several pages is — so no event is
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
                if self.holds(event) {
                    changed.push(ComponentInfo::render(event)?);
                } else if delta.updated.contains(id) {
                    removed.push(id.to_string());
                }
            }
            // Gone between the /changes call and the /get: only interesting
            // for an event that already existed.
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

    /// Whether `event` is filed in the calendar this instance syncs.
    fn holds(&self, event: &CalendarEvent) -> bool {
        event
            .calendar_ids
            .as_ref()
            .is_some_and(|calendars| calendars.get(&self.calendar_id).copied().unwrap_or(false))
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

/// Why a create was refused over its recurrence.
///
/// Two answers, because there are two things worth saying and only one of them
/// is actionable. Where the rule's end is what could not be stated — see
/// [`unstateable_until`] — the reason carries the instant and the zone it could
/// not be stated in, which between them identify the appointment to change and
/// the `VTIMEZONE` to look at. Every other refusal is the general one: the
/// mapping knows the rule cannot be written back, but nothing about *which*
/// part of it would help someone reading a dialog.
///
/// Naming the zone matters more than it looks. Since the document's own
/// `VTIMEZONE` became the conversion, an end date stated as a UTC instant is
/// the case that *works*; what is left is a calendar entry whose zone is
/// missing or written unreadably, and a refusal that did not say which zone
/// would leave the user with an error they cannot tell apart from the one this
/// code used to give for the ordinary case.
///
/// The sentence the user reads is `jmap_backend_cal::ops`'s, over these two
/// values — that is where gettext is bound and where the string can therefore
/// be translated, which is the whole reason this hands back a reason rather
/// than prose.
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
    /// Render an event, deriving its revision from the result.
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
/// JSCalendar's `updated` timestamp is the obvious candidate and the wrong
/// one: RFC 8984 leaves it optional, so a server that omits it would make
/// every event look unchanged forever. A digest of the component is always
/// available, and it is a *better* token than a timestamp — it changes
/// exactly when something EDS can see changes, so a server-side edit to a
/// property this mapping drops does not churn every client's cache.
///
/// FNV-1a rather than `DefaultHasher`, and spelled out here rather than
/// shared with `jmap-book-sync`: revisions are persisted in the EDS cache and
/// compared across restarts, and `DefaultHasher`'s output is explicitly not
/// stable between Rust releases.
fn revision_of(icalendar: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in icalendar.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
