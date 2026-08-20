// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar-colour write-back — `source_changed`.
//!
//! `ESourceSelectable`'s colour is the one calendar property that can be
//! edited locally without going through EDS's usual save-a-component path
//! (the colour picker in the calendar-properties dialog writes it straight
//! to the `ESource`), so it needs its own, narrower `Calendar/set` call
//! rather than riding on [`crate::CalSync::save_component`].

use serde_json::{Value, json};

use crate::CalSync;
use crate::error::SyncError;

impl CalSync {
    /// Push a colour edit — `None` clears it, matching the omitted-vs-empty
    /// rule the read path already uses for `Calendar.color`.
    pub fn set_color(&self, color: Option<&str>) -> Result<(), SyncError> {
        let patch: Value = json!({ "color": color });
        self.client()
            .calendar_update(self.account_id(), self.calendar_id(), patch)?;
        Ok(())
    }
}
