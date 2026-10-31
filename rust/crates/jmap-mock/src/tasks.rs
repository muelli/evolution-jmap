// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Task methods (`TaskList/get|set|query`, `Task/get|set|query`).

use std::collections::BTreeSet;

use jmap_proto::Id;
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest};
use jmap_proto::tasks::{Task, TaskList, TaskQueryFilter, task_set_error};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::setops::simple_set_with_validation;
use crate::state::ServerState;

pub fn task_list_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => {
            list.extend(
                account
                    .task_lists
                    .iter()
                    .map(|(_, task_list)| task_list.clone()),
            );
        }
        Some(ids) => {
            for id in ids {
                match account.task_lists.get(id) {
                    Some(task_list) => list.push(task_list.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(
        &GetResponse::new(request.account_id, account.task_lists.state(), list)
            .with_not_found(not_found),
    )
}

pub fn task_list_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<TaskList> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let referenced_task_list_ids: BTreeSet<Id> = account
        .tasks
        .iter()
        .filter_map(|(_, task)| task.task_list_id.clone())
        .collect();

    let response = simple_set_with_validation(
        &mut account.task_lists,
        request,
        |id, task_list| {
            validate_task_list_name(task_list)?;
            task_list.id = Some(id.clone());
            Ok(())
        },
        |_id, _before, patched| validate_task_list_name(patched),
        |id, _existing| {
            if referenced_task_list_ids.contains(id) {
                return Err(SetError::new(error::set::FORBIDDEN)
                    .with_description("task list still has tasks"));
            }
            Ok(())
        },
    )?;

    to_result(&response)
}

pub fn task_list_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<Value> = parse_arguments(arguments)?;
    let paging = state.query_paging();
    let account = account_mut(state, &request.account_id)?;

    let (ids, limit) = paging.page(
        account.task_lists.iter().map(|(id, _)| id.clone()),
        request.position,
        request.limit,
    );
    let total = account.task_lists.len() as u64;

    let mut response = QueryResponse::new(request.account_id, account.task_lists.state(), ids)
        .with_position(request.position.max(0) as u64);
    if request.calculate_total {
        response = response.with_total(total);
    }
    if let Some(limit) = limit {
        response = response.with_limit(limit);
    }
    to_result(&response)
}

pub fn task_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => {
            list.extend(account.tasks.iter().map(|(_, task)| task.clone()));
        }
        Some(ids) => {
            for id in ids {
                match account.tasks.get(id) {
                    Some(task) => list.push(task.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(
        &GetResponse::new(request.account_id, account.tasks.state(), list)
            .with_not_found(not_found),
    )
}

pub fn task_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<Task> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let known_task_list_ids: BTreeSet<Id> = account
        .task_lists
        .iter()
        .map(|(task_list_id, _)| task_list_id.clone())
        .collect();

    let response = simple_set_with_validation(
        &mut account.tasks,
        request,
        |id, task| {
            validate_task_list_reference(task, &known_task_list_ids)?;

            if task.uid.trim().is_empty() {
                task.uid = format!("urn:example:task:{}", id.as_str());
            }
            if task.task_type.is_none() {
                task.task_type = Some("Task".to_owned());
            }
            task.id = Some(id.clone());
            Ok(())
        },
        |_id, _before, patched| validate_task_list_reference(patched, &known_task_list_ids),
        |_id, _existing| Ok(()),
    )?;

    to_result(&response)
}

pub fn task_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<TaskQueryFilter> = parse_arguments(arguments)?;
    let paging = state.query_paging();
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let (ids, limit) = paging.page(
        account
            .tasks
            .iter()
            .filter(|(_, task)| task_matches(task, &filter))
            .map(|(id, _)| id.clone()),
        request.position,
        request.limit,
    );

    let total = account
        .tasks
        .iter()
        .filter(|(_, task)| task_matches(task, &filter))
        .count() as u64;

    let mut response = QueryResponse::new(request.account_id, account.tasks.state(), ids)
        .with_position(request.position.max(0) as u64);
    if request.calculate_total {
        response = response.with_total(total);
    }
    if let Some(limit) = limit {
        response = response.with_limit(limit);
    }
    to_result(&response)
}

fn validate_task_list_name(task_list: &TaskList) -> Result<(), SetError> {
    if task_list.name.trim().is_empty() {
        return Err(SetError::new(error::set::INVALID_PROPERTIES)
            .with_description("name is required")
            .with_properties(["name"]));
    }
    Ok(())
}

fn validate_task_list_reference(
    task: &Task,
    known_task_list_ids: &BTreeSet<Id>,
) -> Result<(), SetError> {
    let Some(task_list_id) = task.task_list_id.as_ref() else {
        return Err(SetError::new(error::set::INVALID_PROPERTIES)
            .with_description("taskListId is required")
            .with_properties(["taskListId"]));
    };

    if !known_task_list_ids.contains(task_list_id) {
        return Err(SetError::new(task_set_error::TASK_LIST_NOT_FOUND)
            .with_description(format!("task list {task_list_id} does not exist")));
    }

    Ok(())
}

fn task_matches(task: &Task, filter: &TaskQueryFilter) -> bool {
    if let Some(task_list_id) = &filter.task_list_id
        && task.task_list_id.as_ref() != Some(task_list_id)
    {
        return false;
    }

    if let Some(status) = &filter.status
        && task.status.as_ref() != Some(status)
    {
        return false;
    }

    if let Some(before) = &filter.before
        && task.start.as_ref().is_none_or(|start| start >= before)
    {
        return false;
    }

    if let Some(after) = &filter.after
        && task.start.as_ref().is_none_or(|start| start <= after)
    {
        return false;
    }

    if let Some(due_before) = &filter.due_before
        && task.due.as_ref().is_none_or(|due| due >= due_before)
    {
        return false;
    }

    if let Some(due_after) = &filter.due_after
        && task.due.as_ref().is_none_or(|due| due <= due_after)
    {
        return false;
    }

    if let Some(completed_before) = &filter.completed_before
        && task
            .completed
            .as_ref()
            .is_none_or(|completed| completed >= completed_before)
    {
        return false;
    }

    if let Some(completed_after) = &filter.completed_after
        && task
            .completed
            .as_ref()
            .is_none_or(|completed| completed <= completed_after)
    {
        return false;
    }

    if let Some(has_recurrence) = filter.has_recurrence
        && task.recurrence_rule.is_some() != has_recurrence
    {
        return false;
    }

    if let Some(text) = &filter.text
        && !task_matches_text(task, text)
    {
        return false;
    }

    true
}

fn task_matches_text(task: &Task, text: &str) -> bool {
    let needle = text.to_ascii_lowercase();

    if task
        .title
        .as_deref()
        .is_some_and(|title| contains_case_insensitive(title, &needle))
    {
        return true;
    }

    if task
        .description
        .as_deref()
        .is_some_and(|description| contains_case_insensitive(description, &needle))
    {
        return true;
    }

    task.locations.as_ref().is_some_and(|locations| {
        locations.values().any(|location| {
            location
                .name
                .as_deref()
                .is_some_and(|name| contains_case_insensitive(name, &needle))
                || location
                    .description
                    .as_deref()
                    .is_some_and(|description| contains_case_insensitive(description, &needle))
        })
    })
}

fn contains_case_insensitive(value: &str, needle: &str) -> bool {
    value.to_ascii_lowercase().contains(needle)
}
