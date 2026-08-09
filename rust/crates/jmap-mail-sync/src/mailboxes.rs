// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a message is filed, as the change that files it somewhere else.
//!
//! Camel has one vfunc for this — `transfer_messages_to_sync`, taking a source
//! folder, a destination folder and a flag saying whether the originals go —
//! and JMAP answers both of its halves with one property. RFC 8621 §4.6 makes
//! `mailboxIds` the set of mailboxes a message is in, so a copy adds a member
//! and a move adds one and takes another away. There is no `Email/copy` and no
//! `Email/move`, because in JMAP a mailbox is closer to a label than to a
//! directory: the message is one object either way, and the only difference
//! between a copy and a move is how many mailboxes end up naming it.
//!
//! ## A message is always somewhere
//!
//! RFC 8621 §4.6 spends one sentence on the rule this module is built around:
//! an `Email` in the mail store belongs to one or more `Mailbox`es. It is what
//! makes a move one patch rather than two requests — a request that removed the
//! source first would be refused for leaving the message nowhere, and one that
//! added the destination first would leave the message filed in both if the
//! second request never happened, which is a copy the user did not ask for and
//! that nothing afterwards knows to clean up. One `Email/set` update is applied
//! by the server as one change, so the message is in the old mailbox or the new
//! one and never in neither.
//!
//! It is also why [`Filing::moved`] into the mailbox a message is already in is
//! empty rather than a patch: the same pointer would have to be both `true` and
//! `null`, and whichever of the two won, the answer would be wrong — a message
//! filed where it already was, or a message filed nowhere.
//!
//! ## What is not decided here
//!
//! Which mailbox a message *came from* is the caller's claim, not something
//! this checks: a Camel folder knows its own mailbox id, and asking the server
//! to confirm it would be a round trip spent re-reading what the summary the
//! user clicked in already said. A `null` for a member that is not there is not
//! an error — RFC 8620 §5.3 removes the member, and a member that was already
//! absent is already removed — so a stale claim costs the message nothing.

use jmap_proto::Id;
use serde_json::{Map, Value};

use crate::pointer;

/// The property a filing patches.
const MAILBOX_IDS: &str = "mailboxIds";

/// What has to happen on the server for a message to be filed somewhere else.
///
/// Built by [`Filing::copied_into`] or [`Filing::moved`] and spent by
/// [`MailSync::file_message`](crate::MailSync::file_message), in the shape
/// [`KeywordChange`](crate::KeywordChange) has for the other thing a folder
/// writes: a difference rather than a state. A whole-set write would say
/// something about every mailbox the message is in — including the ones another
/// client filed it into after the listing this filing was decided from — and
/// what it would say is "gone".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filing {
    /// The mailbox the message is filed into.
    into: Id,
    /// The mailbox it is filed out of, for a move.
    out_of: Option<Id>,
}

impl Filing {
    /// The message gains `into` and keeps every mailbox it is already in.
    pub fn copied_into(into: Id) -> Self {
        Self { into, out_of: None }
    }

    /// The message gains `into` and loses `out_of`, in one change.
    ///
    /// A move into the mailbox the message is being moved out of is the one
    /// filing that has nothing to say; see [`Filing::is_empty`].
    pub fn moved(out_of: Id, into: Id) -> Self {
        Self {
            into,
            out_of: Some(out_of),
        }
    }

    /// Whether this filing would leave the message exactly where it is, and is
    /// therefore a request that is never sent.
    pub fn is_empty(&self) -> bool {
        self.out_of.as_ref() == Some(&self.into)
    }

    /// The filing as the `PatchObject` an `Email/set` update takes.
    ///
    /// `true` files the message into a mailbox and `null` takes it out — RFC
    /// 8621 §4.6 gives the value of a set member as `true` and nothing else, so
    /// "not in it" is the member being absent rather than being `false`.
    ///
    /// Both members go in one object, and the object has no order: RFC 8620
    /// §5.3 defines what a `PatchObject` *results in*, not a sequence of steps,
    /// so there is no member a server is entitled to apply first and no
    /// intermediate state with no mailbox in it for it to refuse. That is the
    /// property a two-request move does not have, and it is why this is one
    /// patch.
    pub fn patch(&self) -> Value {
        let mut patch = Map::new();
        if self.is_empty() {
            return Value::Object(patch);
        }
        patch.insert(
            pointer::member(MAILBOX_IDS, self.into.as_str()),
            Value::Bool(true),
        );
        if let Some(out_of) = &self.out_of {
            patch.insert(pointer::member(MAILBOX_IDS, out_of.as_str()), Value::Null);
        }
        Value::Object(patch)
    }
}
