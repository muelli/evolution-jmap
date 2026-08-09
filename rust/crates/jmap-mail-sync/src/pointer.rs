// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! One member of a JMAP object, as the key that addresses it in a patch.
//!
//! RFC 8620 §5.3 makes each key of a `PatchObject` a JSON Pointer (RFC 6901)
//! into the object being patched, so `keywords/$seen` is one keyword rather
//! than a property whose name contains a slash. Every patch this crate builds
//! addresses a member of a map — a keyword, a mailbox — whose name came off
//! the network, and `/` and `~` are the two characters a pointer gives a
//! meaning to. Left alone they would let the name invent structure: a member
//! called `a/b` would address a `b` member of an `a` object that is not there.
//!
//! So this is where a name becomes a key, in one place rather than once per
//! property — the escaping is the same everywhere, and a copy of it that fell
//! behind would be a hole rather than a duplication.

/// The patch key for `name` inside `property`.
///
/// The escapes are RFC 6901 §3's and are applied in its order — `~` first, or
/// the `~1` produced for a `/` would be read again and become a `/`.
pub(crate) fn member(property: &str, name: &str) -> String {
    format!("{property}/{}", name.replace('~', "~0").replace('/', "~1"))
}
