// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mailbox names as Camel folder paths.
//!
//! Camel identifies a folder by a `/`-separated path — it is the key
//! `camel_store_get_folder` takes, it appears in the `folder://` URIs
//! Evolution stores in filters and account settings, and it names the folder's
//! directory in the on-disk summary cache. A JMAP mailbox name is none of
//! those things: it is a display string that may contain any character except
//! NUL, and uniqueness is only promised among siblings (RFC 8621 §2).
//!
//! So the mapping cannot be the identity, and it has two jobs beyond looking
//! reasonable:
//!
//! * **Injective.** Two mailboxes that map onto one path mean a store that
//!   hands back the wrong folder's mail. Every character that would make a
//!   name ambiguous — `/`, which would invent a hierarchy level, and `%`,
//!   which is the escape itself — is percent-encoded, so decoding is
//!   unambiguous and distinct names stay distinct.
//! * **Safe as a filename.** A component of `.` or `..` is a legal mailbox
//!   name and a directory traversal in the summary cache, and a NUL would
//!   truncate the path on the way to C. Those are encoded too.
//!
//! The overwhelmingly common case — a name of ordinary text — comes through
//! verbatim, which keeps the paths in a user's saved filters readable.

/// The separator Camel joins path components with.
pub(crate) const SEPARATOR: char = '/';

/// Render one mailbox name as one path component.
pub(crate) fn encode_component(name: &str) -> String {
    // `.` and `..` have to be encoded as whole components rather than
    // per-character: a name of `.hidden` is a perfectly good filename, and
    // encoding its dot would make paths noisy for no gain. Nothing else can
    // produce these strings, because a name containing `%` has it escaped.
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

/// Join a parent path and an already-encoded component.
pub(crate) fn join(parent: Option<&str>, component: &str) -> String {
    match parent {
        Some(parent) => format!("{parent}{SEPARATOR}{component}"),
        None => component.to_owned(),
    }
}
