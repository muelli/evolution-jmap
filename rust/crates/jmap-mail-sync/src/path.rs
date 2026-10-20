// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mailbox names as Camel folder paths.
//!
//! Camel identifies a folder by a `/`-separated path. The path is the key of
//! `camel_store_get_folder`, it occurs in the `folder://` URIs of Evolution
//! filters and account settings, and it names the folder directory in the
//! on-disk summary cache. A JMAP mailbox name is a display string that can
//! contain all characters except NUL, and it is unique only among its
//! siblings (RFC 8621 §2). Thus the mapping cannot be the identity, and it
//! must have these two properties:
//!
//! * **Injective.** If two mailboxes map to one path, the store returns the
//!   mail of the wrong folder. The encoding percent-encodes each character
//!   that makes a name ambiguous: `/` (which adds a hierarchy level) and `%`
//!   (the escape character). Thus distinct names give distinct paths.
//! * **Safe as a filename.** A component of `.` or `..` is a legal mailbox
//!   name, but a directory traversal in the summary cache. A NUL truncates
//!   the path in C. The encoding also encodes these.
//!
//! A name of ordinary text does not change, so the paths in the saved
//! filters of a user stay readable.

/// The separator Camel joins path components with.
pub(crate) const SEPARATOR: char = '/';

/// Render one mailbox name as one path component.
pub(crate) fn encode_component(name: &str) -> String {
    // Encode `.` and `..` only as whole components: `.hidden` is a good
    // filename. No other name gives these encodings, because `%` is escaped.
    match name {
        "." => return "%2E".to_owned(),
        ".." => return "%2E%2E".to_owned(),
        _ => {}
    }

    let mut encoded = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '%' => encoded.push_str("%25"),
            SEPARATOR => encoded.push_str("%2F"),
            '\0' => encoded.push_str("%00"),
            _ => encoded.push(character),
        }
    }
    encoded
}

/// Split a path into the path of its parent (if it has one) and its last
/// component.
///
/// Callers outside the crate need this because Camel names the new location
/// of a folder by path. The store must get the new parent and the new
/// component from the string of `rename_folder_sync`, and only this module
/// knows the separator.
///
/// `encode_component` escapes the separator, so the last separator always
/// starts the last component. `FolderTree::insert` reads a path the same way.
pub fn split(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once(SEPARATOR) {
        Some((parent, component)) => (Some(parent), component),
        None => (None, path),
    }
}

/// Join a parent path and an already-encoded component.
pub(crate) fn join(parent: Option<&str>, component: &str) -> String {
    match parent {
        Some(parent) => format!("{parent}{SEPARATOR}{component}"),
        None => component.to_owned(),
    }
}
