// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A flag change on the wire: the difference between two keyword sets.
//!
//! [`crate::message`] converts a server's `keywords` object into
//! [`MessageFlags`] and a list of labels. This module does the reverse
//! mapping.
//!
//! ## A difference, not a state
//!
//! A `keywords` object holds the keywords of all clients: a label from the
//! user's phone, a `$phishing` mark from the server's filter, a keyword with
//! no name in this provider. A write of the full set removes each keyword that
//! arrived after the last listing. Thus, the provider sends only the
//! difference between the last listing and the row. Keywords that neither
//! side names stay as they are.
//!
//! Also, the two ends of the difference are easy to get, but the full server
//! set is not. Camel gives a folder the changed row, and the summary holds the
//! keywords from the last listing.
//!
//! ## Keys are pointers
//!
//! RFC 8620 §5.3 makes each `PatchObject` key a JSON Pointer (RFC 6901), so
//! `keywords/$seen` addresses one keyword. An RFC 5788 keyword is an IMAP
//! atom, and an atom can contain `/` and `~`, which have a meaning in a
//! pointer. Without escapes, a `home/todo` label addresses a `todo` member of
//! a `home` object. [`KeywordChange::patch`] escapes both characters.

use std::collections::BTreeMap;

use jmap_proto::mail::keyword;
use serde_json::{Map, Value};

use crate::message::MessageFlags;
use crate::pointer;

/// The keywords of one message: [`MessageFlags`] and the labels, as the single
/// set that a JMAP server keeps.
///
/// The set ignores case, because RFC 8621 §4.1.1 uses the RFC 5788 keywords,
/// and IMAP compares keywords without case. Thus, `Work` and `work` are the
/// same keyword, and a sync does not write it again. The key is the lowercase
/// name. The value is the original spelling, which a removal must use.
///
/// If one keyword arrives in two spellings, the set keeps the spelling that
/// it got first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keywords(BTreeMap<String, String>);

impl Keywords {
    /// The keyword set of a summary row.
    ///
    /// The set ignores [`MessageFlags::attachments`]. That bit comes from
    /// `hasAttachment`, which the server calculates (RFC 8621 §4.1.1). As a
    /// keyword, it becomes a label that all other clients show.
    ///
    /// The tags stay verbatim, with any leading `$`, so that they match the
    /// server's labels.
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

    /// The set as the two columns of a summary row: the reverse of
    /// [`Keywords::new`].
    ///
    /// A flag keyword must give a flag bit, not a label. The match uses the
    /// lowercase key, so `$Seen` also sets the seen bit.
    ///
    /// This never sets [`MessageFlags::attachments`], because `hasAttachment`
    /// is a server property, not a keyword. The listing must supply that bit.
    ///
    /// The labels come back in lowercase key order, not in arrival order.
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
    /// Use this when a refresh finds a row with a user change that is not sent
    /// yet. The row then holds the new listing with the change applied again.
    /// Thus, the refresh keeps the user's change and also shows the changes
    /// of other clients.
    ///
    /// Keywords that the change does not name stay as they are in this set.
    ///
    /// If an added keyword is already in this set, it keeps this set's
    /// spelling, because a later removal must use the server's name.
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

    /// The number of keywords in the set, which is the number of names that
    /// [`Keywords::iter`] gives. Two spellings of one keyword count as one.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn insert(&mut self, name: &str) {
        self.0
            .entry(name.to_lowercase())
            .or_insert_with(|| name.to_owned());
    }
}

/// A set from a list of names, which the set folds to lowercase keys.
///
/// This is the reverse of [`Keywords::iter`]. The provider stores the names of
/// the last listing's keywords, and this converts them back into a set. A
/// stored list can name one keyword twice, so the set removes duplicates.
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
/// [`KeywordChange::between`] makes it, and
/// [`MailSync::set_keywords`](crate::MailSync::set_keywords) sends it. An
/// empty change is usual, because Camel marks rows as changed for other
/// reasons. The provider does not send an empty change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeywordChange {
    /// Keywords to add, spelled as the row spells them.
    set: Vec<String>,
    /// Keywords to remove, spelled as the server spells them, because the
    /// patch key must match the key on the server.
    cleared: Vec<String>,
}

impl KeywordChange {
    /// The change from `before` to `after`.
    ///
    /// A keyword in both sets is not in the result, also if the spellings are
    /// different. Thus, the change does not write a keyword that is already
    /// set.
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
    /// `true` sets a keyword and `null` removes it. RFC 8621 §4.1.1 permits only
    /// `true` as a value, so a keyword that is not set is absent, not `false`.
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
