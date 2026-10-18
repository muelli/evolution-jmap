// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar operations (draft-ietf-jmap-calendars).

use jmap_proto::Id;
use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventNotification, CalendarEventNotificationQueryFilter,
    CalendarEventParseRequest, CalendarEventParseResponse, CalendarEventQueryFilter,
    CalendarEventSetRequest, ParticipantIdentity, ParticipantIdentitySetRequest,
};
use jmap_proto::error::SetError;
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use jmap_proto::request::Request;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CORE};
use serde::Deserialize;
use serde_json::Value;

use crate::client::Client;
use crate::contacts::{demote_multi_element_parsed_blobs, set_failure};
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

    /// Create a calendar event and return the stored record.
    pub fn event_create(
        &self,
        account_id: &Id,
        event: &CalendarEvent,
    ) -> Result<CalendarEvent, Error> {
        self.event_create_with_scheduling(account_id, event, false)
    }

    /// Create a calendar event and request participant invitations.
    ///
    /// Sends scheduling messages when `send_scheduling_messages` is true.
    /// Returns a [`SetError`] when the server refuses scheduling.
    pub fn event_create_with_scheduling(
        &self,
        account_id: &Id,
        event: &CalendarEvent,
        send_scheduling_messages: bool,
    ) -> Result<CalendarEvent, Error> {
        let request =
            SetRequest::<CalendarEvent>::new(account_id.clone()).create("new", event.clone());
        let response = self.event_set_scheduling(request, send_scheduling_messages)?;
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

    /// Fetch calendar events by ID (RFC 8620 §2).
    ///
    /// Splits requests into chunks when IDs exceed `maxObjectsInGet`.
    /// Executes at least one call when `ids` is empty.
    pub fn event_get(
        &self,
        account_id: &Id,
        ids: &[Id],
    ) -> Result<GetResponse<CalendarEvent>, Error> {
        let max_objects = self
            .session()
            .max_objects_in_get()
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|&limit| limit > 0)
            .unwrap_or(ids.len().max(1));

        let mut combined: Option<GetResponse<CalendarEvent>> = None;
        let mut rest = ids;
        loop {
            let take = rest.len().min(max_objects);
            let (chunk, remaining) = rest.split_at(take);

            let call_id = self.next_call_id();
            let request = Request::new([CAPABILITY_CORE, CAPABILITY_CALENDARS]).call(
                "CalendarEvent/get",
                &GetRequest::ids(account_id.clone(), chunk.iter().cloned()),
                &call_id,
            )?;
            let response = self.api_call(&request)?;
            let invocation = response
                .responses_for(&call_id)
                .next()
                .ok_or_else(|| Error::Protocol("no CalendarEvent/get response".to_owned()))?;
            let arguments = Self::unwrap_invocation(invocation, "CalendarEvent/get")?;
            let chunk_response: GetResponse<CalendarEvent> = serde_json::from_value(arguments)?;
            match &mut combined {
                None => combined = Some(chunk_response),
                Some(combined) => {
                    combined.list.extend(chunk_response.list);
                    combined.not_found.extend(chunk_response.not_found);
                    combined.state = chunk_response.state;
                }
            }

            rest = remaining;
            if rest.is_empty() {
                return Ok(combined.expect("the loop always sets combined on its first iteration"));
            }
        }
    }

    /// Apply a patch to a calendar event (RFC 8620 PatchObject).
    pub fn event_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        self.event_update_with_scheduling(account_id, id, patch, false)
    }

    /// Update a calendar event and request participant notifications.
    ///
    /// Sends scheduling updates when `send_scheduling_messages` is true.
    pub fn event_update_with_scheduling(
        &self,
        account_id: &Id,
        id: &Id,
        patch: Value,
        send_scheduling_messages: bool,
    ) -> Result<(), Error> {
        let request =
            SetRequest::<CalendarEvent>::new(account_id.clone()).update(id.clone(), patch);
        let response = self.event_set_scheduling(request, send_scheduling_messages)?;
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
        self.event_destroy_with_scheduling(account_id, id, false)
    }

    /// Destroy a calendar event and request cancellation messages.
    ///
    /// Sends cancellation notices when `send_scheduling_messages` is true.
    pub fn event_destroy_with_scheduling(
        &self,
        account_id: &Id,
        id: &Id,
        send_scheduling_messages: bool,
    ) -> Result<(), Error> {
        let request = SetRequest::<CalendarEvent>::new(account_id.clone()).destroy(id.clone());
        let response = self.event_set_scheduling(request, send_scheduling_messages)?;
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

    /// Parse an uploaded iCalendar blob into a [`CalendarEvent`] (draft-ietf-jmap-calendars §5.7).
    ///
    /// Accepts single-element array responses.
    /// Marks blobs with multiple distinct events as not parsable.
    pub fn event_parse(
        &self,
        request: &CalendarEventParseRequest,
    ) -> Result<CalendarEventParseResponse, Error> {
        let mut arguments = self.single_call(USING, "CalendarEvent/parse", request)?;
        demote_multi_element_parsed_blobs(&mut arguments);
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

    /// Modify calendar events (`CalendarEvent/set`, draft-ietf-jmap-calendars §5.9).
    ///
    /// Supports the `sendSchedulingMessages` parameter for iTIP scheduling.
    pub fn event_set(
        &self,
        request: &CalendarEventSetRequest,
    ) -> Result<SetResponse<CalendarEvent>, Error> {
        let arguments = self.single_call(USING, "CalendarEvent/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    // Omits `sendSchedulingMessages` when false for server compatibility.
    fn event_set_scheduling(
        &self,
        request: SetRequest<CalendarEvent>,
        send_scheduling_messages: bool,
    ) -> Result<SetResponse<CalendarEvent>, Error> {
        let request = CalendarEventSetRequest::new(request);
        self.event_set(&if send_scheduling_messages {
            request.send_scheduling_messages(true)
        } else {
            request
        })
    }

    /// Fetch all calendar event notifications for this account (draft-ietf-jmap-calendars §8).
    ///
    /// Notifications report remote changes on shared calendars.
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

    /// Dismiss a calendar event notification.
    ///
    /// Destroy is the only supported mutation on notifications (draft §8).
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
        if let Some(err) = response.not_destroyed.as_ref().and_then(|map| map.get(id)) {
            return Err(Error::Set(err.clone()));
        }
        // Treat omitted IDs as notFound.
        Err(Error::Set(SetError::new("notFound")))
    }

    /// Fetch all participant identities (`ParticipantIdentity/get`, draft-ietf-jmap-calendars §3.1).
    pub fn participant_identities(
        &self,
        account_id: &Id,
    ) -> Result<Vec<ParticipantIdentity>, Error> {
        let arguments = self.single_call(
            USING,
            "ParticipantIdentity/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: ParticipantIdentityGetResponse = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Create a participant identity; returns the stored identity (with
    /// server-set `id` and `isDefault`).
    pub fn participant_identity_create(
        &self,
        account_id: &Id,
        identity: &ParticipantIdentity,
    ) -> Result<ParticipantIdentity, Error> {
        let request = ParticipantIdentitySetRequest::new(
            SetRequest::<ParticipantIdentity>::new(account_id.clone())
                .create("new", identity.clone()),
        );
        let response = self.participant_identity_set(&request)?;
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

    /// Apply a patch to a participant identity (RFC 8620 PatchObject).
    pub fn participant_identity_update(
        &self,
        account_id: &Id,
        id: &Id,
        patch: Value,
    ) -> Result<(), Error> {
        let request = ParticipantIdentitySetRequest::new(
            SetRequest::<ParticipantIdentity>::new(account_id.clone()).update(id.clone(), patch),
        );
        let response = self.participant_identity_set(&request)?;
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

    /// Destroy a participant identity.
    ///
    /// Fails with `cannotDestroyDefault` if the identity is default.
    pub fn participant_identity_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = ParticipantIdentitySetRequest::new(
            SetRequest::<ParticipantIdentity>::new(account_id.clone()).destroy(id.clone()),
        );
        let response = self.participant_identity_set(&request)?;
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

    /// Make an identity the default participant identity (draft-ietf-jmap-calendars §3.2).
    ///
    /// Demotes the previous default identity.
    pub fn participant_identity_set_default(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = ParticipantIdentitySetRequest::new(SetRequest::new(account_id.clone()))
            .setting_default(id.to_string());
        self.participant_identity_set(&request)?;
        Ok(())
    }

    fn participant_identity_set(
        &self,
        request: &ParticipantIdentitySetRequest,
    ) -> Result<SetResponse<ParticipantIdentity>, Error> {
        let arguments = self.single_call(USING, "ParticipantIdentity/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParticipantIdentityGetResponse {
    #[serde(default)]
    list: Vec<ParticipantIdentity>,
}
