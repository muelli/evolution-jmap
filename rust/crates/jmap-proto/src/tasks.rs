// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP for Tasks (draft-ietf-jmap-tasks) and JSCalendar Task (RFC 8984 §2.2 / §5.2):
//! `Task`, `TaskList`, `TaskListRights`, `TasksCapability`, `TaskQueryFilter`,
//! and standard constants.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::calendars::{
    Alert, EventRelation, Location, Participant, RecurrenceRule, VirtualLocation,
};
use crate::id::Id;
use crate::state::UtcDate;

/// Standard task capability identifier (draft-ietf-jmap-tasks §1.1).
pub const CAPABILITY_TASKS: &str = "urn:ietf:params:jmap:tasks";

/// Tasks capability properties (draft-ietf-jmap-tasks §1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TasksCapability {
    /// The largest number of tasks the server lets a single `Task/get` return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_get: Option<u64>,
    /// The largest number of tasks the server lets a single `Task/set` change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_set: Option<u64>,
    /// The largest number of task lists the server lets a single `TaskList/get` return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_task_lists_per_get: Option<u64>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TasksCapability {
    /// Create an empty capability with no limits set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the largest number of tasks a single `Task/get` returns.
    pub fn with_max_tasks_per_get(mut self, max: u64) -> Self {
        self.max_tasks_per_get = Some(max);
        self
    }

    /// Set the largest number of tasks a single `Task/set` changes.
    pub fn with_max_tasks_per_set(mut self, max: u64) -> Self {
        self.max_tasks_per_set = Some(max);
        self
    }

    /// Set the largest number of task lists a single `TaskList/get` returns.
    pub fn with_max_task_lists_per_get(mut self, max: u64) -> Self {
        self.max_task_lists_per_get = Some(max);
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// A TaskList collection (draft-ietf-jmap-tasks §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskList {
    /// The server-assigned id. Absent on a `TaskList/set create` argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The user-visible name of the task list.
    pub name: String,
    /// A colour the client may use to display the task list, as a CSS colour value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// A number the client may use to order task lists for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u64>,
    /// True if this is the account's default task list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// True if the user has subscribed to this shared task list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    /// The rights other accounts have on this task list, keyed by account id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_with: Option<BTreeMap<Id, TaskListRights>>,
    /// The rights the requesting account has on this task list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<TaskListRights>,
    /// True if the requesting account may delete this task list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TaskList {
    /// Create a task list with the given name and no other properties set.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: None,
            name: name.into(),
            color: None,
            sort_order: None,
            is_default: None,
            is_subscribed: None,
            share_with: None,
            my_rights: None,
            may_delete: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the display colour.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Set the display sort order.
    pub fn with_sort_order(mut self, order: u64) -> Self {
        self.sort_order = Some(order);
        self
    }

    /// Set whether this is the account's default task list.
    pub fn is_default(mut self, default: bool) -> Self {
        self.is_default = Some(default);
        self
    }

    /// Set whether the user has subscribed to this shared task list.
    pub fn is_subscribed(mut self, subscribed: bool) -> Self {
        self.is_subscribed = Some(subscribed);
        self
    }

    /// Set the rights other accounts have on this task list.
    pub fn with_share_with(mut self, share_with: BTreeMap<Id, TaskListRights>) -> Self {
        self.share_with = Some(share_with);
        self
    }

    /// Set the rights the requesting account has on this task list.
    pub fn with_my_rights(mut self, rights: TaskListRights) -> Self {
        self.my_rights = Some(rights);
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Permissions on a task list (draft-ietf-jmap-tasks §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskListRights {
    /// True if the account may see the tasks on this list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read_items: Option<bool>,
    /// True if the account may create, update and destroy any task on this list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write_all: Option<bool>,
    /// True if the account may create, update and destroy its own tasks on this list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write_own: Option<bool>,
    /// True if the account may update the private properties of tasks on this list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_update_private: Option<bool>,
    /// True if the account may see other sharees of this list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_reread: Option<bool>,
    /// True if the account may change the sharing settings of this list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_admin: Option<bool>,
    /// True if the account may delete this list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TaskListRights {
    /// Rights that grant every permission.
    pub fn all() -> Self {
        Self {
            may_read_items: Some(true),
            may_write_all: Some(true),
            may_write_own: Some(true),
            may_update_private: Some(true),
            may_reread: Some(true),
            may_admin: Some(true),
            may_delete: Some(true),
            extra: BTreeMap::new(),
        }
    }

    /// Rights that grant read access only.
    pub fn read_only() -> Self {
        Self {
            may_read_items: Some(true),
            may_write_all: Some(false),
            may_write_own: Some(false),
            may_update_private: Some(false),
            may_reread: Some(true),
            may_admin: Some(false),
            may_delete: Some(false),
            extra: BTreeMap::new(),
        }
    }

    /// True if these rights grant write access to all or to the account's own tasks.
    pub fn is_writable(&self) -> bool {
        self.may_write_all == Some(true) || self.may_write_own == Some(true)
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// A Task object (draft-ietf-jmap-tasks §4, RFC 8984 §2.2 / §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Task {
    /// The server-assigned id. Absent on a `Task/set create` argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The id of the task list this task belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_list_id: Option<Id>,
    /// The JSCalendar object type. Always `"Task"` for a Task object.
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    /// A globally unique identifier for the task, stable across updates.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uid: String,
    /// Other objects this task relates to, keyed by their uid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_to: Option<BTreeMap<String, EventRelation>>,
    /// The identifier of the product that last created or updated the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prod_id: Option<String>,
    /// The date and time the task was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    /// The date and time the task was last updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<UtcDate>,
    /// A monotonically increasing number the server updates every time the task changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// The short, free text summary of the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A longer, free text description of the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The media type of the description, if not plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_content_type: Option<String>,
    /// True if only the date, not the time of day, of `start` and `due` matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_without_time: Option<bool>,
    /// The physical locations associated with the task, keyed by an id of the client's choosing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<BTreeMap<String, Location>>,
    /// The virtual locations associated with the task, keyed by an id of the client's choosing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_locations: Option<BTreeMap<String, VirtualLocation>>,
    /// Links to resources related to the task, keyed by an id of the client's choosing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, Value>>,
    /// The language tag of the task's title and description, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Translations of the task's localisable properties, keyed by language tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub localizations: Option<BTreeMap<String, Value>>,
    /// Keywords or tags associated with the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeSet<String>>,
    /// Category labels associated with the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<BTreeSet<String>>,
    /// A colour the client may use to display the task, as a CSS colour value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The date and time by which the task should be completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// The date and time at which the task should start to be worked on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// The time zone `start` and `due` are expressed in, if not floating or UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// How long the task is expected to take, as an ISO 8601 duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// An estimate of how much time the task requires, as an ISO 8601 duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_duration: Option<String>,
    /// The overall status of the task, one of the [`task_status`] values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// A human-readable description of progress, for statuses finer than `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// The date and time `progress` or `percent_complete` was last updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_updated: Option<UtcDate>,
    /// How much of the task is complete, as a percentage from 0 to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent_complete: Option<u64>,
    /// The date and time the task was completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<UtcDate>,
    /// The task's priority, from 0 (undefined) to 9 (lowest), per RFC 8984 §4.4.9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u64>,
    /// How visible the task should be to other users, one of `public`, `private` or `secret`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy: Option<String>,
    /// Whether the task blocks other scheduling for its time span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_busy_status: Option<String>,
    /// Alerts to notify the user about the task, keyed by an id of the client's choosing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts: Option<BTreeMap<String, Alert>>,
    /// True if the user's default alerts apply to this task instead of `alerts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_default_alerts: Option<bool>,
    /// The recurrence rule for a repeating task, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_rule: Option<RecurrenceRule>,
    /// Overrides to specific recurrence instances, keyed by their recurrence id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_overrides: Option<BTreeMap<String, Value>>,
    /// The people associated with the task, keyed by an id of the client's choosing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participants: Option<BTreeMap<String, Participant>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Task {
    /// Create a task with the given uid and no other properties set.
    pub fn new(uid: impl Into<String>) -> Self {
        Self {
            task_type: Some("Task".to_owned()),
            uid: uid.into(),
            ..Self::default()
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the id of the task list this task belongs to.
    pub fn with_task_list_id(mut self, task_list_id: impl Into<Id>) -> Self {
        self.task_list_id = Some(task_list_id.into());
        self
    }

    /// Set the short, free text summary.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the longer, free text description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the date and time by which the task should be completed.
    pub fn with_due(mut self, due: impl Into<String>) -> Self {
        self.due = Some(due.into());
        self
    }

    /// Set the date and time at which the task should start to be worked on.
    pub fn with_start(mut self, start: impl Into<String>) -> Self {
        self.start = Some(start.into());
        self
    }

    /// Set the time zone `start` and `due` are expressed in.
    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    /// Set the overall status, one of the [`task_status`] values.
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// Set the human-readable progress description.
    pub fn with_progress(mut self, progress: impl Into<String>) -> Self {
        self.progress = Some(progress.into());
        self
    }

    /// Set how much of the task is complete, as a percentage from 0 to 100.
    pub fn with_percent_complete(mut self, percent: u64) -> Self {
        self.percent_complete = Some(percent);
        self
    }

    /// Set the date and time the task was completed.
    pub fn with_completed(mut self, completed: impl Into<UtcDate>) -> Self {
        self.completed = Some(completed.into());
        self
    }

    /// Set the priority, from 0 (undefined) to 9 (lowest).
    pub fn with_priority(mut self, priority: u64) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Set the display colour.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Set whether only the date, not the time of day, of `start` and `due` matters.
    pub fn show_without_time(mut self, show: bool) -> Self {
        self.show_without_time = Some(show);
        self
    }

    /// Set the alerts that notify the user about the task.
    pub fn with_alerts(mut self, alerts: BTreeMap<String, Alert>) -> Self {
        self.alerts = Some(alerts);
        self
    }

    /// Set the physical locations associated with the task.
    pub fn with_locations(mut self, locations: BTreeMap<String, Location>) -> Self {
        self.locations = Some(locations);
        self
    }

    /// Set the virtual locations associated with the task.
    pub fn with_virtual_locations(
        mut self,
        virtual_locations: BTreeMap<String, VirtualLocation>,
    ) -> Self {
        self.virtual_locations = Some(virtual_locations);
        self
    }

    /// Set the keywords or tags associated with the task.
    pub fn with_keywords(mut self, keywords: BTreeSet<String>) -> Self {
        self.keywords = Some(keywords);
        self
    }

    /// Set the recurrence rule for a repeating task.
    pub fn with_recurrence_rule(mut self, rule: RecurrenceRule) -> Self {
        self.recurrence_rule = Some(rule);
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Task/query` filter (draft-ietf-jmap-tasks §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskQueryFilter {
    /// Match only tasks on this task list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_list_id: Option<Id>,
    /// Match only tasks whose title, description or locations contain this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Match only tasks that start before this date and time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Match only tasks that start after this date and time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Match only tasks due before this date and time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_before: Option<String>,
    /// Match only tasks due after this date and time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_after: Option<String>,
    /// Match only tasks completed before this date and time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_before: Option<UtcDate>,
    /// Match only tasks completed after this date and time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_after: Option<UtcDate>,
    /// Match only tasks with this [`task_status`] value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Match only tasks that do, or do not, have a recurrence rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_recurrence: Option<bool>,
    /// Extra filter properties this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TaskQueryFilter {
    /// Create an empty filter that matches every task.
    pub fn new() -> Self {
        Self::default()
    }

    /// Match only tasks on the given task list.
    pub fn with_task_list_id(mut self, id: impl Into<Id>) -> Self {
        self.task_list_id = Some(id.into());
        self
    }

    /// Match only tasks whose title, description or locations contain this text.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Match only tasks with the given status.
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// Match only tasks due before the given date and time.
    pub fn with_due_before(mut self, due: impl Into<String>) -> Self {
        self.due_before = Some(due.into());
        self
    }

    /// Match only tasks that do, or do not, have a recurrence rule.
    pub fn with_has_recurrence(mut self, has_recurrence: bool) -> Self {
        self.has_recurrence = Some(has_recurrence);
        self
    }

    /// Set the extra filter properties this struct does not name.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard task status values (draft-ietf-jmap-tasks §4.1).
pub mod task_status {
    /// The task has not been started.
    pub const NEEDS_ACTION: &str = "needs-action";
    /// The task has been completed.
    pub const COMPLETED: &str = "completed";
    /// The task is in progress.
    pub const IN_PROCESS: &str = "in-process";
    /// The task has been cancelled.
    pub const CANCELLED: &str = "cancelled";
    /// The task could not be completed.
    pub const FAILED: &str = "failed";
}

/// Standard task progress values (RFC 8984 §5.2.5, draft-ietf-jmap-tasks §4.1).
pub mod task_progress {
    /// The task has not been started.
    pub const NEEDS_ACTION: &str = "needs-action";
    /// The task has been completed.
    pub const COMPLETED: &str = "completed";
    /// The task is in progress.
    pub const IN_PROCESS: &str = "in-process";
    /// The task could not be completed.
    pub const FAILED: &str = "failed";
    /// The task has been cancelled.
    pub const CANCELLED: &str = "cancelled";
}

/// Set errors added for tasks (draft-ietf-jmap-tasks §4.3).
pub mod task_set_error {
    /// The set error type returned when a recurrence rule would generate too many instances.
    pub const TOO_MANY_RECURRENCES: &str = "tooManyRecurrences";
    /// The set error type returned when the given task list does not exist.
    pub const TASK_LIST_NOT_FOUND: &str = "taskListNotFound";
}
