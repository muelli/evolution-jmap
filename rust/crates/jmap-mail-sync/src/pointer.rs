// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The patch key that addresses one member of a JMAP object.
//!
//! RFC 8620 §5.3 makes each `PatchObject` key a JSON Pointer (RFC 6901), so
//! `keywords/$seen` addresses one keyword. Each patch in this crate addresses
//! a map member (a keyword or a mailbox) whose name comes from the server.
//! A pointer gives `/` and `~` a meaning: unescaped, a member called `a/b`
//! addresses a `b` member of an `a` object that does not exist.

/// The patch key for `name` inside `property`.
pub(crate) fn member(property: &str, name: &str) -> String {
    format!(
        "{property}/{}",
        jmap_proto::text::escape_pointer_segment(name)
    )
}
