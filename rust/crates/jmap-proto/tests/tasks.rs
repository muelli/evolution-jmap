// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::calendars::{Alert, Location, OffsetTrigger, RecurrenceRule, VirtualLocation};
use jmap_proto::state::UtcDate;
use jmap_proto::tasks::{
    Task, TaskList, TaskListRights, TaskQueryFilter, TasksCapability, task_progress,
    task_set_error, task_status,
};

#[test]
fn task_list_roundtrip_and_builders() {
    let mut share_with = BTreeMap::new();
    share_with.insert(Id::new("user_assistant"), TaskListRights::read_only());

    let list = TaskList::new("Groceries")
        .with_id("tl_groceries")
        .with_color("#26A69A")
        .with_sort_order(10)
        .is_default(true)
        .is_subscribed(true)
        .with_my_rights(TaskListRights::all())
        .with_share_with(share_with);

    let val = serde_json::to_value(&list).expect("serialize TaskList");
    assert_eq!(val["name"], "Groceries");
    assert_eq!(val["id"], "tl_groceries");
    assert_eq!(val["color"], "#26A69A");
    assert_eq!(val["sortOrder"], 10);
    assert_eq!(val["isDefault"], true);
    assert_eq!(val["isSubscribed"], true);
    assert_eq!(val["myRights"]["mayWriteAll"], true);
    assert_eq!(val["shareWith"]["user_assistant"]["mayWriteAll"], false);

    let back: TaskList = serde_json::from_value(val).expect("deserialize TaskList");
    assert_eq!(back.name, "Groceries");
    assert_eq!(back.id.as_ref().unwrap().as_str(), "tl_groceries");
    assert!(back.my_rights.as_ref().unwrap().is_writable());
    assert!(
        !back
            .share_with
            .as_ref()
            .unwrap()
            .get(&Id::new("user_assistant"))
            .unwrap()
            .is_writable()
    );
}

#[test]
fn task_roundtrip_and_builders() {
    let mut alerts = BTreeMap::new();
    let trigger_val = serde_json::to_value(OffsetTrigger::new("-PT15M")).unwrap();
    alerts.insert("a1".to_owned(), Alert::new("display", trigger_val));

    let mut locations = BTreeMap::new();
    locations.insert("loc1".to_owned(), Location::new("Supermarket"));

    let mut virtual_locations = BTreeMap::new();
    virtual_locations.insert(
        "vloc1".to_owned(),
        VirtualLocation::new("https://example.com/shared-cart"),
    );

    let mut keywords = BTreeSet::new();
    keywords.insert("shopping".to_owned());
    keywords.insert("weekly".to_owned());

    let task = Task::new("task-uid-12345")
        .with_id("t1")
        .with_task_list_id("tl_groceries")
        .with_title("Buy milk and sourdough")
        .with_description("Get organic whole milk and fresh sourdough bread")
        .with_due("2026-09-01T18:00:00")
        .with_start("2026-09-01T17:00:00")
        .with_time_zone("Europe/Zurich")
        .with_status(task_status::NEEDS_ACTION)
        .with_progress(task_progress::IN_PROCESS)
        .with_percent_complete(50)
        .with_completed(UtcDate::new("2026-09-01T18:00:00Z"))
        .with_priority(1)
        .with_color("#FF7043")
        .show_without_time(false)
        .with_alerts(alerts)
        .with_locations(locations)
        .with_virtual_locations(virtual_locations)
        .with_keywords(keywords)
        .with_recurrence_rule(RecurrenceRule::new("weekly").with_interval(1));

    let val = serde_json::to_value(&task).expect("serialize Task");
    assert_eq!(val["@type"], "Task");
    assert_eq!(val["uid"], "task-uid-12345");
    assert_eq!(val["id"], "t1");
    assert_eq!(val["taskListId"], "tl_groceries");
    assert_eq!(val["title"], "Buy milk and sourdough");
    assert_eq!(val["due"], "2026-09-01T18:00:00");
    assert_eq!(val["status"], "needs-action");
    assert_eq!(val["progress"], "in-process");
    assert_eq!(val["percentComplete"], 50);
    assert_eq!(val["priority"], 1);
    assert_eq!(val["recurrenceRule"]["frequency"], "weekly");

    let back: Task = serde_json::from_value(val).expect("deserialize Task");
    assert_eq!(back.uid, "task-uid-12345");
    assert_eq!(back.id.as_ref().unwrap().as_str(), "t1");
    assert_eq!(back.status.as_deref(), Some(task_status::NEEDS_ACTION));
    assert_eq!(back.progress.as_deref(), Some(task_progress::IN_PROCESS));
    assert_eq!(back.percent_complete, Some(50));
}

#[test]
fn task_query_filter_roundtrip_and_builders() {
    let filter = TaskQueryFilter::new()
        .with_task_list_id("tl_groceries")
        .with_text("milk")
        .with_status(task_status::NEEDS_ACTION)
        .with_due_before("2026-09-02T00:00:00")
        .with_has_recurrence(true);

    let val = serde_json::to_value(&filter).expect("serialize TaskQueryFilter");
    assert_eq!(val["taskListId"], "tl_groceries");
    assert_eq!(val["text"], "milk");
    assert_eq!(val["status"], "needs-action");
    assert_eq!(val["dueBefore"], "2026-09-02T00:00:00");
    assert_eq!(val["hasRecurrence"], true);

    let back: TaskQueryFilter = serde_json::from_value(val).expect("deserialize TaskQueryFilter");
    assert_eq!(back.task_list_id.as_ref().unwrap().as_str(), "tl_groceries");
    assert_eq!(back.text.as_deref(), Some("milk"));
}

#[test]
fn tasks_capability_roundtrip_and_builders() {
    let cap = TasksCapability::new()
        .with_max_tasks_per_get(1000)
        .with_max_tasks_per_set(500)
        .with_max_task_lists_per_get(100);

    let val = serde_json::to_value(&cap).expect("serialize TasksCapability");
    assert_eq!(val["maxTasksPerGet"], 1000);
    assert_eq!(val["maxTasksPerSet"], 500);
    assert_eq!(val["maxTaskListsPerGet"], 100);

    let back: TasksCapability = serde_json::from_value(val).expect("deserialize TasksCapability");
    assert_eq!(back.max_tasks_per_get, Some(1000));
    assert_eq!(back.max_tasks_per_set, Some(500));
    assert_eq!(back.max_task_lists_per_get, Some(100));
}

#[test]
fn task_constants_verification() {
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
