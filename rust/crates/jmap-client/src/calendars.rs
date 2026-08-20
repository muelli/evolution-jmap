// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar operations (draft-ietf-jmap-calendars).

use jmap_proto::Id;
use jmap_proto::calendars::{Calendar, CalendarEvent, CalendarEventQueryFilter};
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
}
