// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Determines who sends meeting invitations: the server always does.
//!
//! Evolution never sends them itself. The user can suppress the message for one change.

use std::ffi::CStr;

use eds_sys::{E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE, ECalOperationFlags};
use jmap_cal_sync::Itip;

/// Static capability indicating the backend creates messages.
pub const CREATE_MESSAGES: &str = "create-messages";

/// Static capability indicating the backend saves schedules.
pub const SAVE_SCHEDULES: &str = "save-schedules";

/// Static capability indicating the backend supports suppression on removal.
pub const ITIP_SUPPRESS_ON_REMOVE_SUPPORTED: &str = "itip-suppress-on-remove-supported";

/// Backend capabilities property name.
pub const CAPABILITIES_PROPERTY: &CStr = c"capabilities";

/// Returns the inherited capability list with the server scheduling capabilities added.
pub fn capabilities(inherited: Option<&str>) -> String {
    let mut caps: Vec<&str> = inherited
        .unwrap_or_default()
        .split(',')
        .filter(|cap| !cap.is_empty())
        .collect();
    for cap in [
        CREATE_MESSAGES,
        SAVE_SCHEDULES,
        ITIP_SUPPRESS_ON_REMOVE_SUPPORTED,
    ] {
        if !caps.contains(&cap) {
            caps.push(cap);
        }
    }
    caps.join(",")
}

/// Marks a received change in its opflags. EDS defines bits 0 to 4 only.
///
/// EDS keeps the opflags of a queued offline change and uploads with them later.
const RECEIVED: ECalOperationFlags = 1 << 30;

/// Determines whether an operation sends iTIP messages to guests.
///
/// Returns [`Itip::Send`] unless `opflags` suppresses the message.
/// Returns [`Itip::Received`] when [`received`] marked the opflags.
pub fn itip(opflags: ECalOperationFlags) -> Itip {
    let send = opflags & E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE == 0;
    if opflags & RECEIVED != 0 {
        Itip::Received { reply: send }
    } else if send {
        Itip::Send
    } else {
        Itip::Suppress
    }
}

/// Returns the flags that `receive_objects_sync` gives to the meta backend.
///
/// The mark stays with the change through EDS's offline queue.
pub fn received(opflags: ECalOperationFlags) -> ECalOperationFlags {
    opflags | RECEIVED
}

/// Returns the flags for a reminder dismissal, which never sends iTIP.
///
/// A dismissal changes only the user's own alert, so the guests must not hear of it.
pub fn dismissal(opflags: ECalOperationFlags) -> ECalOperationFlags {
    opflags | E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE
}
