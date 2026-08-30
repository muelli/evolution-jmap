// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP for Tasks (draft-ietf-jmap-tasks) and JSCalendar Task (RFC 8984 §5):
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
pub struct TasksCapability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_get: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_set: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_task_lists_per_get: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TasksCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_tasks_per_get(mut self, max: u64) -> Self {
        self.max_tasks_per_get = Some(max);
        self
    }

    pub fn with_max_tasks_per_set(mut self, max: u64) -> Self {
        self.max_tasks_per_set = Some(max);
        self
    }

    pub fn with_max_task_lists_per_get(mut self, max: u64) -> Self {
        self.max_task_lists_per_get = Some(max);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// A TaskList collection (draft-ietf-jmap-tasks §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_with: Option<BTreeMap<Id, TaskListRights>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<TaskListRights>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TaskList {
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

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    pub fn with_sort_order(mut self, order: u64) -> Self {
        self.sort_order = Some(order);
        self
    }

    pub fn is_default(mut self, default: bool) -> Self {
        self.is_default = Some(default);
        self
    }

    pub fn is_subscribed(mut self, subscribed: bool) -> Self {
        self.is_subscribed = Some(subscribed);
        self
    }

    pub fn with_share_with(mut self, share_with: BTreeMap<Id, TaskListRights>) -> Self {
        self.share_with = Some(share_with);
        self
    }

    pub fn with_my_rights(mut self, rights: TaskListRights) -> Self {
        self.my_rights = Some(rights);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Permissions on a task list (draft-ietf-jmap-tasks §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskListRights {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write_all: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write_own: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_update_private: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_reread: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_admin: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TaskListRights {
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

    pub fn is_writable(&self) -> bool {
        self.may_write_all == Some(true) || self.may_write_own == Some(true)
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// A Task object (draft-ietf-jmap-tasks §4, RFC 8984 §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_list_id: Option<Id>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    pub uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_to: Option<BTreeMap<String, EventRelation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prod_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_without_time: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<BTreeMap<String, Location>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_locations: Option<BTreeMap<String, VirtualLocation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, crate::contacts::Link>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub localizations: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_duration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_updated: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent_complete: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_busy_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts: Option<BTreeMap<String, Alert>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_default_alerts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_rule: Option<RecurrenceRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_overrides: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participants: Option<BTreeMap<String, Participant>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Task {
    pub fn new(uid: impl Into<String>) -> Self {
        Self {
            task_type: Some("Task".to_owned()),
            uid: uid.into(),
            ..Self::default()
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_task_list_id(mut self, task_list_id: impl Into<Id>) -> Self {
        self.task_list_id = Some(task_list_id.into());
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_due(mut self, due: impl Into<String>) -> Self {
        self.due = Some(due.into());
        self
    }

    pub fn with_start(mut self, start: impl Into<String>) -> Self {
        self.start = Some(start.into());
        self
    }

    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn with_progress(mut self, progress: impl Into<String>) -> Self {
        self.progress = Some(progress.into());
        self
    }

    pub fn with_percent_complete(mut self, percent: u64) -> Self {
        self.percent_complete = Some(percent);
        self
    }

    pub fn with_completed(mut self, completed: impl Into<UtcDate>) -> Self {
        self.completed = Some(completed.into());
        self
    }

    pub fn with_priority(mut self, priority: u64) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    pub fn show_without_time(mut self, show: bool) -> Self {
        self.show_without_time = Some(show);
        self
    }

    pub fn with_alerts(mut self, alerts: BTreeMap<String, Alert>) -> Self {
        self.alerts = Some(alerts);
        self
    }

    pub fn with_locations(mut self, locations: BTreeMap<String, Location>) -> Self {
        self.locations = Some(locations);
        self
    }

    pub fn with_virtual_locations(
        mut self,
        virtual_locations: BTreeMap<String, VirtualLocation>,
    ) -> Self {
        self.virtual_locations = Some(virtual_locations);
        self
    }

    pub fn with_keywords(mut self, keywords: BTreeSet<String>) -> Self {
        self.keywords = Some(keywords);
        self
    }

    pub fn with_recurrence_rule(mut self, rule: RecurrenceRule) -> Self {
        self.recurrence_rule = Some(rule);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Task/query` filter (draft-ietf-jmap-tasks §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_list_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_before: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_after: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_recurrence: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TaskQueryFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_task_list_id(mut self, id: impl Into<Id>) -> Self {
        self.task_list_id = Some(id.into());
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn with_due_before(mut self, due: impl Into<String>) -> Self {
        self.due_before = Some(due.into());
        self
    }

    pub fn with_has_recurrence(mut self, has_recurrence: bool) -> Self {
        self.has_recurrence = Some(has_recurrence);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard task status values (RFC 8984 §5.1, draft-ietf-jmap-tasks §4.1).
pub mod task_status {
    pub const NEEDS_ACTION: &str = "needs-action";
    pub const COMPLETED: &str = "completed";
    pub const IN_PROCESS: &str = "in-process";
    pub const CANCELLED: &str = "cancelled";
    pub const FAILED: &str = "failed";
}

/// Standard task progress values (RFC 8984 §5.1, draft-ietf-jmap-tasks §4.1).
pub mod task_progress {
    pub const NEEDS_ACTION: &str = "needs-action";
    pub const COMPLETED: &str = "completed";
    pub const IN_PROCESS: &str = "in-process";
    pub const FAILED: &str = "failed";
    pub const CANCELLED: &str = "cancelled";
}

/// Set errors added for tasks (draft-ietf-jmap-tasks §4.3).
pub mod task_set_error {
    pub const TOO_MANY_RECURRENCES: &str = "tooManyRecurrences";
    pub const TASK_LIST_NOT_FOUND: &str = "taskListNotFound";
}
