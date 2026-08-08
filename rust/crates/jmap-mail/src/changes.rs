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
//! ## Three lists, not four
//!
//! `uid_recent` is deliberately never filled, and that is a decision rather
//! than a gap. Camel's recent list is what runs the user's incoming filters —
//! `CamelFolder`'s own `changed` handler asks the session for a filter driver
//! the moment a folder with `CAMEL_FOLDER_FILTER_RECENT` reports one — and a
//! JMAP listing cannot tell a message that has just arrived from one that was
//! always there. The first refresh of an account finds the whole mailbox, so
//! "added" and "recent" would be the same list, and the user's rules would file,
//! forward or delete every message they already had. What could honestly answer
//! the question is `Email/changes` against a state from the previous session,
//! which is a later increment; until then the answer is that nothing is recent.

use std::ffi::CStr;

use eds_sys::{
    CamelFolderChangeInfo, camel_folder_change_info_add_uid, camel_folder_change_info_change_uid,
    camel_folder_change_info_changed, camel_folder_change_info_free,
    camel_folder_change_info_get_added_uids, camel_folder_change_info_get_changed_uids,
    camel_folder_change_info_get_recent_uids, camel_folder_change_info_get_removed_uids,
    camel_folder_change_info_new, camel_folder_change_info_remove_uid,
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
