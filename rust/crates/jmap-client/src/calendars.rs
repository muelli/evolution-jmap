// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar operations (draft-ietf-jmap-calendars).

use jmap_proto::Id;
use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventNotification, CalendarEventNotificationQueryFilter,
    CalendarEventParseRequest, CalendarEventParseResponse, CalendarEventQueryFilter,
};
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CORE};
use serde_json::Value;

use crate::client::Client;
use crate::contacts::set_failure;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_CALENDARS];

impl Client {
    /// Fetch all calendars (`Calendar/get` with `ids: null`).
    pub fn calendars(&self, account_id: &Id) -> Result<Vec<Calendar>, Error> {
        let arguments =
            self.single_call(USING, "Calendar/get", &GetRequest::all(account_id.clone()))?;
        let response: GetResponse<Calendar> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Create a calendar; returns the stored calendar (with server-set id).
    pub fn calendar_create(&self, account_id: &Id, calendar: &Calendar) -> Result<Calendar, Error> {
        let request =
            SetRequest::<Calendar>::new(account_id.clone()).create("new", calendar.clone());
        let response = self.calendar_set(&request)?;
        if let Some(created) = response
            .created
            .as_ref()
            .and_then(|created| created.get("new"))
        {
            return Ok(created.clone());
        }
        Err(set_failure(
            response.not_created.as_ref().and_then(|map| map.get("new")),
        ))
    }

    /// Destroy a calendar.
    pub fn calendar_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SetRequest::<Calendar>::new(account_id.clone()).destroy(id.clone());
        let response = self.calendar_set(&request)?;
        if response
            .destroyed
            .as_ref()
            .is_some_and(|destroyed| destroyed.contains(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_destroyed.as_ref().and_then(|map| map.get(id)),
        ))
    }

    /// Apply a patch to a calendar (RFC 8620 PatchObject).
    pub fn calendar_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SetRequest::<Calendar>::new(account_id.clone()).update(id.clone(), patch);
        let response = self.calendar_set(&request)?;
        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_updated.as_ref().and_then(|map| map.get(id)),
        ))
    }

    fn calendar_set(&self, request: &SetRequest<Calendar>) -> Result<SetResponse<Calendar>, Error> {
        let arguments = self.single_call(USING, "Calendar/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Create a calendar event; returns the stored event (with server-set
    /// id, uid, …).
    pub fn event_create(
        &self,
        account_id: &Id,
        event: &CalendarEvent,
    ) -> Result<CalendarEvent, Error> {
        let request =
            SetRequest::<CalendarEvent>::new(account_id.clone()).create("new", event.clone());
        let response = self.event_set(&request)?;
        if let Some(created) = response
            .created
            .as_ref()
            .and_then(|created| created.get("new"))
        {
            return Ok(created.clone());
        }
        Err(set_failure(
            response.not_created.as_ref().and_then(|map| map.get("new")),
        ))
    }

    /// Fetch calendar events by id.
    pub fn event_get(
        &self,
        account_id: &Id,
        ids: &[Id],
    ) -> Result<GetResponse<CalendarEvent>, Error> {
        let request = GetRequest::ids(account_id.clone(), ids.iter().cloned());
        let arguments = self.single_call(USING, "CalendarEvent/get", &request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Apply a patch to a calendar event (RFC 8620 PatchObject).
    pub fn event_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request =
            SetRequest::<CalendarEvent>::new(account_id.clone()).update(id.clone(), patch);
        let response = self.event_set(&request)?;
        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_updated.as_ref().and_then(|map| map.get(id)),
        ))
    }

    /// Destroy a calendar event.
    pub fn event_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SetRequest::<CalendarEvent>::new(account_id.clone()).destroy(id.clone());
        let response = self.event_set(&request)?;
        if response
            .destroyed
            .as_ref()
            .is_some_and(|destroyed| destroyed.contains(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_destroyed.as_ref().and_then(|map| map.get(id)),
        ))
    }

    /// `CalendarEvent/parse` (draft-ietf-jmap-calendars §5.7): reads an
    /// uploaded iCalendar blob into a JSCalendar `CalendarEvent`.
    pub fn event_parse(
        &self,
        request: &CalendarEventParseRequest,
    ) -> Result<CalendarEventParseResponse, Error> {
        let arguments = self.single_call(USING, "CalendarEvent/parse", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// `CalendarEvent/query`: matching event ids (sorted by start).
    pub fn event_query(
        &self,
        account_id: &Id,
        filter: CalendarEventQueryFilter,
    ) -> Result<QueryResponse, Error> {
        let request = QueryRequest::new(account_id.clone()).filter(filter);
        let arguments = self.single_call(USING, "CalendarEvent/query", &request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    fn event_set(
        &self,
        request: &SetRequest<CalendarEvent>,
    ) -> Result<SetResponse<CalendarEvent>, Error> {
        let arguments = self.single_call(USING, "CalendarEvent/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Fetch all `CalendarEventNotification`s visible to this credential
    /// (draft-ietf-jmap-calendars §8) — one appears for each
    /// create/update/destroy of an event on a calendar shared with this
    /// principal, made by someone else.
    pub fn calendar_event_notifications(
        &self,
        account_id: &Id,
    ) -> Result<Vec<CalendarEventNotification>, Error> {
        let arguments = self.single_call(
            USING,
            "CalendarEventNotification/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: GetResponse<CalendarEventNotification> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Resolve `CalendarEventNotification` ids matching `filter`
    /// (`CalendarEventNotification/query`, draft §8).
    pub fn calendar_event_notification_query(
        &self,
        account_id: &Id,
        filter: CalendarEventNotificationQueryFilter,
    ) -> Result<Vec<Id>, Error> {
        let request = QueryRequest::new(account_id.clone()).filter(filter);
        let arguments = self.single_call(USING, "CalendarEventNotification/query", &request)?;
        let response: QueryResponse = serde_json::from_value(arguments)?;
        Ok(response.ids)
    }

    /// Dismiss a `CalendarEventNotification`: destroy is the only mutation
    /// the draft allows on this type (create/update are always rejected as
    /// `forbidden`, since the object is entirely server-created).
    pub fn calendar_event_notification_destroy(
        &self,
        account_id: &Id,
        id: &Id,
    ) -> Result<(), Error> {
        let request =
            SetRequest::<CalendarEventNotification>::new(account_id.clone()).destroy(id.clone());
        let arguments = self.single_call(USING, "CalendarEventNotification/set", &request)?;
        let response: SetResponse<CalendarEventNotification> = serde_json::from_value(arguments)?;
        if response
            .destroyed
            .as_ref()
            .is_some_and(|destroyed| destroyed.contains(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_destroyed.as_ref().and_then(|map| map.get(id)),
        ))
    }
}
