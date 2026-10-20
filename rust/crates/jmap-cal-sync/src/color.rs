// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Write-back of the calendar colour (`source_changed`).
//!
//! The colour of `ESourceSelectable` is the only calendar property that a user
//! can edit locally without the usual EDS component save. The colour picker in
//! the calendar-properties dialog writes it directly to the `ESource`. Thus it
//! needs its own, smaller `Calendar/set` call, and does not use
//! [`crate::CalSync::save_component`].

use serde_json::{Value, json};

use crate::CalSync;
use crate::error::SyncError;

impl CalSync {
    /// Sends a colour edit. `None` clears the colour, which agrees with the
    /// omitted-or-empty rule of the read path for `Calendar.color`.
    pub fn set_color(&self, color: Option<&str>) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let calendar_id = self.calendar_id().to_string();
        tracing::debug!(account_id, calendar_id, color, "pushing calendar colour");
        let patch: Value = json!({ "color": color });
        if let Err(error) =
            self.client()
                .calendar_update(self.account_id(), self.calendar_id(), patch)
        {
            tracing::warn!(
                account_id,
                calendar_id,
                %error,
                "calendar colour push failed"
            );
            return Err(error.into());
        }
        Ok(())
    }
}
