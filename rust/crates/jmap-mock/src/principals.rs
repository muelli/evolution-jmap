// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal methods (`Principal/get`, `Principal/query`, RFC 9670) and
//! principal seeding helpers.

use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::error::MethodError;
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::principals::{
    BusyPeriod, GetAvailabilityRequest, GetAvailabilityResponse, Principal, PrincipalQueryFilter,
};
use jmap_proto::session::CAPABILITY_CALENDARS;
use jmap_proto::state::UtcDate;
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::state::AccountState;

pub fn principal_get(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(
            account
                .principals
                .iter()
                .map(|(_, principal)| principal.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.principals.get(id) {
                    Some(principal) => list.push(principal.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.principals.state(),
        list,
        not_found,
    })
}

pub fn principal_query(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: QueryRequest<PrincipalQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let ids: Vec<Id> = account
        .principals
        .iter()
        .filter(|(_, principal)| principal_matches(principal, &filter))
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let total = account
        .principals
        .iter()
        .filter(|(_, principal)| principal_matches(principal, &filter))
        .count() as u64;

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.principals.state(),
        can_calculate_changes: false,
        position: request.position.max(0) as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

/// `Principal/getAvailability` (draft-ietf-jmap-calendars §2.2): computes
/// `BusyPeriod`s from the account's seeded `CalendarEvent`s, so the client
/// method is testable without a live server (design §4.3).
///
/// `tooLarge` (the draft's other named error, for an unreasonably wide
/// window) is not implemented here — no test needs it yet, and there is no
/// clean way to check window width without doing the calendar-date
/// arithmetic `UtcDate`'s own doc says this crate deliberately avoids.
pub fn principal_get_availability(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: GetAvailabilityRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let allowed = account
        .principals
        .get(&request.id)
        .is_some_and(may_get_availability);
    if !allowed {
        return Err(MethodError::new("notFound"));
    }

    let mut list: Vec<BusyPeriod> = account
        .calendar_events
        .iter()
        .filter_map(|(_, event)| busy_period_for(event, &request))
        .collect();
    list.sort_by(|a, b| a.utc_start.cmp(&b.utc_start));

    to_result(&GetAvailabilityResponse { list })
}

/// A principal may be queried for availability unless its per-principal
/// `urn:ietf:params:jmap:calendars` capability explicitly says
/// `mayGetAvailability: false` (design §2.3) — absent the capability entry
/// at all, the mock allows it, mirroring how a real server would for a
/// principal it has no reason to restrict.
fn may_get_availability(principal: &Principal) -> bool {
    principal
        .capabilities
        .get(CAPABILITY_CALENDARS)
        .and_then(|value| value.get("mayGetAvailability"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// One event's `BusyPeriod`, or `None` if it doesn't count as busy in the
/// requested window. Mock simplification, same one `calendars.rs::
/// event_matches` already documents: `start`/`utcStart`/`utcEnd` are
/// compared textually, correct for the tests' `Etc/UTC` events, not for
/// arbitrary time zones.
fn busy_period_for(event: &CalendarEvent, request: &GetAvailabilityRequest) -> Option<BusyPeriod> {
    if matches!(event.status.as_deref(), Some("cancelled")) {
        return None;
    }
    // RFC 8984 §4.4.2 defaults `freeBusyStatus` to `busy`; only an explicit
    // `free` excludes an event here.
    if matches!(event.free_busy_status.as_deref(), Some("free")) {
        return None;
    }
    let start = event.start.as_deref()?;
    let window_start = request.utc_start.as_str().trim_end_matches('Z');
    let window_end = request.utc_end.as_str().trim_end_matches('Z');
    if start < window_start || start >= window_end {
        return None;
    }

    let end = busy_end(start, event.duration.as_deref());
    let busy_status = if event.status.as_deref() == Some("tentative") {
        "tentative"
    } else {
        "confirmed"
    };
    Some(BusyPeriod {
        utc_start: UtcDate::new(format!("{start}Z")),
        utc_end: UtcDate::new(format!("{end}Z")),
        busy_status: busy_status.to_owned(),
        event: request.show_details.then(|| event.clone()),
    })
}

/// Add a simple `PT<h>H<m>M<s>S` duration — the only shape
/// `CalendarEvent::duration`'s doc comment gives, e.g. `PT1H` — to a
/// `YYYY-MM-DDTHH:MM:SS` local start time.
///
/// Mock simplification: same-day second-of-day arithmetic only. `UtcDate`'s
/// own doc says this crate never does calendar arithmetic (month/leap-year/
/// DST); a duration this can't parse, or one that would cross midnight,
/// leaves the busy period zero-length (`end == start`) rather than guessing.
fn busy_end(start: &str, duration: Option<&str>) -> String {
    (|| {
        let seconds = parse_simple_duration_seconds(duration?)?;
        let (date, time) = start.split_once('T')?;
        let mut parts = time.trim_end_matches('Z').split(':');
        let hours: u32 = parts.next()?.parse().ok()?;
        let minutes: u32 = parts.next()?.parse().ok()?;
        let secs: u32 = parts.next()?.parse().ok()?;
        let end_of_day = hours * 3600 + minutes * 60 + secs + seconds;
        if end_of_day >= 86_400 {
            return None;
        }
        Some(format!(
            "{date}T{:02}:{:02}:{:02}",
            end_of_day / 3600,
            (end_of_day % 3600) / 60,
            end_of_day % 60
        ))
    })()
    .unwrap_or_else(|| start.to_owned())
}

/// Parse an ISO 8601 time-only duration (`PT` followed by any of `<n>H`,
/// `<n>M`, `<n>S`) into whole seconds. `None` on anything else, including a
/// date component (`P<n>D...`) — this mock only ever sees the time-only form
/// `CalendarEvent::simple`'s callers write.
fn parse_simple_duration_seconds(duration: &str) -> Option<u32> {
    let rest = duration.strip_prefix("PT")?;
    let mut seconds: u32 = 0;
    let mut number = String::new();
    for ch in rest.chars() {
        match ch {
            '0'..='9' => number.push(ch),
            'H' | 'M' | 'S' => {
                let value: u32 = number.parse().ok()?;
                number.clear();
                seconds += match ch {
                    'H' => value.checked_mul(3600)?,
                    'M' => value.checked_mul(60)?,
                    _ => value,
                };
            }
            _ => return None,
        }
    }
    if !number.is_empty() {
        return None;
    }
    Some(seconds)
}

fn principal_matches(principal: &Principal, filter: &PrincipalQueryFilter) -> bool {
    if let Some(name) = &filter.name
        && !principal.name.contains(name.as_str())
    {
        return false;
    }
    if let Some(email) = &filter.email
        && principal.email.as_deref() != Some(email.as_str())
    {
        return false;
    }
    if let Some(text) = &filter.text {
        let matches_name = principal.name.contains(text.as_str());
        let matches_email = principal
            .email
            .as_deref()
            .is_some_and(|email| email.contains(text.as_str()));
        if !(matches_name || matches_email) {
            return false;
        }
    }
    true
}

impl AccountState {
    /// Seed a principal; returns its id. Does not bump state.
    pub fn seed_principal(&mut self, principal: Principal) -> Id {
        let id = self.principals.alloc_id();
        let principal = Principal {
            id: Some(id.clone()),
            ..principal
        };
        self.principals.seed_with_id(id.clone(), principal);
        id
    }

    /// Seed a principal and make it the account's `currentUserPrincipalId`
    /// (RFC 9670 §2.5) — the common case, since most tests only need one
    /// principal representing the account owner. Tests that need more than
    /// one (e.g. an attendee to resolve via `Principal/query`) call
    /// [`Self::seed_principal`] for the rest.
    pub fn seed_current_user_principal(&mut self, principal: Principal) -> Id {
        let id = self.seed_principal(principal);
        self.current_user_principal_id = Some(id.clone());
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hours_minutes_and_seconds() {
        assert_eq!(parse_simple_duration_seconds("PT1H"), Some(3600));
        assert_eq!(parse_simple_duration_seconds("PT30M"), Some(1800));
        assert_eq!(parse_simple_duration_seconds("PT1H30M15S"), Some(5415));
        assert_eq!(parse_simple_duration_seconds("PT0S"), Some(0));
    }

    #[test]
    fn rejects_a_date_component_or_garbage() {
        assert_eq!(parse_simple_duration_seconds("P1DT1H"), None);
        assert_eq!(parse_simple_duration_seconds("bogus"), None);
        assert_eq!(parse_simple_duration_seconds("PT1H30"), None);
    }

    #[test]
    fn busy_end_adds_the_duration_within_the_same_day() {
        assert_eq!(
            busy_end("2026-09-01T09:00:00", Some("PT1H")),
            "2026-09-01T10:00:00"
        );
        assert_eq!(
            busy_end("2026-09-01T09:00:00", Some("PT30M")),
            "2026-09-01T09:30:00"
        );
    }

    #[test]
    fn busy_end_falls_back_to_start_when_it_cannot_compute_an_end() {
        assert_eq!(busy_end("2026-09-01T09:00:00", None), "2026-09-01T09:00:00");
        assert_eq!(
            busy_end("2026-09-01T09:00:00", Some("garbage")),
            "2026-09-01T09:00:00"
        );
        // Would cross midnight — the mock doesn't do calendar arithmetic.
        assert_eq!(
            busy_end("2026-09-01T23:30:00", Some("PT1H")),
            "2026-09-01T23:30:00"
        );
    }
}
