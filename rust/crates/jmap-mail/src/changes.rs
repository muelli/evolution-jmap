// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelFolderChangeInfo`: what a folder tells Camel it has changed.
//!
//! Filling a summary is only half of a refresh. Evolution draws a folder's
//! message list once and then keeps it up to date from the folder's `changed`
//! signal, whose one argument is this: four lists of uids — added, removed,
//! changed, recent. A folder that rewrote its summary and emitted nothing has
//! new mail that appears when the user next clicks away and back, and a folder
//! that emits on every poll redraws a list the user is reading.
//!
//! ## Owned, because the struct is not a GObject
//!
//! `CamelFolderChangeInfo` is a plain struct behind a boxed type, allocated
//! with `camel_folder_change_info_new` and freed with `_free`; nothing
//! reference-counts it. [`Changes`] is that pair written as a Rust type, so the
//! one path out of [`crate::summary::apply_listing`] that could otherwise leak
//! it — an early return between the allocation and the emission — cannot.
//! `camel_folder_changed` does not take ownership: it hands the pointer to the
//! signal's handlers, which read it and are done by the time the emission
//! returns, so the borrow this type keeps handing out is the whole contract.
//!
//! ## The fourth list, and what may go on it
//!
//! `uid_recent` is what runs the user's incoming filters — `CamelFolder`'s own
//! `changed` handler asks the session for a filter driver the moment a folder
//! with `CAMEL_FOLDER_FILTER_RECENT` reports one — so putting a uid on it is
//! asking for that message to be filed, forwarded or deleted by the user's
//! rules. Only one of the two ways this provider learns what a mailbox holds
//! may answer the question at all:
//!
//! - A **listing** may not. It finds the whole mailbox and cannot tell a
//!   message that has just arrived from one that was always there, so its
//!   "added" is every message the user already had — see
//!   [`crate::summary::apply_listing`], which never calls [`Changes::arrive`].
//! - A **delta** may. `Email/changes` is asked from a state the folder itself
//!   recorded at its last refresh, so a message it names that this folder has
//!   no row for is one that reached the mailbox since then. That is exactly
//!   what recent means, and [`crate::summary::apply_delta`] is where it is
//!   said.
//!
//! Camel keeps the two lists independently — `camel_folder_change_info_recent_uid`
//! does not imply `_add_uid` — so an arrival is recorded on both, which is why
//! [`Changes::arrive`] is a second call beside [`Changes::add`] rather than a
//! replacement for it.

use std::ffi::CStr;

use eds_sys::{
    CamelFolderChangeInfo, camel_folder_change_info_add_uid, camel_folder_change_info_change_uid,
    camel_folder_change_info_changed, camel_folder_change_info_free,
    camel_folder_change_info_get_added_uids, camel_folder_change_info_get_changed_uids,
    camel_folder_change_info_get_recent_uids, camel_folder_change_info_get_removed_uids,
    camel_folder_change_info_new, camel_folder_change_info_recent_uid,
    camel_folder_change_info_remove_uid,
};
use glib_sys::{GFALSE, GPtrArray};

use crate::folder_info::c_string;

/// The uids one listing moved, owned until Camel has been shown them.
pub struct Changes {
    info: *mut CamelFolderChangeInfo,
}

impl Changes {
    /// An empty diff — which is what most refreshes end with.
    pub fn new() -> Self {
        // SAFETY: no arguments; the allocation is owned by this value from here
        // until `drop`.
        Self {
            info: unsafe { camel_folder_change_info_new() },
        }
    }

    /// Records a uid that was not in the folder before.
    pub fn add(&mut self, uid: &str) {
        // SAFETY: the info is live for this value's lifetime, and the uid is
        // NUL-terminated, alive across the call, and copied by it.
        unsafe { camel_folder_change_info_add_uid(self.info, c_string(uid).as_ptr()) };
    }

    /// Records a uid whose message reached the mailbox since the state the
    /// folder was refreshed from — the one thing only a delta knows.
    ///
    /// Beside [`Changes::add`] and not instead of it: Camel's two lists are
    /// separate, and a uid that is only recent is one the message list is never
    /// told to draw a line for.
    pub fn arrive(&mut self, uid: &str) {
        // SAFETY: as above.
        unsafe { camel_folder_change_info_recent_uid(self.info, c_string(uid).as_ptr()) };
    }

    /// Records a uid whose row has gone.
    pub fn remove(&mut self, uid: &str) {
        // SAFETY: as above.
        unsafe { camel_folder_change_info_remove_uid(self.info, c_string(uid).as_ptr()) };
    }

    /// Records a uid whose row is still there and reads differently.
    pub fn change(&mut self, uid: &str) {
        // SAFETY: as above.
        unsafe { camel_folder_change_info_change_uid(self.info, c_string(uid).as_ptr()) };
    }

    /// Whether there is anything to tell Camel about.
    ///
    /// This is the test the refresh vfunc emits on, so it is Camel's own
    /// `camel_folder_change_info_changed` rather than a count kept here: the
    /// two could disagree — a uid added and removed within one listing is the
    /// obvious way — and the one that decides whether a message list redraws
    /// should be the one Camel would have asked.
    pub fn is_empty(&self) -> bool {
        // SAFETY: the info is live for this value's lifetime.
        unsafe { camel_folder_change_info_changed(self.info) == GFALSE }
    }

    /// The pointer `camel_folder_changed` is given. Borrowed: the emission
    /// reads the lists and is finished with them when it returns.
    pub fn as_ptr(&self) -> *mut CamelFolderChangeInfo {
        self.info
    }
}

/// The four lists, for the tests and for anything that wants to read a diff
/// back rather than hand it over.
impl Changes {
    pub fn added(&self) -> Vec<String> {
        // SAFETY: the info is live and the accessor borrows one of its arrays.
        unsafe { uids(camel_folder_change_info_get_added_uids(self.info)) }
    }

    pub fn removed(&self) -> Vec<String> {
        // SAFETY: as above.
        unsafe { uids(camel_folder_change_info_get_removed_uids(self.info)) }
    }

    pub fn changed(&self) -> Vec<String> {
        // SAFETY: as above.
        unsafe { uids(camel_folder_change_info_get_changed_uids(self.info)) }
    }

    pub fn recent(&self) -> Vec<String> {
        // SAFETY: as above.
        unsafe { uids(camel_folder_change_info_get_recent_uids(self.info)) }
    }
}

impl Default for Changes {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        // SAFETY: the one allocation, made in `new` and never handed over.
        unsafe { camel_folder_change_info_free(self.info) };
    }
}

/// A borrowed array of uids, copied out as strings.
///
/// # Safety
///
/// `array` must be NULL or a live `GPtrArray` of NUL-terminated strings.
unsafe fn uids(array: *mut GPtrArray) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: the contract above; every string lives as long as the array,
    // which outlives this call.
    unsafe {
        (0..(*array).len)
            .map(|index| {
                let uid = (*array).pdata.add(index as usize).read();
                CStr::from_ptr(uid.cast()).to_string_lossy().into_owned()
            })
            .collect()
    }
}
