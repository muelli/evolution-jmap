// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal handlers and seed helpers for `Principal/*` and `ShareNotification/*`.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarRights};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use jmap_proto::principals::{
    BusyPeriod, GetAvailabilityRequest, GetAvailabilityResponse, Principal, PrincipalQueryFilter,
    ShareNotification, ShareNotificationQueryFilter,
};
use jmap_proto::session::CAPABILITY_CALENDARS;
use jmap_proto::state::UtcDate;
use serde::Serialize;
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::state::AccountState;

/// Use a fixed stand-in for "now" to keep tests reproducible.
const MOCK_NOW: &str = "2026-01-01T00:00:00Z";

pub fn principal_get(
    state: &mut crate::state::ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let Some(account) = state.account(&request.account_id) else {
        return Err(MethodError::new(
            jmap_proto::error::method::ACCOUNT_NOT_FOUND,
        ));
    };
    let account_id = request.account_id.clone();
    let account_state = account.principals.state();

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
                if let Some(principal) = account.principals.get(id) {
                    list.push(principal.clone());
                } else if account.current_user_principal_id.as_ref() == Some(id) {
                    let p = account
                        .principals
                        .iter()
                        .next()
                        .map(|(_, p)| {
                            let mut p = p.clone();
                            p.id = Some(id.clone());
                            p
                        })
                        .unwrap_or_else(|| {
                            let mut p = Principal::new(&account.name);
                            p.id = Some(id.clone());
                            p.email = Some(account.name.clone());
                            p.principal_type =
                                Some(jmap_proto::principals::principal_type::INDIVIDUAL.to_owned());
                            p
                        });
                    list.push(p);
                } else if let Some(foreign) = find_foreign_principal(state, &account_id, caller, id)
                {
                    list.push(foreign);
                } else {
                    not_found.push(id.clone());
                }
            }
        }
    }

    to_result(&GetResponse::new(account_id, account_state, list).with_not_found(not_found))
}

/// Handle `Principal/set` (RFC 9670 §2.3).
/// Only update is supported; create and destroy are rejected with `forbidden`.
pub fn principal_set(
    state: &mut crate::state::ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: SetRequest<Principal> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;
    let viewer = caller
        .or(account.current_user_principal_id.as_ref())
        .cloned();
    let old_state = account.principals.state();
    if let Some(expected) = &request.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    for creation_id in request.create.unwrap_or_default().into_keys() {
        not_created.insert(
            creation_id,
            SetError::new(error::set::FORBIDDEN)
                .with_description("Principal objects cannot be created via Principal/set"),
        );
    }

    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();
    for id in request.destroy.unwrap_or_default() {
        not_destroyed.insert(
            id,
            SetError::new(error::set::FORBIDDEN)
                .with_description("Principal objects cannot be destroyed via Principal/set"),
        );
    }

    let mut updated: BTreeMap<Id, Option<Principal>> = BTreeMap::new();
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();

    for (id, patch) in request.update.unwrap_or_default() {
        let is_self =
            viewer.as_ref() == Some(&id) || account.current_user_principal_id.as_ref() == Some(&id);
        let existing = account.principals.get(&id).cloned().or_else(|| {
            if is_self {
                let mut p = Principal::new(&account.name);
                p.id = Some(id.clone());
                p.email = Some(account.name.clone());
                p.principal_type =
                    Some(jmap_proto::principals::principal_type::INDIVIDUAL.to_owned());
                Some(p)
            } else {
                None
            }
        });

        let Some(existing) = existing else {
            not_updated.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        };

        let Some(patch_map) = patch.as_object() else {
            not_updated.insert(id, SetError::new(error::set::INVALID_PATCH));
            continue;
        };

        let mut value = serde_json::to_value(&existing).map_err(|e| {
            MethodError::new(error::method::SERVER_FAIL).with_description(e.to_string())
        })?;

        match crate::patch::apply_patch(&mut value, patch_map) {
            Ok(()) => match serde_json::from_value::<Principal>(value) {
                Ok(mut patched) => {
                    patched.id = Some(id.clone());
                    account.principals.seed_with_id(id.clone(), patched);
                    account.principals.state();
                    updated.insert(id, None);
                }
                Err(e) => {
                    not_updated.insert(
                        id,
                        SetError::new(error::set::INVALID_PATCH).with_description(e.to_string()),
                    );
                }
            },
            Err(msg) => {
                not_updated.insert(
                    id,
                    SetError::new(error::set::INVALID_PATCH).with_description(msg),
                );
            }
        }
    }

    let mut response =
        SetResponse::<Principal>::new(request.account_id, account.principals.state())
            .with_old_state(old_state);
    if !updated.is_empty() {
        response = response.with_updated(updated);
    }
    if !not_updated.is_empty() {
        response = response.with_not_updated(not_updated);
    }
    if !not_created.is_empty() {
        response = response.with_not_created(not_created);
    }
    if !not_destroyed.is_empty() {
        response = response.with_not_destroyed(not_destroyed);
    }
    to_result(&response)
}

fn find_foreign_principal(
    state: &crate::state::ServerState,
    requesting_account_id: &Id,
    caller: Option<&Id>,
    target_id: &Id,
) -> Option<Principal> {
    for (acc_id, acc) in &state.accounts {
        if acc_id == requesting_account_id {
            continue;
        }
        let matches_principal = acc.current_user_principal_id.as_ref() == Some(target_id)
            || acc.principals.contains(target_id);
        if !matches_principal {
            continue;
        }

        let is_visible = !acc.is_personal
            || caller.is_some_and(|c| {
                acc.current_user_principal_id.as_ref() == Some(c)
                    || acc
                        .mailboxes
                        .iter()
                        .any(|(_, m)| m.share_with.as_ref().is_some_and(|s| s.contains_key(c)))
                    || acc
                        .calendars
                        .iter()
                        .any(|(_, cal)| cal.share_with.as_ref().is_some_and(|s| s.contains_key(c)))
                    || acc
                        .address_books
                        .iter()
                        .any(|(_, b)| b.share_with.as_ref().is_some_and(|s| s.contains_key(c)))
            });

        if is_visible {
            if let Some(p) = acc.principals.get(target_id) {
                return Some(p.clone());
            }
            let mut p = Principal::new(&acc.name);
            p.id = Some(target_id.clone());
            p.email = Some(acc.name.clone());
            p.principal_type = Some(jmap_proto::principals::principal_type::INDIVIDUAL.to_owned());
            return Some(p);
        }
    }
    None
}

pub fn principal_query(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    if state.disable_principal_query {
        return Err(MethodError::new("forbidden")
            .with_description("The administrator has disabled directory queries."));
    }
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

    let mut response = QueryResponse::new(request.account_id, account.principals.state(), ids)
        .with_position(request.position.max(0) as u64);
    if request.calculate_total {
        response = response.with_total(total);
    }
    to_result(&response)
}

/// Handle `Principal/getAvailability` over seeded `CalendarEvent` values.
/// Omit draft `tooLarge`.
pub fn principal_get_availability(
    state: &mut crate::state::ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: GetAvailabilityRequest = parse_arguments(arguments)?;
    let Some(requesting_account) = state.account(&request.account_id) else {
        return Err(MethodError::new(
            jmap_proto::error::method::ACCOUNT_NOT_FOUND,
        ));
    };
    let caller = caller.or(requesting_account.current_user_principal_id.as_ref());

    let target = state.accounts.iter().find(|(_, acc)| {
        acc.current_user_principal_id.as_ref() == Some(&request.id)
            || acc.principals.contains(&request.id)
    });

    let Some((target_account_id, target_account)) = target else {
        return Err(MethodError::new("notFound"));
    };

    if target_account
        .principals
        .get(&request.id)
        .is_some_and(|p| !may_get_availability(p))
    {
        return Err(MethodError::new("notFound"));
    }

    let is_owner = match caller {
        Some(c) => match &target_account.current_user_principal_id {
            Some(owner_id) => c == owner_id,
            None => target_account_id == &request.account_id,
        },
        None => target_account_id == &request.account_id,
    };

    let mut list: Vec<BusyPeriod> = Vec::new();

    if is_owner {
        for (_, event) in target_account.calendar_events.iter() {
            if let Some(period) = busy_period_for(event, &request, true) {
                list.push(period);
            }
        }
    } else if let Some(caller_id) = caller {
        let granted_calendars: BTreeMap<&Id, &CalendarRights> = target_account
            .calendars
            .iter()
            .filter_map(|(cal_id, cal)| {
                let rights = cal.share_with.as_ref()?.get(caller_id)?;
                let has_fb =
                    rights.may_read_free_busy == Some(true) || rights.may_read_items == Some(true);
                has_fb.then_some((cal_id, rights))
            })
            .collect();

        if granted_calendars.is_empty() {
            return to_result(&GetAvailabilityResponse::new(Vec::new()));
        }

        for (_, event) in target_account.calendar_events.iter() {
            let Some(cal_ids) = &event.calendar_ids else {
                continue;
            };
            let matching_rights = cal_ids.keys().find_map(|cid| granted_calendars.get(cid));
            let Some(rights) = matching_rights else {
                continue;
            };

            let can_read_items = rights.may_read_items == Some(true);
            if let Some(period) = busy_period_for(event, &request, can_read_items) {
                list.push(period);
            }
        }
    } else {
        return to_result(&GetAvailabilityResponse::new(Vec::new()));
    }

    list.sort_by(|a, b| a.utc_start.cmp(&b.utc_start));

    to_result(&GetAvailabilityResponse::new(list))
}

/// Return `true` when `mayGetAvailability` is absent.
fn may_get_availability(principal: &Principal) -> bool {
    principal
        .capabilities
        .get(CAPABILITY_CALENDARS)
        .and_then(|value| value.get("mayGetAvailability"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Build one `BusyPeriod` for an event.
/// Return `None` when the event is not busy in the request window or is secret.
fn busy_period_for(
    event: &CalendarEvent,
    request: &GetAvailabilityRequest,
    can_read_items: bool,
) -> Option<BusyPeriod> {
    if matches!(event.status.as_deref(), Some("cancelled")) {
        return None;
    }
    // RFC 8984 section 4.4.2 sets default `freeBusyStatus` to `busy`.
    // Only explicit `free` excludes an event here.
    if matches!(event.free_busy_status.as_deref(), Some("free")) {
        return None;
    }
    // RFC 8984 section 4.4.3: secret events are completely hidden from availability.
    if matches!(event.privacy.as_deref(), Some("secret")) {
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
    let mut period = BusyPeriod::new(format!("{start}Z"), format!("{end}Z"), busy_status);
    if request.show_details && can_read_items {
        if matches!(event.privacy.as_deref(), Some("private")) {
            let mut redacted = event.clone();
            redacted.title = None;
            redacted.description = None;
            period = period.with_event(redacted);
        } else {
            period = period.with_event(event.clone());
        }
    }
    Some(period)
}

/// Add `PT<h>H<m>M<s>S` to `YYYY-MM-DDTHH:MM:SS` with same-day arithmetic.
/// Return `start` if parsing fails or if the end crosses midnight.
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

/// Parse ISO 8601 time-only durations (`PT...`) into seconds.
/// Return `None` for unsupported forms.
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

/// Handle `ShareNotification/get` for the caller-resolved recipient id.
pub fn share_notification_get(
    state: &mut crate::state::ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;
    let viewer = caller.or(account.current_user_principal_id.as_ref());

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => {
            for (_, (recipient, notification)) in account.share_notifications.iter() {
                if Some(recipient) == viewer {
                    list.push(notification.clone());
                }
            }
        }
        Some(ids) => {
            for id in ids {
                match account.share_notifications.get(id) {
                    Some((recipient, notification)) if Some(recipient) == viewer => {
                        list.push(notification.clone());
                    }
                    _ => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(
        &GetResponse::new(
            request.account_id,
            account.share_notifications.state(),
            list,
        )
        .with_not_found(not_found),
    )
}

/// Handle `ShareNotification/set` (RFC 9670 §3.3).
/// Only destroy is supported to dismiss notifications.
pub fn share_notification_set(
    state: &mut crate::state::ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: SetRequest<ShareNotification> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;
    let viewer = caller
        .or(account.current_user_principal_id.as_ref())
        .cloned();

    let old_state = account.share_notifications.state();
    if let Some(expected) = &request.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    for creation_id in request.create.unwrap_or_default().into_keys() {
        not_created.insert(
            creation_id,
            SetError::new(error::set::FORBIDDEN)
                .with_description("ShareNotification objects are server-created"),
        );
    }
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    for id in request.update.unwrap_or_default().into_keys() {
        not_updated.insert(
            id,
            SetError::new(error::set::FORBIDDEN)
                .with_description("ShareNotification objects cannot be updated"),
        );
    }

    let mut destroyed: Vec<Id> = Vec::new();
    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();
    account.share_notifications.transaction(|txn| {
        for id in request.destroy.unwrap_or_default() {
            let visible = txn
                .get(&id)
                .is_some_and(|(recipient, _)| Some(recipient) == viewer.as_ref());
            if visible && txn.destroy(&id) {
                destroyed.push(id);
            } else {
                not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
            }
        }
    });

    let mut response = SetResponse::<ShareNotification>::new(
        request.account_id,
        account.share_notifications.state(),
    )
    .with_old_state(old_state);
    if !destroyed.is_empty() {
        response = response.with_destroyed(destroyed);
    }
    if !not_destroyed.is_empty() {
        response = response.with_not_destroyed(not_destroyed);
    }
    if !not_created.is_empty() {
        response = response.with_not_created(not_created);
    }
    if !not_updated.is_empty() {
        response = response.with_not_updated(not_updated);
    }
    to_result(&response)
}

/// Handle `ShareNotification/query` with [`share_notification_get`] visibility.
/// Then filter by `objectType`, `objectAccountId`, `before`, and `after`.
pub fn share_notification_query(
    state: &mut crate::state::ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: QueryRequest<ShareNotificationQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;
    let viewer = caller.or(account.current_user_principal_id.as_ref());
    let filter = request.filter.unwrap_or_default();

    let visible = |recipient: &Id, notification: &ShareNotification| {
        Some(recipient) == viewer && share_notification_matches(notification, &filter)
    };

    let mut matching: Vec<(&Id, &ShareNotification)> = account
        .share_notifications
        .iter()
        .filter_map(|(id, (recipient, notification))| {
            visible(recipient, notification).then_some((id, notification))
        })
        .collect();

    if let Some(sort) = &request.sort {
        for comparator in sort.iter().rev() {
            let ascending = comparator.is_ascending;
            match comparator.property.as_str() {
                "created" => {
                    matching.sort_by(|(_, a), (_, b)| {
                        let ord = a.created.cmp(&b.created);
                        if ascending { ord } else { ord.reverse() }
                    });
                }
                other => {
                    return Err(
                        MethodError::new("unsupportedSort").with_description(format!(
                            "mock cannot sort share notifications by {other}"
                        )),
                    );
                }
            }
        }
    }

    let total = matching.len() as u64;
    let ids: Vec<Id> = matching
        .into_iter()
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let mut response =
        QueryResponse::new(request.account_id, account.share_notifications.state(), ids)
            .with_position(request.position.max(0) as u64);
    if request.calculate_total {
        response = response.with_total(total);
    }
    to_result(&response)
}

fn share_notification_matches(
    notification: &ShareNotification,
    filter: &ShareNotificationQueryFilter,
) -> bool {
    if let Some(after) = &filter.after
        && notification.created.as_str() < after.as_str()
    {
        return false;
    }
    if let Some(before) = &filter.before
        && notification.created.as_str() >= before.as_str()
    {
        return false;
    }
    if let Some(object_type) = &filter.object_type
        && &notification.object_type != object_type
    {
        return false;
    }
    if let Some(object_account_id) = &filter.object_account_id
        && &notification.object_account_id != object_account_id
    {
        return false;
    }
    true
}

/// Record `ShareNotification` entries when rights change in `AddressBook/set` or `Mailbox/set`.
pub(crate) fn record_share_changes<R>(
    account: &mut AccountState,
    object_type: &str,
    object_id: &Id,
    object_account_id: &Id,
    old: Option<&BTreeMap<Id, R>>,
    new: Option<&BTreeMap<Id, R>>,
) where
    R: Clone + PartialEq + Serialize,
{
    let mut recipients: BTreeSet<Id> = BTreeSet::new();
    if let Some(map) = old {
        recipients.extend(map.keys().cloned());
    }
    if let Some(map) = new {
        recipients.extend(map.keys().cloned());
    }

    let mut to_create: Vec<(Id, ShareNotification)> = Vec::new();
    for recipient in recipients {
        let old_rights = old.and_then(|map| map.get(&recipient));
        let new_rights = new.and_then(|map| map.get(&recipient));
        if old_rights == new_rights {
            continue;
        }
        let mut notification = ShareNotification::new(
            UtcDate::new(MOCK_NOW),
            object_type,
            object_id.clone(),
            object_account_id.clone(),
        );
        if let Some(rights) = old_rights {
            notification =
                notification.with_old_rights(serde_json::to_value(rights).unwrap_or(Value::Null));
        }
        if let Some(rights) = new_rights {
            notification =
                notification.with_new_rights(serde_json::to_value(rights).unwrap_or(Value::Null));
        }
        to_create.push((recipient, notification));
    }

    if !to_create.is_empty() {
        account.share_notifications.transaction(|txn| {
            for (recipient, notification) in to_create {
                let id = txn.alloc_id();
                txn.create(id.clone(), (recipient, notification.with_id(id)));
            }
        });
    }
}

fn principal_matches(principal: &Principal, filter: &PrincipalQueryFilter) -> bool {
    if let Some(account_ids) = &filter.account_ids {
        let has_matching_account = principal
            .accounts
            .as_ref()
            .is_some_and(|accounts| account_ids.iter().any(|aid| accounts.contains_key(aid)));
        if !has_matching_account {
            return false;
        }
    }
    if let Some(principal_type) = &filter.principal_type
        && principal.principal_type.as_deref() != Some(principal_type.as_str())
    {
        return false;
    }
    if let Some(time_zone) = &filter.time_zone
        && principal.time_zone.as_deref() != Some(time_zone.as_str())
    {
        return false;
    }
    if let Some(name) = &filter.name
        && !principal.name.contains(name.as_str())
    {
        return false;
    }
    if let Some(email) = &filter.email
        && !principal
            .email
            .as_deref()
            .is_some_and(|e| e.contains(email.as_str()))
    {
        return false;
    }
    if let Some(text) = &filter.text {
        let matches_name = principal.name.contains(text.as_str());
        let matches_email = principal
            .email
            .as_deref()
            .is_some_and(|email| email.contains(text.as_str()));
        let matches_desc = principal
            .description
            .as_deref()
            .is_some_and(|desc| desc.contains(text.as_str()));
        if !(matches_name || matches_email || matches_desc) {
            return false;
        }
    }
    true
}

impl AccountState {
    /// Seed a principal and return its id without a state bump.
    pub fn seed_principal(&mut self, principal: Principal) -> Id {
        let id = self.principals.alloc_id();
        let principal = principal.with_id(id.clone());
        self.principals.seed_with_id(id.clone(), principal);
        id
    }

    /// Seed a principal, set `currentUserPrincipalId`, and return its id.
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
        // This mock arithmetic does not cross midnight.
        assert_eq!(
            busy_end("2026-09-01T23:30:00", Some("PT1H")),
            "2026-09-01T23:30:00"
        );
    }
}
