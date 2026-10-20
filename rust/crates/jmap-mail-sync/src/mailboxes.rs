// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Message filing: the change that moves or copies a message to a mailbox.
//!
//! Camel has one vfunc for this: `transfer_messages_to_sync`, with a source
//! folder, a destination folder and a flag that removes the originals. In
//! JMAP, one property does both. `mailboxIds` (RFC 8621 §4.1.1) is the set of
//! mailboxes that contain a message. A copy adds a member. A move adds one
//! member and removes another. In one account, a mailbox is like a label: the
//! message is one object, and `Email/copy` copies only between accounts.
//!
//! ## A message is always in a mailbox
//!
//! RFC 8621 §4.1.1: an `Email` in the mail store belongs to one or more
//! mailboxes. Thus, a move is one patch, not two requests. If the first
//! request removes the source, the server refuses it. If the first request
//! adds the destination and the second fails, the message stays in both
//! mailboxes. The server applies one `Email/set` update as one change.
//!
//! For the same reason, [`Filing::moved`] into the current mailbox gives an
//! empty patch. Otherwise, one pointer must be both `true` and `null`, and
//! each result is wrong.
//!
//! ## What is not decided here
//!
//! The source mailbox comes from the caller, and this module does not check
//! it. A Camel folder knows its own mailbox id, so a check costs a round trip
//! for no new data. A `null` for an absent member is not an error: RFC 8620
//! §5.3 removes the member, and the member is already absent.

use jmap_proto::Id;
use serde_json::{Map, Value};

use crate::pointer;

/// The property a filing patches.
pub(crate) const MAILBOX_IDS: &str = "mailboxIds";

/// The patch that removes a message from one mailbox and adds it to none.
///
/// This is not a [`Filing`], because each filing has a destination mailbox.
/// The server accepts this patch only if the message stays in a different
/// mailbox (RFC 8621 §4.1.1). Otherwise, the server refuses the write.
///
/// [`MailSync::expunge_message`](crate::MailSync::expunge_message) adds the
/// check of which mailboxes contain the message.
pub(crate) fn out_of(mailbox: &Id) -> Value {
    let mut patch = Map::new();
    patch.insert(pointer::member(MAILBOX_IDS, mailbox.as_str()), Value::Null);
    Value::Object(patch)
}

/// What has to happen on the server for a message to be filed somewhere else.
///
/// [`Filing::copied_into`] or [`Filing::moved`] makes it, and
/// [`MailSync::file_message`](crate::MailSync::file_message) sends it. Like
/// [`KeywordChange`](crate::KeywordChange), it is a difference, not a state.
/// A write of the full set removes mailboxes that another client added after
/// the last listing.
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
    /// A move into the source mailbox is empty; see [`Filing::is_empty`].
    pub fn moved(out_of: Id, into: Id) -> Self {
        Self {
            into,
            out_of: Some(out_of),
        }
    }

    /// Whether this filing does not move the message. The provider does not
    /// send an empty filing.
    pub fn is_empty(&self) -> bool {
        self.out_of.as_ref() == Some(&self.into)
    }

    /// The filing as the `PatchObject` an `Email/set` update takes.
    ///
    /// `true` adds the message to a mailbox and `null` removes it. RFC 8621
    /// §4.1.1 permits only `true` as a value, so "not in it" is an absent
    /// member, not `false`.
    ///
    /// Both members go in one object. RFC 8620 §5.3 defines the result of a
    /// `PatchObject`, not an order of steps. Thus, no intermediate state has
    /// zero mailboxes, and the server has no reason to refuse it.
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
