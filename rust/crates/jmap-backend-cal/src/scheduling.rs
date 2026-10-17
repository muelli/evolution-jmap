// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Who tells a meeting's guests: the backend's half of the rule in
//! `docs/MEETING-SCHEDULING.md` section 4.
//!
//! The switch is the account's "server handles meeting invitations" option,
//! and it lives where EDS's CalDAV backend keeps the same option: the
//! `[WebDAV Backend]` group's `CalendarAutoSchedule`
//! (`e_source_webdav_get_calendar_auto_schedule`). Unlike CalDAV's, it is on
//! when the group is absent, because every JMAP calendar server we know
//! schedules (rule 1); a calendar source turns it off with
//!
//! ```text
//! [WebDAV Backend]
//! CalendarAutoSchedule=false
//! ```
//!
//! Anything that creates the group without setting the key reads as off too,
//! since `CalendarAutoSchedule` defaults to false. That is the safe side to
//! fail on: Evolution mails the guests itself, as it did before.
//!
//! The switch is read afresh for every change and every capability query,
//! as CalDAV does, so a toggled option takes effect without reconnecting.

use std::ffi::CStr;

use eds_sys::{
    E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE, E_SOURCE_EXTENSION_WEBDAV_BACKEND,
    ECalOperationFlags, ESource, ESourceWebdav, e_source_webdav_get_calendar_auto_schedule,
    e_source_webdav_get_type,
};
use glib_sys::GFALSE;
use jmap_backend_core::marshal::extension_if_present;
use jmap_cal_sync::Itip;

/// `E_CAL_STATIC_CAPABILITY_CREATE_MESSAGES`: Evolution's `itip-utils.c`
/// composes no mail of its own for a client that has it.
pub const CREATE_MESSAGES: &str = "create-messages";

/// `E_CAL_STATIC_CAPABILITY_SAVE_SCHEDULES`: the backend sends the iTIP for
/// a save, and honours `E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`.
pub const SAVE_SCHEDULES: &str = "save-schedules";

/// `E_CAL_STATIC_CAPABILITY_ITIP_SUPPRESS_ON_REMOVE_SUPPORTED`: the same
/// flag is honoured on a removal.
pub const ITIP_SUPPRESS_ON_REMOVE_SUPPORTED: &str = "itip-suppress-on-remove-supported";

/// `CLIENT_BACKEND_PROPERTY_CAPABILITIES`, a `#define`d string in
/// `e-client.h`.
pub const CAPABILITIES_PROPERTY: &CStr = c"capabilities";

/// The capabilities to answer, given what the meta backend answered.
///
/// With the switch on, the three that tell Evolution the backend delivers
/// the iTIP itself are added, as `e-cal-backend-caldav.c` adds them.
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

/// Whether one change may ask the server to tell the guests.
///
/// Only while the switch is on, and only when the user did not say not to
/// send (`E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`, which `ECalMetaBackend`
/// also keeps with an offline change and replays on upload).
pub fn itip(server_schedules: bool, opflags: ECalOperationFlags) -> Itip {
    if server_schedules && opflags & E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE == 0 {
        Itip::Send
    } else {
        Itip::Suppress
    }
}

/// The account switch, as `source` states it. See the module comment.
///
/// No source is "off": nothing is claimed for a backend EDS has not finished
/// constructing.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource` that outlives the call.
pub unsafe fn server_handles_invitations(source: *mut ESource) -> bool {
    if source.is_null() {
        return false;
    }
    // A lookup by name walks the registered children of
    // `E_TYPE_SOURCE_EXTENSION`, so the type must exist first.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe { e_source_webdav_get_type() };
    // Asked without creating the group: this is the user's own account
    // source, and a group created only to be read would turn the switch off.
    // SAFETY: a valid source by this function's contract, and the name is
    // the header's own constant for an `ESourceWebdav`.
    match unsafe {
        extension_if_present::<ESourceWebdav>(source, E_SOURCE_EXTENSION_WEBDAV_BACKEND)
    } {
        None => true,
        // SAFETY: a live extension, by `extension_if_present`'s contract.
        Some(webdav) => unsafe { e_source_webdav_get_calendar_auto_schedule(webdav) != GFALSE },
    }
}
