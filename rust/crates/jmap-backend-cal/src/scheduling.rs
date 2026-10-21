// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Determines meeting invitation dispatch between server and client.
//!
//! The configuration setting lives in `[WebDAV Backend]` as `CalendarAutoSchedule`.
//! When the group is absent, scheduling defaults to server-managed.
//! Setting `CalendarAutoSchedule=false` disables server scheduling.
//!
//! The backend reads this setting on every change and capability query.

use std::cell::Cell;
use std::ffi::CStr;

use eds_sys::{
    E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE, E_SOURCE_EXTENSION_WEBDAV_BACKEND,
    ECalOperationFlags, ESource, ESourceWebdav, e_source_webdav_get_calendar_auto_schedule,
    e_source_webdav_get_type,
};
use glib_sys::GFALSE;
use jmap_backend_core::marshal::extension_if_present;
use jmap_cal_sync::Itip;

/// Static capability indicating the backend creates messages.
pub const CREATE_MESSAGES: &str = "create-messages";

/// Static capability indicating the backend saves schedules.
pub const SAVE_SCHEDULES: &str = "save-schedules";

/// Static capability indicating the backend supports suppression on removal.
pub const ITIP_SUPPRESS_ON_REMOVE_SUPPORTED: &str = "itip-suppress-on-remove-supported";

/// Backend capabilities property name.
pub const CAPABILITIES_PROPERTY: &CStr = c"capabilities";

/// Returns the capability list for the calendar backend.
///
/// Adds server scheduling capabilities when `server_schedules` is true.
pub fn capabilities(inherited: Option<&str>, server_schedules: bool) -> String {
    let mut caps: Vec<&str> = inherited
        .unwrap_or_default()
        .split(',')
        .filter(|cap| !cap.is_empty())
        .collect();
    if server_schedules {
        for cap in [
            CREATE_MESSAGES,
            SAVE_SCHEDULES,
            ITIP_SUPPRESS_ON_REMOVE_SUPPORTED,
        ] {
            if !caps.contains(&cap) {
                caps.push(cap);
            }
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
/// Returns [`Itip::Send`] if server scheduling is active and unsuppressed.
/// Returns [`Itip::Received`] while [`receiving`] marks this thread.
pub fn itip(server_schedules: bool, opflags: ECalOperationFlags) -> Itip {
    let send = server_schedules && opflags & E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE == 0;
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

/// Checks whether the account source enables server scheduling.
///
/// Returns false when `source` is null.
///
/// # Safety
///
/// `source` must be null or point to a valid, live `ESource`.
pub unsafe fn server_handles_invitations(source: *mut ESource) -> bool {
    if source.is_null() {
        return false;
    }
    // Register the extension type before lookup by name.
    // SAFETY: Initializing the GType requires no preconditions.
    unsafe { e_source_webdav_get_type() };
    // Inspect extension without creating an empty group.
    // SAFETY: `source` is valid by the caller contract.
    match unsafe {
        extension_if_present::<ESourceWebdav>(source, E_SOURCE_EXTENSION_WEBDAV_BACKEND)
    } {
        None => true,
        // SAFETY: `webdav` is a valid extension pointer returned by `extension_if_present`.
        Some(webdav) => unsafe { e_source_webdav_get_calendar_auto_schedule(webdav) != GFALSE },
    }
}
