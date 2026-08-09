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
use crate::pointer;

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

    /// The set as a summary row's two columns: [`Keywords::new`] run backwards.
    ///
    /// Exactly backwards, which is what the match below is for — a keyword that
    /// went in as a bit of the flags word and came back out as a label would be
    /// a message Evolution showed as unread and tagged `$seen`. The names are
    /// matched folded, because that is the form the set is keyed by and a server
    /// that shouts `$Seen` is naming the same keyword.
    ///
    /// [`MessageFlags::attachments`] is never set: it is the one field of that
    /// word which is not a keyword at all — `hasAttachment` is a property RFC
    /// 8621 §4.1.1 has the server compute — so a set cannot carry it and the bit
    /// has to come from the listing that mentioned it.
    ///
    /// The labels come back in folded order rather than the order they arrived
    /// in, which is the only order a set has.
    pub fn split(&self) -> (MessageFlags, Vec<String>) {
        let mut flags = MessageFlags::default();
        let mut tags = Vec::new();
        for (folded, name) in &self.0 {
            match folded.as_str() {
                keyword::SEEN => flags.seen = true,
                keyword::ANSWERED => flags.answered = true,
                keyword::FLAGGED => flags.flagged = true,
                keyword::DRAFT => flags.draft = true,
                keyword::FORWARDED => flags.forwarded = true,
                keyword::JUNK => flags.junk = true,
                keyword::NOT_JUNK => flags.not_junk = true,
                _ => tags.push(name.clone()),
            }
        }
        (flags, tags)
    }

    /// This set with `change` applied to it.
    ///
    /// A [`KeywordChange`] read as something that has *happened* rather than
    /// something to make happen — the same value, from the other side. It exists
    /// for the case where the change and the set have different origins: a
    /// refresh meets a row carrying a change of the user's that has not been
    /// sent yet, and what the row should now hold is the listing with that
    /// change replayed on top of it, not the listing alone. Overwriting would
    /// undo the user's click on screen; ignoring the listing would hide what
    /// another client did.
    ///
    /// Everything the change does not name is left exactly as this set has it,
    /// which is the same conservatism [`KeywordChange::between`] is built on:
    /// a keyword neither end mentions is one nothing here can speak for.
    ///
    /// A keyword the change adds keeps this set's spelling if it is already
    /// here, which matters because a later removal has to quote the name the
    /// server holds.
    pub fn patched(&self, change: &KeywordChange) -> Self {
        let mut keywords = self.clone();
        for name in &change.cleared {
            keywords.0.remove(&name.to_lowercase());
        }
        for name in &change.set {
            keywords.insert(name);
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

    /// How many keywords the set holds — which is how many names
    /// [`Keywords::iter`] yields, and not how many were put in: two spellings
    /// of one keyword are one keyword.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn insert(&mut self, name: &str) {
        self.0
            .entry(name.to_lowercase())
            .or_insert_with(|| name.to_owned());
    }
}

/// A set from the names it holds, folded on the way in like every other way of
/// building one.
///
/// This is the inverse of [`Keywords::iter`], and it exists for the same reason
/// the iterator does: the keywords the last listing found have to survive the
/// process that found them, and the only thing there is to store is the names.
/// A list read back from wherever they were kept is therefore turned into a set
/// here rather than trusted to be one — nothing about a file on disk guarantees
/// it does not name one keyword twice.
impl FromIterator<String> for Keywords {
    fn from_iter<I: IntoIterator<Item = String>>(names: I) -> Self {
        let mut keywords = Self::default();
        for name in names {
            keywords.insert(&name);
        }
        keywords
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
            patch.insert(pointer::member(KEYWORDS, name), Value::Bool(true));
        }
        for name in &self.cleared {
            patch.insert(pointer::member(KEYWORDS, name), Value::Null);
        }
        Value::Object(patch)
    }
}

/// The property a keyword change patches.
const KEYWORDS: &str = "keywords";
