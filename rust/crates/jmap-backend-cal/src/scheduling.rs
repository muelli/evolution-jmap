// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Determines who sends meeting invitations: the server always does.
//!
//! Evolution never sends them itself. The user can suppress the message for one change.

use std::cell::Cell;
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

thread_local! {
    static RECEIVING: Cell<bool> = const { Cell::new(false) };
}

/// Marks the changes on this thread as received until the guard drops.
///
/// `receive_objects_sync` holds it while the meta backend saves on the same thread.
#[must_use]
pub fn receiving() -> Receiving {
    Receiving(RECEIVING.replace(true))
}

/// The guard from [`receiving`]. It restores the previous mark on drop.
pub struct Receiving(bool);

impl Drop for Receiving {
    fn drop(&mut self) {
        RECEIVING.set(self.0);
    }
}

/// Determines whether an operation sends iTIP messages to guests.
///
/// Returns [`Itip::Send`] unless `opflags` suppresses the message.
/// Returns [`Itip::Received`] while [`receiving`] marks this thread.
pub fn itip(opflags: ECalOperationFlags) -> Itip {
    let send = opflags & E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE == 0;
    if RECEIVING.get() {
        Itip::Received { reply: send }
    } else if send {
        Itip::Send
    } else {
        Itip::Suppress
    }
}

/// Returns the flags for a reminder dismissal, which never sends iTIP.
///
/// A dismissal changes only the user's own alert, so the guests must not hear of it.
pub fn dismissal(opflags: ECalOperationFlags) -> ECalOperationFlags {
    opflags | E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE
}
