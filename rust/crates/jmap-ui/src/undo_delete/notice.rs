// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the info bar after a collection delete shows, and for how long.
//!
//! [`jmap_collection_sync::pending`] has the rules of the undo window. This
//! module only reads them: an info bar shows while [`decide`] tells to wait.

use std::time::SystemTime;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceResource, e_source_dup_display_name, e_source_get_enabled,
    e_source_has_extension, e_source_resource_get_identity, e_source_set_enabled,
};
use glib_sys::{GFALSE, GTRUE};
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::marshal::{extension_if_present, read_string, take_string};
use jmap_collection_sync::pending::{Due, Pending, decide};
use jmap_collection_sync::{ChildKind, split_shared_identity};

/// The state of a child source that decides the info bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    pub kind: ChildKind,
    /// `[Resource] Identity`: a collection id, or `<accountId>/<collectionId>`
    /// for a collection that another user shares.
    pub identity: String,
    pub display_name: String,
    pub enabled: bool,
    pub due: Option<Due>,
}

/// The text and the timer of one info bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub kind: ChildKind,
    pub primary: String,
    pub secondary: String,
    /// The time until the destroy, in whole seconds, rounded up.
    pub seconds: u32,
}

impl Deleted {
    /// Reads `source`. Returns `None` for a source that is not an address book
    /// or a calendar.
    ///
    /// # Safety
    ///
    /// `source` must be a valid `ESource`.
    pub unsafe fn read(source: *mut ESource) -> Option<Self> {
        // SAFETY: a valid source by the contract of this function. Each string
        // that a getter returns stays valid while the source is unchanged.
        unsafe {
            let kind = if e_source_has_extension(source, E_SOURCE_EXTENSION_ADDRESS_BOOK.as_ptr())
                != GFALSE
            {
                ChildKind::AddressBook
            } else if e_source_has_extension(source, E_SOURCE_EXTENSION_CALENDAR.as_ptr()) != GFALSE
            {
                ChildKind::Calendar
            } else {
                return None;
            };
            let identity =
                extension_if_present::<ESourceResource>(source, E_SOURCE_EXTENSION_RESOURCE)
                    .and_then(|resource| read_string(e_source_resource_get_identity(resource)))
                    .unwrap_or_default();
            Some(Self {
                kind,
                identity,
                display_name: take_string(e_source_dup_display_name(source)).unwrap_or_default(),
                enabled: e_source_get_enabled(source) != GFALSE,
                due: jmap_config::pending_delete::read(source),
            })
        }
    }

    /// The info bar for this state at `now`, or `None` if no info bar shows.
    pub fn notice(&self, now: SystemTime) -> Option<Notice> {
        let Pending::Wait(left) = decide(self.enabled, self.due, now) else {
            return None;
        };
        let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);
        let shared = split_shared_identity(&self.identity).is_some();
        let name = self.display_name.as_str();
        let (primary, secondary) = match (self.kind, shared) {
            (ChildKind::Calendar, true) => (
                // TRANSLATORS: %1$s is the name of the calendar.
                translate_with(c"Calendar \"%1$s\" is deleted.", &[name]),
                translate(
                    c"The calendar is deleted for all users who share it. To keep it, click Undo before this message closes.",
                ),
            ),
            (ChildKind::Calendar, false) => (
                // TRANSLATORS: %1$s is the name of the calendar.
                translate_with(c"Calendar \"%1$s\" is deleted.", &[name]),
                translate(
                    c"The calendar is deleted on the server. To keep it, click Undo before this message closes.",
                ),
            ),
            (ChildKind::AddressBook, true) => (
                // TRANSLATORS: %1$s is the name of the address book.
                translate_with(c"Address book \"%1$s\" is deleted.", &[name]),
                translate(
                    c"The address book is deleted for all users who share it. To keep it, click Undo before this message closes.",
                ),
            ),
            (ChildKind::AddressBook, false) => (
                // TRANSLATORS: %1$s is the name of the address book.
                translate_with(c"Address book \"%1$s\" is deleted.", &[name]),
                translate(
                    c"The address book is deleted on the server. To keep it, click Undo before this message closes.",
                ),
            ),
        };
        Some(Notice {
            kind: self.kind,
            primary,
            secondary,
            seconds: u32::try_from(seconds).unwrap_or(u32::MAX),
        })
    }
}

/// Undoes the delete on the local copy of `source`: enables it, then clears the
/// due time. The caller writes the source to the registry.
///
/// The order is the order of the registry. An enabled source with a due time is
/// an undo there, so a stop between the two changes keeps the collection.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn undo(source: *mut ESource) {
    // SAFETY: a valid source by the contract of this function.
    unsafe {
        e_source_set_enabled(source, GTRUE);
        jmap_config::pending_delete::write(source, None);
    }
}

/// The kind of collection that the shell view `name` shows, or `None` for a
/// view that shows no info bar.
pub fn view_kind(name: &str) -> Option<ChildKind> {
    match name {
        "calendar" => Some(ChildKind::Calendar),
        "addressbook" => Some(ChildKind::AddressBook),
        _ => None,
    }
}
