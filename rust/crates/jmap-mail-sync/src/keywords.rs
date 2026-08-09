// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a flag change is on the wire: the difference between two keyword sets.
//!
//! [`crate::message`] reads keywords — a server's `keywords` object becomes a
//! [`MessageFlags`] word and a list of labels. This is the same mapping run
//! backwards, and it is the first thing in this crate that writes.
//!
//! ## A difference, not a state
//!
//! The obvious shape for "the user marked this read" is to send the row's whole
//! keyword set and let the server replace what it has. It is also wrong twice
//! over. A `keywords` object holds everything every client ever put on the
//! message: a label from the user's phone, a `$phishing` verdict from the
//! server's own filter, a keyword this provider has no name for. A whole-set
//! write says something about each of them — and what it says is "gone" for any
//! that arrived after the listing this row came from. Sending only the
//! difference between what the last listing saw and what the row claims now
//! leaves every keyword neither side mentions exactly as it was, which is the
//! only thing a client that does not hold a lock can honestly claim about them.
//!
//! The second reason is that the two ends of the difference are cheap to have
//! and the whole set is not: Camel hands a folder a row that has changed, and
//! the keywords it *had* are what the summary was filled from.
//!
//! ## Keys are pointers
//!
//! RFC 8620 §5.3 makes each key of a `PatchObject` a JSON Pointer (RFC 6901)
//! into the object being patched, so `keywords/$seen` is one keyword and not a
//! property called `keywords/$seen`. That matters because an RFC 5788 keyword
//! is an IMAP atom, and an atom may contain `/` and `~` — the two characters a
//! pointer gives a meaning to. Unescaped, a user's `home/todo` label would
//! address a `todo` member of a `home` object inside `keywords`, inventing
//! structure rather than setting a keyword; [`KeywordChange::patch`] escapes
//! both.

use std::collections::BTreeMap;

use jmap_proto::mail::keyword;
use serde_json::{Map, Value};

use crate::message::MessageFlags;

/// The keywords of one message: [`MessageFlags`] and the labels beside it, as
/// the single set a JMAP server keeps.
///
/// Case-insensitive, because RFC 8621 §4.1.1 takes its keyword vocabulary from
/// RFC 5788 and IMAP keywords are compared without regard to case. A server
/// that stores a label as `Work` and a row that spells it `work` hold the same
/// keyword, and a comparison that missed that would rewrite it on every
/// synchronisation — so the set is keyed by the folded name while remembering
/// the spelling it arrived with, which is the name a removal has to quote.
///
/// Two spellings of one keyword in the same set collapse to the first, in
/// folded order: they are one keyword, and only one of them can be the name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keywords(BTreeMap<String, String>);

impl Keywords {
    /// The keyword set a summary row amounts to.
    ///
    /// [`MessageFlags::attachments`] contributes nothing: it is the one bit of
    /// that word that comes from a property of the message — `hasAttachment`,
    /// which RFC 8621 §4.1.1 has the *server* compute — rather than from a
    /// label. Sending it back as a keyword would put a label on the message
    /// that every other client would then show.
    ///
    /// The tags are taken verbatim, including any leading `$`: a label is only
    /// the same label as the one on the server if it is spelled the same way.
    pub fn new(flags: &MessageFlags, tags: &[String]) -> Self {
        let mut keywords = Self::default();
        for (set, name) in [
            (flags.seen, keyword::SEEN),
            (flags.answered, keyword::ANSWERED),
            (flags.flagged, keyword::FLAGGED),
            (flags.draft, keyword::DRAFT),
            (flags.forwarded, keyword::FORWARDED),
            (flags.junk, keyword::JUNK),
            (flags.not_junk, keyword::NOT_JUNK),
        ] {
            if set {
                keywords.insert(name);
            }
        }
        for tag in tags {
            keywords.insert(tag);
        }
        keywords
    }

    /// The keywords as they are spelled, in folded order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.values().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn insert(&mut self, name: &str) {
        self.0
            .entry(name.to_lowercase())
            .or_insert_with(|| name.to_owned());
    }
}

/// What has to happen on the server for one keyword set to become another.
///
/// Built by [`KeywordChange::between`] and spent by
/// [`MailSync::set_keywords`](crate::MailSync::set_keywords). Empty is the
/// ordinary case rather than an edge one — Camel marks a row as needing a
/// write for reasons that are not keywords at all — and an empty change is a
/// request that is never sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeywordChange {
    /// Keywords to add, spelled as the row spells them.
    set: Vec<String>,
    /// Keywords to remove, spelled as the *server* spelled them: the key a
    /// patch takes off the object has to be the key the object has.
    cleared: Vec<String>,
}

impl KeywordChange {
    /// The change from `before` to `after`.
    ///
    /// A keyword in both is in neither half of the result, even if the two
    /// spell it differently: re-setting a keyword that is already set is a
    /// write nobody asked for, over a member another client may be changing.
    pub fn between(before: &Keywords, after: &Keywords) -> Self {
        Self {
            set: after
                .0
                .iter()
                .filter(|(folded, _)| !before.0.contains_key(*folded))
                .map(|(_, name)| name.clone())
                .collect(),
            cleared: before
                .0
                .iter()
                .filter(|(folded, _)| !after.0.contains_key(*folded))
                .map(|(_, name)| name.clone())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.cleared.is_empty()
    }

    /// The change as the `PatchObject` an `Email/set` update takes.
    ///
    /// `true` sets a keyword and `null` removes it — RFC 8621 §4.1.1 gives the
    /// value of a set keyword as `true` and nothing else, so "not set" is the
    /// member being absent rather than being `false`.
    pub fn patch(&self) -> Value {
        let mut patch = Map::new();
        for name in &self.set {
            patch.insert(pointer(name), Value::Bool(true));
        }
        for name in &self.cleared {
            patch.insert(pointer(name), Value::Null);
        }
        Value::Object(patch)
    }
}

/// One keyword, as the JSON Pointer that addresses it inside an `Email`.
///
/// The escapes are RFC 6901 §3's and are applied in its order — `~` first, or
/// the `~1` produced for a `/` would be read again and become a `/`.
fn pointer(name: &str) -> String {
    format!("keywords/{}", name.replace('~', "~0").replace('/', "~1"))
}
