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
//! * **Stable across the C boundary.** `jmap-mail`'s `c_string` rewrites a
//!   raw CR or LF to U+FFFD when the path crosses into Camel, so the path
//!   Camel echoes back to a later `get_folder_sync` call would differ from
//!   the one stored here unless the rewrite already happened. Escaping CR
//!   and LF here keeps the two forms identical, so a mailbox named with
//!   either character can still be opened and does not alias a sibling
//!   mailbox whose name only differs by one of them.
//!
//! A name of ordinary text does not change, so the paths in the saved
//! filters of a user stay readable.

/// The separator Camel joins path components with.
pub(crate) const SEPARATOR: char = '/';

/// Render one mailbox name as one path component.
pub(crate) fn encode_component(name: &str) -> String {
    // Encode `.`, `..` and the empty name only as whole components: `.hidden`
    // is a good filename, but the empty component makes a path end in a
    // separator or, at the root, be empty. No other name gives these
    // encodings, because every `%` the loop below writes is always escaped.
    match name {
        "" => return "%".to_owned(),
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
            '\r' => encoded.push_str("%0D"),
            '\n' => encoded.push_str("%0A"),
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

#[cfg(test)]
mod tests {
    use super::encode_component;

    #[test]
    fn a_cr_or_lf_in_a_name_does_not_survive_into_the_path() {
        assert_eq!(encode_component("Inbox\rX"), "Inbox%0DX");
        assert_eq!(encode_component("Inbox\nX"), "Inbox%0AX");
        assert_eq!(encode_component("Inbox\r\nX"), "Inbox%0D%0AX");
    }

    /// `jmap-mail`'s `c_string` rewrites a `\r` or `\n` to U+FFFD at the
    /// C boundary (SECURITY-AUDIT item 167(a), Area 13). Before this fix,
    /// two names differing only in that rewrite, such as `"A\rB"` and
    /// `"A\u{fffd}B"`, encoded to two paths that collided after that later
    /// rewrite: Camel saw one folder name for both mailboxes. Escaping the
    /// control character here, before it ever reaches that boundary, keeps
    /// the two paths distinct so the rewrite downstream is a no-op.
    #[test]
    fn a_name_with_a_raw_control_character_does_not_collide_with_its_u_fffd_lookalike() {
        assert_ne!(encode_component("A\rB"), encode_component("A\u{fffd}B"));
        assert_ne!(encode_component("A\nB"), encode_component("A\u{fffd}B"));
    }
}
