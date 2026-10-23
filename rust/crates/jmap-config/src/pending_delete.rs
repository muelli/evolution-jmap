// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The due time of a deferred collection delete: a custom `ESourceExtension`,
//! `[JMAP Pending Delete]` in the keyfile of a child source.
//!
//! [`jmap_collection_sync::pending`] tells why a delete waits for an undo
//! window. The registry writes the due time here when the user deletes a
//! collection. Evolution reads it to show the info bar with "Undo". Thus both
//! processes register this type at module load. The due time must stay after a
//! restart of the registry, so it is a property with `E_SOURCE_PARAM_SETTING`.
//! [`crate::oauth2`] tells why only such a property gets to the `.source` file.

use std::ffi::CStr;

use eds_sys::ESource;
use jmap_backend_core::setting_extension::{self, Setting, SettingExtension};
use jmap_collection_sync::pending::Due;

/// The keyfile group name, and the `extension_name` of `e_source_get_extension`.
pub const EXTENSION_NAME: &CStr = c"JMAP Pending Delete";

/// The property that holds the due time: seconds since the Unix epoch, or zero
/// for no pending delete. The keyfile key is `Due`.
pub const PROPERTY_DUE: &CStr = c"due";

/// The due time as a [`Setting`]: one int64 property, `due`.
pub struct PendingDelete;

impl Setting for PendingDelete {
    const TYPE_NAME: &'static CStr = c"JmapPendingDeleteExtension";
    const EXTENSION_NAME: &'static CStr = EXTENSION_NAME;
    const PROPERTY: &'static CStr = PROPERTY_DUE;
    type Value = i64;
}

/// The registered extension type of [`PendingDelete`].
pub type Extension = SettingExtension<PendingDelete>;

/// Registers [`Extension`], or finds the existing registration.
///
/// The module load of each process that parses a JMAP child source must call
/// this. EDS ignores a keyfile group with no registered type, so without this
/// call a restart loses the due time. [`read`] and [`write`] also call it.
pub fn ensure_registered() {
    setting_extension::ensure_registered::<PendingDelete>();
}

/// Returns the due time of `source`, or `None` if no delete is pending.
///
/// A read does not add the extension to the source.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn read(source: *mut ESource) -> Option<Due> {
    // SAFETY: `source` is valid by the contract.
    Due::from_stamp(unsafe { setting_extension::read::<PendingDelete>(source) }?)
}

/// Writes `due` to `source`. `None` clears a pending delete.
///
/// The write goes through the GObject property, so it emits `notify`. The
/// registry sends the changed source to Evolution on that signal. A clear on a
/// source without the extension does nothing and does not add the extension.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn write(source: *mut ESource, due: Option<Due>) {
    // SAFETY: `source` is valid by the contract.
    if due.is_none() && unsafe { setting_extension::read::<PendingDelete>(source) }.is_none() {
        return;
    }
    // SAFETY: `source` is valid by the contract.
    unsafe { setting_extension::write::<PendingDelete>(source, due.map_or(0, Due::stamp)) };
}
