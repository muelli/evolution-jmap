// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::calendars::{
    Alert, EventRelation, Location, OffsetTrigger, RecurrenceRule, VirtualLocation,
};
use jmap_proto::id::Id;
use jmap_proto::state::UtcDate;
use jmap_proto::tasks::{
    CAPABILITY_TASKS, Task, TaskList, TaskListRights, TaskQueryFilter, TasksCapability,
    task_progress, task_set_error, task_status,
};
use serde_json::json;

#[test]
fn task_list_rights_is_writable_logic_and_permissions() {
    let mut rights = TaskListRights::default();
    assert!(!rights.is_writable());

    // When only may_write_all is true, is_writable must return true.
    rights.may_write_all = Some(true);
    rights.may_write_own = Some(false);
    assert!(rights.is_writable());

    // When only may_write_own is true, is_writable must return true.
    rights.may_write_all = Some(false);
    rights.may_write_own = Some(true);
    assert!(rights.is_writable());

    // When neither is true, is_writable must return false.
    rights.may_write_all = Some(false);
    rights.may_write_own = Some(false);
    assert!(!rights.is_writable());

    // When both are None, is_writable must return false.
    rights.may_write_all = None;
    rights.may_write_own = None;
    assert!(!rights.is_writable());

    let all_rights = TaskListRights::all();
    assert_eq!(all_rights.may_read_items, Some(true));
    assert_eq!(all_rights.may_write_all, Some(true));
    assert_eq!(all_rights.may_write_own, Some(true));
    assert_eq!(all_rights.may_update_private, Some(true));
    assert_eq!(all_rights.may_reread, Some(true));
    assert_eq!(all_rights.may_admin, Some(true));
    assert_eq!(all_rights.may_delete, Some(true));
    assert!(all_rights.is_writable());

    let ro_rights = TaskListRights::read_only();
    assert_eq!(ro_rights.may_read_items, Some(true));
    assert_eq!(ro_rights.may_write_all, Some(false));
    assert_eq!(ro_rights.may_write_own, Some(false));
    assert_eq!(ro_rights.may_update_private, Some(false));
    assert_eq!(ro_rights.may_reread, Some(true));
    assert_eq!(ro_rights.may_admin, Some(false));
    assert_eq!(ro_rights.may_delete, Some(false));
    assert!(!ro_rights.is_writable());
}

#[test]
fn task_list_rights_with_extra_preserves_rights_and_extra_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customVendorRight".to_string(), json!(true));
    extra.insert("auditLevel".to_string(), json!("high"));

    let rights = TaskListRights::all().with_extra(extra);

    // Assert that standard rights fields are preserved rather than defaulted to None.
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_write_all, Some(true));
    assert_eq!(rights.may_write_own, Some(true));
    assert_eq!(rights.may_update_private, Some(true));
    assert_eq!(rights.may_reread, Some(true));
    assert_eq!(rights.may_admin, Some(true));
    assert_eq!(rights.may_delete, Some(true));
    assert!(rights.is_writable());

    assert_eq!(rights.extra.get("customVendorRight"), Some(&json!(true)));
    assert_eq!(rights.extra.get("auditLevel"), Some(&json!("high")));

    let val = serde_json::to_value(&rights).expect("serialize TaskListRights");
    assert_eq!(val["mayReadItems"], true);
    assert_eq!(val["mayWriteAll"], true);
    assert_eq!(val["customVendorRight"], true);
    assert_eq!(val["auditLevel"], "high");

    let back: TaskListRights = serde_json::from_value(val).expect("deserialize TaskListRights");
    assert_eq!(back, rights);
}

#[test]
fn task_with_extra_preserves_task_fields_and_extra_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customTaskFlag".to_string(), json!("flagValue"));
    extra.insert("customCostCenter".to_string(), json!(9001));

    let task = Task::new("uid-task-preservation-999")
        .with_id("t-999")
        .with_task_list_id("tl-100")
        .with_title("Comprehensive Task")
        .with_description("Description of comprehensive task")
        .with_due("2026-12-31T23:59:59")
        .with_start("2026-12-01T00:00:00")
        .with_time_zone("UTC")
        .with_status(task_status::IN_PROCESS)
        .with_progress(task_progress::IN_PROCESS)
        .with_percent_complete(75)
        .with_priority(2)
        .with_color("#4A90E2")
        .show_without_time(true)
        .with_extra(extra);

    // Assert that Task fields are preserved and not replaced by Default::default().
    assert_eq!(task.task_type, Some("Task".to_string()));
    assert_eq!(task.uid, "uid-task-preservation-999");
    assert_eq!(task.id, Some(Id::from("t-999")));
    assert_eq!(task.task_list_id, Some(Id::from("tl-100")));
    assert_eq!(task.title, Some("Comprehensive Task".to_string()));
    assert_eq!(
        task.description,
        Some("Description of comprehensive task".to_string())
    );
    assert_eq!(task.due, Some("2026-12-31T23:59:59".to_string()));
    assert_eq!(task.start, Some("2026-12-01T00:00:00".to_string()));
    assert_eq!(task.time_zone, Some("UTC".to_string()));
    assert_eq!(task.status, Some(task_status::IN_PROCESS.to_string()));
    assert_eq!(task.progress, Some(task_progress::IN_PROCESS.to_string()));
    assert_eq!(task.percent_complete, Some(75));
    assert_eq!(task.priority, Some(2));
    assert_eq!(task.color, Some("#4A90E2".to_string()));
    assert_eq!(task.show_without_time, Some(true));

    assert_eq!(task.extra.get("customTaskFlag"), Some(&json!("flagValue")));
    assert_eq!(task.extra.get("customCostCenter"), Some(&json!(9001)));

    let val = serde_json::to_value(&task).expect("serialize Task");
    assert_eq!(val["@type"], "Task");
    assert_eq!(val["uid"], "uid-task-preservation-999");
    assert_eq!(val["id"], "t-999");
    assert_eq!(val["taskListId"], "tl-100");
    assert_eq!(val["title"], "Comprehensive Task");
    assert_eq!(val["customTaskFlag"], "flagValue");
    assert_eq!(val["customCostCenter"], 9001);

    let back: Task = serde_json::from_value(val).expect("deserialize Task");
    assert_eq!(back, task);
}

#[test]
fn task_query_filter_with_extra_preserves_filter_criteria() {
    let mut extra = BTreeMap::new();
    extra.insert("customVendorFilter".to_string(), json!("vendorCriteria"));
    extra.insert("indexScanPreference".to_string(), json!("btree"));

    let filter = TaskQueryFilter::new()
        .with_task_list_id("tl-personal")
        .with_text("urgent deadline")
        .with_status(task_status::NEEDS_ACTION)
        .with_due_before("2026-11-01T00:00:00")
        .with_has_recurrence(false)
        .with_extra(extra);

    // Assert that filter criteria are preserved and not replaced by Default::default().
    assert_eq!(filter.task_list_id, Some(Id::from("tl-personal")));
    assert_eq!(filter.text, Some("urgent deadline".to_string()));
    assert_eq!(filter.status, Some(task_status::NEEDS_ACTION.to_string()));
    assert_eq!(filter.due_before, Some("2026-11-01T00:00:00".to_string()));
    assert_eq!(filter.has_recurrence, Some(false));

    assert_eq!(
        filter.extra.get("customVendorFilter"),
        Some(&json!("vendorCriteria"))
    );
    assert_eq!(
        filter.extra.get("indexScanPreference"),
        Some(&json!("btree"))
    );

    let val = serde_json::to_value(&filter).expect("serialize TaskQueryFilter");
    assert_eq!(val["taskListId"], "tl-personal");
    assert_eq!(val["text"], "urgent deadline");
    assert_eq!(val["status"], "needs-action");
    assert_eq!(val["dueBefore"], "2026-11-01T00:00:00");
    assert_eq!(val["hasRecurrence"], false);
    assert_eq!(val["customVendorFilter"], "vendorCriteria");
    assert_eq!(val["indexScanPreference"], "btree");

    let back: TaskQueryFilter = serde_json::from_value(val).expect("deserialize TaskQueryFilter");
    assert_eq!(back, filter);
}

#[test]
fn task_list_builders_and_properties_roundtrip() {
    let mut share_with = BTreeMap::new();
    share_with.insert(Id::from("assistant-user"), TaskListRights::read_only());

    let mut extra = BTreeMap::new();
    extra.insert("vendorSyncToken".to_string(), json!("sync-1234"));

    let list = TaskList::new("Work Projects")
        .with_id("tl-work")
        .with_color("#008080")
        .with_sort_order(5)
        .is_default(false)
        .is_subscribed(true)
        .with_share_with(share_with.clone())
        .with_my_rights(TaskListRights::all())
        .with_extra(extra);

    assert_eq!(list.name, "Work Projects");
    assert_eq!(list.id, Some(Id::from("tl-work")));
    assert_eq!(list.color, Some("#008080".to_string()));
    assert_eq!(list.sort_order, Some(5));
    assert_eq!(list.is_default, Some(false));
    assert_eq!(list.is_subscribed, Some(true));
    assert_eq!(list.share_with, Some(share_with));
    assert_eq!(list.my_rights, Some(TaskListRights::all()));
    assert_eq!(list.extra.get("vendorSyncToken"), Some(&json!("sync-1234")));

    let val = serde_json::to_value(&list).expect("serialize TaskList");
    assert_eq!(val["name"], "Work Projects");
    assert_eq!(val["id"], "tl-work");
    assert_eq!(val["color"], "#008080");
    assert_eq!(val["sortOrder"], 5);
    assert_eq!(val["isDefault"], false);
    assert_eq!(val["isSubscribed"], true);
    assert_eq!(val["vendorSyncToken"], "sync-1234");

    let back: TaskList = serde_json::from_value(val).expect("deserialize TaskList");
    assert_eq!(back, list);
}

#[test]
fn tasks_capability_builders_and_properties_roundtrip() {
    let mut extra = BTreeMap::new();
    extra.insert("supportsSubtasks".to_string(), json!(true));

    let cap = TasksCapability::new()
        .with_max_tasks_per_get(5000)
        .with_max_tasks_per_set(1000)
        .with_max_task_lists_per_get(250)
        .with_extra(extra);

    assert_eq!(cap.max_tasks_per_get, Some(5000));
    assert_eq!(cap.max_tasks_per_set, Some(1000));
    assert_eq!(cap.max_task_lists_per_get, Some(250));
    assert_eq!(cap.extra.get("supportsSubtasks"), Some(&json!(true)));

    let val = serde_json::to_value(&cap).expect("serialize TasksCapability");
    assert_eq!(val["maxTasksPerGet"], 5000);
    assert_eq!(val["maxTasksPerSet"], 1000);
    assert_eq!(val["maxTaskListsPerGet"], 250);
    assert_eq!(val["supportsSubtasks"], true);

    let back: TasksCapability = serde_json::from_value(val).expect("deserialize TasksCapability");
    assert_eq!(back, cap);
}

#[test]
fn task_all_fields_and_subcomponents_roundtrip() {
    let mut relation_map = BTreeMap::new();
    relation_map.insert("parent".to_string(), true);
    let mut related_to = BTreeMap::new();
    related_to.insert(
        "parent-task-uid-001".to_string(),
        EventRelation::new(relation_map),
    );

    let mut locations = BTreeMap::new();
    locations.insert("loc-main".to_string(), Location::new("Office Room 402"));

    let mut virtual_locations = BTreeMap::new();
    virtual_locations.insert(
        "vloc-main".to_string(),
        VirtualLocation::new("https://meet.example.com/project-sync"),
    );

    let mut links = BTreeMap::new();
    links.insert(
        "link-doc".to_string(),
        json!({"href": "https://wiki.example.com/spec", "title": "Specification Document"}),
    );

    let mut localizations = BTreeMap::new();
    localizations.insert("de-DE".to_string(), json!({"title": "Wichtige Aufgabe"}));

    let mut keywords = BTreeSet::new();
    keywords.insert("engineering".to_string());
    keywords.insert("q4-deliverable".to_string());

    let mut categories = BTreeSet::new();
    categories.insert("development".to_string());

    let mut alerts = BTreeMap::new();
    let trigger_val = serde_json::to_value(OffsetTrigger::new("-PT30M")).unwrap();
    alerts.insert("alert-30m".to_string(), Alert::new("display", trigger_val));

    let rule = RecurrenceRule::new("monthly").with_interval(2);

    let mut recurrence_overrides = BTreeMap::new();
    recurrence_overrides.insert(
        "2026-11-01T09:00:00".to_string(),
        json!({"title": "Modified recurrence title"}),
    );

    let task = Task::new("uid-task-full-structure")
        .with_id("t-full")
        .with_task_list_id("tl-active")
        .with_title("Comprehensive Feature Task")
        .with_description("Detailed breakdown of feature execution")
        .with_due("2026-11-15T18:00:00")
        .with_start("2026-11-01T09:00:00")
        .with_time_zone("Europe/Berlin")
        .with_status(task_status::IN_PROCESS)
        .with_progress(task_progress::IN_PROCESS)
        .with_percent_complete(35)
        .with_completed(UtcDate::new("2026-11-15T18:00:00Z"))
        .with_priority(3)
        .with_color("#2E7D32")
        .show_without_time(false)
        .with_alerts(alerts)
        .with_locations(locations)
        .with_virtual_locations(virtual_locations)
        .with_keywords(keywords)
        .with_recurrence_rule(rule);

    let mut task = task;
    task.related_to = Some(related_to);
    task.prod_id = Some("//Example Corp//Task Manager 1.0//EN".to_string());
    task.created = Some(UtcDate::new("2026-10-01T12:00:00Z"));
    task.updated = Some(UtcDate::new("2026-10-15T16:00:00Z"));
    task.sequence = Some(4);
    task.description_content_type = Some("text/plain".to_string());
    task.links = Some(links);
    task.locale = Some("en-US".to_string());
    task.localizations = Some(localizations);
    task.categories = Some(categories);
    task.duration = Some("PT8H".to_string());
    task.estimated_duration = Some("PT12H".to_string());
    task.progress_updated = Some(UtcDate::new("2026-10-15T16:00:00Z"));
    task.privacy = Some("private".to_string());
    task.free_busy_status = Some("busy".to_string());
    task.use_default_alerts = Some(false);
    task.recurrence_overrides = Some(recurrence_overrides);

    let val = serde_json::to_value(&task).expect("serialize full Task");
    assert_eq!(val["@type"], "Task");
    assert_eq!(val["uid"], "uid-task-full-structure");
    assert_eq!(val["id"], "t-full");
    assert_eq!(val["taskListId"], "tl-active");
    assert_eq!(val["title"], "Comprehensive Feature Task");
    assert_eq!(val["prodId"], "//Example Corp//Task Manager 1.0//EN");
    assert_eq!(val["created"], "2026-10-01T12:00:00Z");
    assert_eq!(val["sequence"], 4);
    assert_eq!(val["duration"], "PT8H");
    assert_eq!(val["estimatedDuration"], "PT12H");
    assert_eq!(val["privacy"], "private");
    assert_eq!(val["freeBusyStatus"], "busy");
    assert_eq!(val["useDefaultAlerts"], false);

    let back: Task = serde_json::from_value(val).expect("deserialize full Task");
    assert_eq!(back, task);
}

#[test]
fn task_standards_and_constants_verification() {
    assert_eq!(CAPABILITY_TASKS, "urn:ietf:params:jmap:tasks");

    assert_eq!(task_status::NEEDS_ACTION, "needs-action");
    assert_eq!(task_status::COMPLETED, "completed");
    assert_eq!(task_status::IN_PROCESS, "in-process");
    assert_eq!(task_status::CANCELLED, "cancelled");
    assert_eq!(task_status::FAILED, "failed");

    assert_eq!(task_progress::NEEDS_ACTION, "needs-action");
    assert_eq!(task_progress::COMPLETED, "completed");
    assert_eq!(task_progress::IN_PROCESS, "in-process");
    assert_eq!(task_progress::FAILED, "failed");
    assert_eq!(task_progress::CANCELLED, "cancelled");

    assert_eq!(task_set_error::TOO_MANY_RECURRENCES, "tooManyRecurrences");
    assert_eq!(task_set_error::TASK_LIST_NOT_FOUND, "taskListNotFound");
}
