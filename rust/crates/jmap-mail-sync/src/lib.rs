// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail synchronisation, in the shape that Camel asks for.
//!
//! One JMAP account is one `CamelJmapStore`. This crate syncs that account:
//! the folders, their messages, and the source of each message. Each entry
//! point maps to one Camel vfunc:
//!
//! - [`MailSync::folder_tree`] maps to `get_folder_info_sync`.
//! - [`MailSync::messages`] fills the `CamelFolderSummary` of a folder.
//! - [`MailSync::message_source`] supplies the bytes that `get_message_sync`
//!   parses.
//!
//! Like `jmap-book-sync` and `jmap-cal-sync`, this crate does not use GObject
//! or the Camel headers. Thus you can test it against `jmap-mockd` on any
//! machine. Two mappings have no equivalent on the address book and calendar
//! side. The first is the path encoding: a mailbox name is a display string,
//! and a Camel path is an identifier. The second is the tree: JMAP uses parent
//! pointers, and Camel uses a linked forest.

pub(crate) mod date;
pub mod error;
pub mod folder;
pub mod identity;
pub mod keywords;
pub mod mailboxes;
pub mod message;
pub mod path;
pub(crate) mod pointer;
pub mod send;

use std::collections::{BTreeMap, BTreeSet};

use jmap_client::Client;
use jmap_proto::mail::{Email, EmailImport, EmailQueryFilter, Mailbox};
use jmap_proto::methods::{Comparator, Filter, QueryRequest, QueryResponse};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL};
use jmap_proto::{Id, State, UtcDate};

pub use error::SyncError;
pub use folder::{FolderDelta, FolderInfo, FolderListChange, FolderRole, FolderTree};
pub use keywords::{KeywordChange, Keywords};
pub use mailboxes::Filing;
pub use message::{
    MessageFlags, MessageSummary, SOURCE_PROPERTIES, SUMMARY_PROPERTIES, download_ceiling,
};
pub use send::{Outgoing, OutgoingMailboxes};

/// The result of a folder-list refresh.
///
/// This type does not apply a delta folder by folder. A Camel path comes from
/// the ancestors of a mailbox. When `Mailbox/changes` reports a renamed parent,
/// it does not report the descendants whose paths also changed. One
/// `Mailbox/get` returns all mailboxes of the account, so any change causes a
/// full rebuild. The delta is useful when it reports no change, which is
/// almost always.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderUpdate {
    /// No change. Use the state for the next request. This state can be
    /// later than the state in the request.
    Unchanged(State),
    /// The current tree, and the state of that listing.
    Rebuilt { state: State, tree: FolderTree },
}

/// The result of a mailbox refresh that asked the server for the changes.
///
/// Each variant is a different answer, not a different level of confidence:
///
/// - [`MessageUpdate::Unchanged`]: one round trip shows that the folder is
///   current. Almost every poll gets this answer.
/// - [`MessageUpdate::Changed`]: a delta. It does not use the
///   created/updated/destroyed categories. `Email/changes` reports on all
///   messages of the account, but a folder asks about one mailbox. JMAP files
///   a message with an ordinary update to its `mailboxIds`. Thus a delta that
///   names a message does not tell if the message moved in or out of this
///   mailbox. This function gets each named message again and reads its
///   mailbox. `present` contains the full rows that this mailbox holds for the
///   changed messages. `absent` contains the uids that are not in this mailbox
///   now: destroyed, filed in a different mailbox, or never in it.
/// - [`MessageUpdate::Relisted`]: the full mailbox. The server returns it when
///   it cannot calculate changes from the state. This function also returns it
///   when a full listing costs less than the delta (see `catch_up_limit`).
///
/// The caller compares `present` and `absent` with its rows, because only the
/// caller has these rows. On the wire, a message that moved into this mailbox
/// and a message with changed flags are the same delta. The existing row tells
/// which of the two it is.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageUpdate {
    /// No mail in the account changed. Use the state for the next request.
    /// This state can be later than the state in the request.
    Unchanged(State),
    /// The rows this mailbox holds for the changed messages, the uids it does
    /// not hold, and the state of this answer.
    Changed {
        state: State,
        /// Rows of this mailbox, oldest first, in the same order as a listing.
        present: Vec<MessageSummary>,
        /// Uids that are not in this mailbox, in sorted order.
        absent: Vec<Id>,
    },
    /// The full mailbox listing, and the state of that listing.
    Relisted {
        state: State,
        messages: Vec<MessageSummary>,
    },
}

/// Synchronises one JMAP mail account.
pub struct MailSync {
    client: Client,
    account_id: Id,
}

impl MailSync {
    pub fn new(client: Client, account_id: Id) -> Self {
        Self { client, account_id }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn account_id(&self) -> &Id {
        &self.account_id
    }

    /// All folders of the account, and the state of the listing
    /// (`get_folder_info_sync`).
    ///
    /// One `Mailbox/get` returns the full tree. JMAP cannot get one level only,
    /// mailbox lists are small, and `CAMEL_STORE_FOLDER_INFO_RECURSIVE` asks
    /// for the full tree.
    ///
    /// The state is necessary for [`MailSync::folder_tree_since`]. Without a
    /// state, the only possible refresh is a full listing.
    pub fn folder_tree(&self) -> Result<(State, FolderTree), SyncError> {
        let response = self.client.mailbox_get(&self.account_id)?;
        Ok((response.state, FolderTree::from_mailboxes(&response.list)?))
    }

    /// The folder tree again, but only if the mailboxes of the account changed
    /// after `since` (the refresh part of `get_folder_info_sync`).
    ///
    /// Usually this is one `Mailbox/changes` that reports no change. Any change
    /// causes a full listing, for the reason that [`FolderUpdate`] gives.
    ///
    /// If the server cannot calculate changes from the state, this function
    /// returns the listing and not an error. The state can be too old or from a
    /// different server. EDS backends report this condition, because EDS can
    /// compare a collection with its cache. Camel cannot do this, so an error
    /// would block the folder tree permanently. Stalwart reports an
    /// unrecognized state as `invalidArguments`, not as the RFC
    /// `cannotCalculateChanges`. This function also returns the listing for
    /// that error.
    pub fn folder_tree_since(&self, since: &State) -> Result<FolderUpdate, SyncError> {
        match self.client.all_changes(&self.account_id, "Mailbox", since) {
            Ok(changes) if changes.is_empty() => Ok(FolderUpdate::Unchanged(changes.new_state)),
            Ok(_) => self.rebuild(),
            Err(error)
                if error.is_cannot_calculate_changes() || error.is_unrecognized_changes_state() =>
            {
                self.rebuild()
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The listing, with its own state and not the state of the delta. The
    /// account can change again between the two calls.
    fn rebuild(&self) -> Result<FolderUpdate, SyncError> {
        let (state, tree) = self.folder_tree()?;
        Ok(FolderUpdate::Rebuilt { state, tree })
    }

    /// All messages in one mailbox, oldest first, and the state for the next
    /// refresh. This fills the summary of a folder.
    ///
    /// This function uses two steps, not one `Email/query` with an `Email/get`
    /// back-reference. A back-reference sends all matching ids to one `/get`,
    /// and a mailbox can hold more ids than one `/get` can name. Separate steps
    /// let the fetch occur in chunks.
    ///
    /// The order is oldest first, because Camel builds a summary and numbers
    /// messages in that order. The sort key is `receivedAt`, not the `Date`
    /// header. The `Date` header comes from the clock of the sender, and an
    /// incorrect clock puts the message in the incorrect position permanently.
    ///
    /// ## Why the state is read first
    ///
    /// This costs one more round trip: an `Email/get` with no ids. The `/get`
    /// calls below also return a state, but that state is from after the
    /// query. A message that arrives between the query and the fetch is then
    /// not in the listing. No later delta reports it, because it changed before
    /// that state. The folder does not show it until the next full listing.
    ///
    /// When the state is read first, the next delta can report changes again
    /// that this listing already has. That is safe: the result is only a
    /// repeated lookup and an identical row write.
    pub fn messages(&self, mailbox: &Id) -> Result<(State, Vec<MessageSummary>), SyncError> {
        let state = self.client.email_state(&self.account_id)?;
        let ids = self.message_ids(mailbox)?;

        // `/get` can answer in any order (RFC 8620 §5.1). The code below puts
        // the rows back in the query order.
        let mut by_uid: BTreeMap<Id, MessageSummary> = BTreeMap::new();
        for email in self.fetch(&ids, SUMMARY_PROPERTIES)? {
            let summary = MessageSummary::from_email(&email)?;
            by_uid.insert(summary.uid.clone(), summary);
        }

        // If the `/get` does not return an id from the query, the message was
        // deleted between the two calls. This is not a failure, and it gets no
        // row. `remove` also lists only once a message that two pages return.
        Ok((
            state,
            ids.iter().filter_map(|id| by_uid.remove(id)).collect(),
        ))
    }

    /// The current contents of one mailbox, compared with the contents at
    /// `since`. This is the refresh part of the folder summary fill.
    ///
    /// Usually this is one `Email/changes` that reports no change. If the
    /// account changed, the cost is one `Email/get` for each chunk of named
    /// messages. For a mailbox that got one new message, this is much less than
    /// a listing, which gets all rows again.
    ///
    /// [`MessageUpdate`] explains why the result is present/absent and not
    /// created/updated/destroyed. `Email/changes` is about the full account,
    /// so this function reads the mailbox membership again. It accepts
    /// `destroyed` as given: a destroyed message is not in any mailbox.
    ///
    /// If the server cannot calculate changes from the state, this function
    /// returns the full mailbox and not an error.
    /// [`MailSync::folder_tree_since`] does the same, for the same reason:
    /// Camel cannot report the error, so the folder would not recover. The
    /// state can be too old, from a different server, or unrecognized.
    /// Stalwart reports an unrecognized state as `invalidArguments`, not as the
    /// RFC `cannotCalculateChanges`.
    ///
    /// `held` is the number of rows that the caller has for this mailbox. It
    /// only decides when a full listing costs less than the delta (see
    /// `catch_up_limit`). The caller supplies it because the server would need
    /// one more round trip. It is only a cost estimate: an incorrect value
    /// gives the same rows at a higher cost.
    pub fn messages_since(
        &self,
        mailbox: &Id,
        since: &State,
        held: usize,
    ) -> Result<MessageUpdate, SyncError> {
        let changes = match self.client.all_changes(&self.account_id, "Email", since) {
            Ok(changes) if changes.is_empty() => {
                return Ok(MessageUpdate::Unchanged(changes.new_state));
            }
            Ok(changes) => changes,
            Err(error)
                if error.is_cannot_calculate_changes() || error.is_unrecognized_changes_state() =>
            {
                return self.relist(mailbox);
            }
            Err(error) => return Err(error.into()),
        };

        // Created and updated go into one list. Neither category tells if the
        // message is in this mailbox (see `MessageUpdate`).
        let touched: Vec<Id> = changes
            .created
            .iter()
            .chain(changes.updated.iter())
            .cloned()
            .collect();

        // The fetch below must look up each id to find its mailbox. A delta
        // from an old state names all messages the account changed since then.
        // A listing of this mailbox can cost less (see `catch_up_limit`).
        //
        // `destroyed` does not count, because this function does not fetch
        // those ids.
        if touched.len() > catch_up_limit(held, self.objects_in_get()) {
            return self.relist(mailbox);
        }

        let mut absent = changes.destroyed;
        let mut present = Vec::new();
        for email in self.fetch(&touched, &filing_properties())? {
            // RFC 8621 §4.1.1 sets each `mailboxIds` value to `true`. A `false`
            // entry for this mailbox is a protocol violation. Refuse it, as
            // `message_mailboxes` does; do not read it as absence.
            let filed = match email
                .mailbox_ids
                .as_ref()
                .and_then(|mailboxes| mailboxes.get(mailbox).copied())
            {
                Some(false) => {
                    return Err(SyncError::protocol(format!(
                        "Email/get returned {} with a false-valued mailboxIds entry for {mailbox}",
                        email
                            .id
                            .as_ref()
                            .map(Id::to_string)
                            .unwrap_or_else(|| "<no id>".to_string())
                    )));
                }
                Some(true) => true,
                None => false,
            };
            let summary = MessageSummary::from_email(&email)?;
            match filed {
                true => present.push(summary),
                false => {
                    absent.insert(summary.uid);
                }
            }
        }

        // If the `/get` does not return an id from the delta, the message was
        // destroyed between the two calls. A listing drops the id. Here, report
        // it as absent, because the folder can have a row for it.
        let answered: BTreeSet<&Id> = present.iter().map(|message| &message.uid).collect();
        let unanswered: Vec<Id> = touched
            .into_iter()
            .filter(|id| !answered.contains(id) && !absent.contains(id))
            .collect();
        absent.extend(unanswered);

        // Use the listing order, because these rows go into the same summary:
        // oldest first by server time, then by uid for equal or missing times.
        // Thus each refresh gives the same order.
        present.sort_by(|one, other| {
            one.received_at
                .cmp(&other.received_at)
                .then_with(|| one.uid.cmp(&other.uid))
        });

        Ok(MessageUpdate::Changed {
            state: changes.new_state,
            present,
            absent: absent.into_iter().collect(),
        })
    }

    /// The full mailbox listing, with its own state and not the state of the
    /// delta. [`MailSync::rebuild`] does the same for the folder tree.
    fn relist(&self, mailbox: &Id) -> Result<MessageUpdate, SyncError> {
        let (state, messages) = self.messages(mailbox)?;
        Ok(MessageUpdate::Relisted { state, messages })
    }

    /// The `Email` objects for `ids`, in the server order. This function uses
    /// as many `Email/get` calls as the account limit makes necessary.
    ///
    /// An empty list causes no calls. A delta with only destroyed messages
    /// gives an empty list.
    fn fetch(&self, ids: &[Id], properties: &[&str]) -> Result<Vec<Email>, SyncError> {
        let mut emails = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(self.objects_in_get()) {
            emails.extend(
                self.client
                    .email_get(&self.account_id, chunk, Some(properties))?,
            );
        }
        Ok(emails)
    }

    /// The RFC 5322 bytes of one message. `get_message_sync` parses them into
    /// a `CamelMimeMessage`.
    ///
    /// ## Why this function gets the blob id again
    ///
    /// [`MessageSummary`] has a blob id, but the caller cannot keep it. A
    /// `CamelFolderSummary` row has no field for it. The row is a Camel struct,
    /// and there is one row for each message. Thus the caller can only supply
    /// a uid, and the blob id costs one `Email/get`.
    ///
    /// That is one round trip for each opened message, but it is always
    /// correct. RFC 8621 §4.1 makes an `Email` immutable, but it does not
    /// prevent a server from issuing a new blob id. RFC 8620 §6.2 lets a server
    /// forget a blob id at any time. A cached blob id would then fail without
    /// warning.
    ///
    /// ## Failures
    ///
    /// A uid that the account does not hold gives [`SyncError::NoSuchMessage`],
    /// not a client error. A summary row that is older than its message is
    /// normal. A message without a `blobId` is a protocol violation. There is
    /// no fallback: a message built again from its body parts has different
    /// bytes, and its signature fails.
    pub fn message_source(&self, uid: &Id) -> Result<Vec<u8>, SyncError> {
        let email = self
            .client
            .email_get(
                &self.account_id,
                std::slice::from_ref(uid),
                Some(SOURCE_PROPERTIES),
            )?
            .into_iter()
            // An answer for a different message does not contain the bytes of
            // this message. Treat it as a missing message.
            .find(|email| email.id.as_ref() == Some(uid))
            .ok_or_else(|| SyncError::NoSuchMessage(uid.clone()))?;

        let blob_id = email.blob_id.ok_or_else(|| {
            SyncError::protocol(format!("Email/get returned {uid} without a blobId"))
        })?;
        // The size in the row sets the download limit, not a constant. See
        // [`download_ceiling`] for the margin, and `jmap_client::limits` for
        // why a limit is necessary.
        let ceiling = download_ceiling(email.size);

        // `{name}` in the download template is a filename hint for a browser.
        // Nothing reads it. The uid is safe in a URL: RFC 8620 §1.2 limits an
        // id to URL-safe characters.
        Ok(self
            .client
            .download_blob(&self.account_id, &blob_id, uid.as_str(), ceiling)?)
    }

    /// Writes the keyword change of one message to the server. This is the
    /// write part of folder synchronisation.
    ///
    /// When Camel marks a row as read, important, or labelled, this function
    /// sends one `Email/set` update. The update contains only the changed
    /// keywords, for the reasons that [`crate::keywords`] gives. An empty
    /// change sends no request. Camel also marks rows for writes that are not
    /// keyword changes, and a request for each of them would cost one round
    /// trip per row.
    ///
    /// A uid that the account does not hold gives [`SyncError::NoSuchMessage`],
    /// as in [`MailSync::message_source`]. A different client can destroy the
    /// message, and then the flag change is not necessary. All other refusals
    /// stay [`SyncError::Client`]. The user must know about an unaccepted
    /// keyword or a read-only mailbox.
    ///
    /// This function does not use `ifInState`. The folder holds the state of its
    /// listing, so a conditional write fails for any change to any other
    /// message in the account. The patch changes only the named keywords, so
    /// concurrent changes to different keywords do not conflict.
    pub fn set_keywords(&self, uid: &Id, change: &KeywordChange) -> Result<(), SyncError> {
        if change.is_empty() {
            return Ok(());
        }
        let account_id = self.account_id().to_string();
        let message_uid = uid.to_string();
        tracing::debug!(account_id, uid = message_uid, "setting message keywords");
        self.update_email(uid, change.patch()).map_err(|error| {
            tracing::warn!(account_id, uid = message_uid, %error, "message keyword change failed");
            error
        })
    }

    /// Files one message into a different mailbox
    /// (`transfer_messages_to_sync`).
    ///
    /// A copy and a move are both one `Email/set` update of `mailboxIds`, for
    /// the reasons that [`crate::mailboxes`] gives. A [`Filing`] that does not
    /// move the message sends no request.
    ///
    /// This function files one message for each call, as
    /// [`MailSync::set_keywords`] does. Camel gives the vfunc a list of uids.
    /// One `Email/set` can contain many updates, but a partial failure then
    /// does not tell which messages moved. One request per message gives one
    /// answer per message, and the caller must report each result.
    ///
    /// A uid that the account does not hold gives [`SyncError::NoSuchMessage`],
    /// as in all other writes here. A missing destination mailbox is different:
    /// the message is not affected, and the user must know that the folder is
    /// gone. That refusal stays [`SyncError::Client`].
    ///
    /// This function does not use `ifInState`, for the reason that
    /// [`MailSync::set_keywords`] gives. The patch changes only the named
    /// members, so it does not conflict with changes to other members.
    pub fn file_message(&self, uid: &Id, filing: &Filing) -> Result<(), SyncError> {
        if filing.is_empty() {
            return Ok(());
        }
        let account_id = self.account_id().to_string();
        let message_uid = uid.to_string();
        tracing::debug!(account_id, uid = message_uid, "filing message");
        self.update_email(uid, filing.patch()).map_err(|error| {
            tracing::warn!(account_id, uid = message_uid, %error, "message filing failed");
            error
        })
    }

    /// Removes one message from a mailbox permanently (`expunge_sync`).
    ///
    /// This is the only write here that reads before it writes. The cause is a
    /// difference between the two models. Camel asks a folder to remove its
    /// deleted messages. In IMAP, a message is in one mailbox, so this removes
    /// the message. RFC 8621 §4.1.1 makes `mailboxIds` a set, so one message can
    /// be in more than one mailbox. The two possible writes have different
    /// results:
    ///
    /// - **destroy** removes the message from the account. This is correct when
    ///   this mailbox is the last mailbox of the message. Otherwise, it causes
    ///   data loss: to empty the trash would also remove the copy of the user.
    /// - **`mailboxIds/<this>: null`** removes the message from this mailbox
    ///   only. This is correct when the message has a different mailbox. If
    ///   not, a server refuses it, because a message must be in a mailbox.
    ///
    /// Camel cannot tell the two cases apart. A summary row knows only the
    /// mailbox of its listing. Thus this function first reads `mailboxIds` (one
    /// `Email/get` of one property) and then selects the write. This costs one
    /// more round trip per message. Without it, the provider either loses mail
    /// or cannot empty a trash.
    ///
    /// If the message is not in `mailbox`, this function does nothing. A uid
    /// comes from the last listing, and a different client can move the message
    /// while the folder is open. A destroy based on an old row would delete
    /// mail. To remove an absent member is safe (RFC 8620 §5.3), but the
    /// request would have no effect.
    ///
    /// A uid that the account does not hold gives [`SyncError::NoSuchMessage`],
    /// as in all other writes here.
    ///
    /// This function does not use `ifInState`, for the reason that
    /// [`MailSync::set_keywords`] gives. Also, the relevant state is the
    /// membership of the message, and no JMAP condition can express it. The
    /// read makes the race window smaller, but does not remove it. If a
    /// different client files this message into a second mailbox between the
    /// read and the destroy, the message is lost. All unconditional `Email/set`
    /// writes have this window. To close it, the server must supply a "destroy
    /// if in no other mailbox" operation, and JMAP has none.
    pub fn expunge_message(&self, uid: &Id, mailbox: &Id) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let message_uid = uid.to_string();
        let mailbox_id = mailbox.to_string();
        let filed_in = self.message_mailboxes(uid).map_err(|error| {
            tracing::warn!(account_id, uid = message_uid, mailbox_id, %error, "message expunge failed");
            error
        })?;
        if !filed_in.contains(mailbox) {
            return Ok(());
        }
        tracing::debug!(
            account_id,
            uid = message_uid,
            mailbox_id,
            "expunging message"
        );
        if filed_in.len() > 1 {
            return self
                .update_email(uid, mailboxes::out_of(mailbox))
                .map_err(|error| {
                    tracing::warn!(account_id, uid = message_uid, mailbox_id, %error, "message expunge failed");
                    error
                });
        }
        self.client
            .email_destroy(&self.account_id, uid)
            .map_err(|error| match &error {
                jmap_client::Error::Set(set_error)
                    if set_error.error_type == jmap_proto::error::set::NOT_FOUND =>
                {
                    SyncError::NoSuchMessage(uid.clone())
                }
                _ => SyncError::Client(error),
            })
            .map_err(|error| {
                tracing::warn!(account_id, uid = message_uid, mailbox_id, %error, "message expunge failed");
                error
            })
    }

    /// Removes one message from a mailbox and keeps it in `trash` if no other
    /// mailbox holds it (a delete outside the trash).
    ///
    /// IMAPX with a real trash folder does the same: a deleted message moves to
    /// the trash, and only an expunge of the trash destroys it. The read and
    /// the race window are the same as in [`MailSync::expunge_message`]. A
    /// message that is not in `mailbox` is left as it is.
    pub fn trash_message(&self, uid: &Id, mailbox: &Id, trash: &Id) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let message_uid = uid.to_string();
        let mailbox_id = mailbox.to_string();
        let result = self.message_mailboxes(uid).and_then(|filed_in| {
            if !filed_in.contains(mailbox) {
                return Ok(());
            }
            tracing::debug!(
                account_id,
                uid = message_uid,
                mailbox_id,
                "trashing message"
            );
            let patch = if filed_in.len() > 1 {
                mailboxes::out_of(mailbox)
            } else {
                Filing::moved(mailbox.clone(), trash.clone()).patch()
            };
            self.update_email(uid, patch)
        });
        result.inspect_err(|error| {
            tracing::warn!(account_id, uid = message_uid, mailbox_id, %error, "message trash failed");
        })
    }

    /// The mailboxes that hold one message.
    ///
    /// The request names one property. The default property set of `Email/get`
    /// is large and includes body parts (RFC 8621 §4.2).
    ///
    /// An id that is not in the answer gives [`SyncError::NoSuchMessage`]. RFC
    /// 8620 §5.1 puts a missing id in `notFound` and not in the list.
    ///
    /// RFC 8621 §4.1.1 sets each `mailboxIds` value to `true`. A `false` value
    /// is a protocol violation, and no write is safe to select from it. Thus
    /// this function refuses it, as `identity_for` refuses an identity without
    /// an id. It does not count the value as membership or as absence.
    fn message_mailboxes(&self, uid: &Id) -> Result<BTreeSet<Id>, SyncError> {
        let found = self.client.email_get(
            &self.account_id,
            std::slice::from_ref(uid),
            Some(&[mailboxes::MAILBOX_IDS]),
        )?;
        let email = found
            .into_iter()
            .find(|email| email.id.as_ref() == Some(uid))
            .ok_or_else(|| SyncError::NoSuchMessage(uid.clone()))?;
        email
            .mailbox_ids
            .unwrap_or_default()
            .into_iter()
            .map(|(id, filed)| {
                if filed {
                    Ok(id)
                } else {
                    Err(SyncError::protocol(format!(
                        "Email/get returned {uid} with a false-valued mailboxIds entry for {id}"
                    )))
                }
            })
            .collect()
    }

    /// Puts a message that the caller holds into a mailbox
    /// (`append_message_sync`).
    ///
    /// This is the only write here that adds a message, and the only one that
    /// sends a message as bytes. Examples are a draft, a filtered message, or a
    /// message from a different account. The bytes go to the server unchanged.
    ///
    /// ## Two round trips, because the protocol has two
    ///
    /// First a blob upload (RFC 8620 §6.1), then an `Email/import` (RFC 8621
    /// §4.8) that names the blob. A method call cannot contain the bytes of a
    /// message. An `Email/set` create from `from`, `subject` and body values is
    /// a different operation: the server builds a new message from parts. That
    /// is correct for a new draft, but not for an existing message. The new
    /// bytes are different, so signatures over the original bytes fail.
    ///
    /// The upload has the type `message/rfc822`. RFC 8620 §6.1 lets the server
    /// return a different type. This function does not read it, because the
    /// import names the blob by id only.
    ///
    /// ## The date, and dates that cannot be sent
    ///
    /// `received_at` is the Camel `date_received`, in seconds since the epoch.
    /// The server default (RFC 8621 §4.8) is the date of the most recent
    /// `Received` header, or the import time. For a message copied from a
    /// different account, that gives the copy time. The message then sorts
    /// into the incorrect position.
    ///
    /// If a `UTCDate` cannot express the time, this function sends no date (see
    /// `date::utc_date`). The caller asked for the message, so a bad date must
    /// not stop the append.
    ///
    /// ## Return value
    ///
    /// The id from the server, which is the Camel uid. `append_message_sync`
    /// must return it, and the caller has no other name for the message. RFC
    /// 8620 §5.3 lets a server return only the properties that it set. Thus the
    /// other row data is not available. The next mailbox refresh builds the
    /// row.
    ///
    /// A refusal stays [`SyncError::Client`], also for a missing mailbox. This
    /// is not [`SyncError::NoSuchFolder`], as the folder writes give. Those
    /// writes name the mailbox as the changed record, and get `notFound`. An
    /// import names the mailbox in `mailboxIds`. The server then refuses the
    /// message with `invalidProperties`, the same error as for a bad blob. Only
    /// the description text tells which property failed, and this crate does
    /// not parse it.
    pub fn import_message(
        &self,
        mailbox: &Id,
        source: Vec<u8>,
        keywords: &Keywords,
        received_at: Option<i64>,
    ) -> Result<Id, SyncError> {
        let account_id = self.account_id().to_string();
        let mailbox_id = mailbox.to_string();
        tracing::debug!(account_id, mailbox_id, "importing message");
        let upload = self
            .client
            .upload_blob(&self.account_id, MESSAGE_MEDIA_TYPE, source)
            .map_err(|error| {
                tracing::warn!(account_id, mailbox_id, %error, "message import failed");
                error
            })?;

        let mut import = EmailImport::new(upload.blob_id, mailbox.clone());
        for keyword in keywords.iter() {
            import = import.keyword(keyword);
        }
        if let Some(received_at) = received_at.and_then(date::utc_date) {
            import = import.received_at(UtcDate::new(received_at));
        }

        let imported = self
            .client
            .email_import(&self.account_id, &import)
            .map_err(|error| {
                tracing::warn!(account_id, mailbox_id, %error, "message import failed");
                error
            })?;
        imported
            .id
            .ok_or_else(|| SyncError::protocol("Email/import created a message without an id"))
    }

    /// Sends one message (the `send_to_sync` of a `CamelTransport`).
    ///
    /// One upload and two requests, in this order, for the reasons that
    /// [`Outgoing`] gives. The import puts the message into the staging mailbox
    /// as a draft. The submission then names the imported id. Its patch files
    /// the message in the sent-mail mailbox when the server accepts it.
    ///
    /// The two requests are separate. RFC 8620 §5.3 lets an
    /// `EmailSubmission/set` name the creation id of the import, but the upload
    /// must occur first in all cases. One combined request saves one round trip
    /// of three. But then an import refusal and a submission refusal look the
    /// same. The user must know which one occurred: the account did not accept
    /// the message, or the message is in Drafts and was not sent.
    ///
    /// The return value is the id of the message in the account. The staging
    /// or destination mailbox lists it with this uid. The submission id is not
    /// returned, because this provider cannot undo a send.
    ///
    /// A refusal is [`SyncError::Client`], from the call that refused it.
    /// Examples: an unknown identity, a `From` that does not agree with the
    /// identity, a message larger than the upload limit. See [`Outgoing`] for
    /// what a refused submission leaves.
    ///
    /// This function calls [`MailSync::stage_outgoing_message`] and then
    /// [`MailSync::submit_staged_message`]. To retry after a credential
    /// failure, call those two separately, so that only the failed request
    /// occurs again. See `stage_outgoing_message` for the reason.
    pub fn send_message(&self, outgoing: Outgoing) -> Result<Id, SyncError> {
        let uid = self.stage_outgoing_message(&outgoing)?;
        self.submit_staged_message(&uid, &outgoing)?;
        Ok(uid)
    }

    /// The first request of [`MailSync::send_message`]: imports the bytes of
    /// `outgoing` into its staging mailbox as a draft.
    ///
    /// This is a separate call so that a caller can retry only the failed
    /// request after a credential refresh. The transport in `jmap-mail` does
    /// this. Assume that [`MailSync::submit_staged_message`] fails with a 401
    /// and the caller runs all of `send_message` again. The second run then
    /// imports the message again: the first import stays, and nothing tells
    /// this crate that the account has the message.
    pub fn stage_outgoing_message(&self, outgoing: &Outgoing) -> Result<Id, SyncError> {
        let keywords = Outgoing::staged_keywords();
        // No `receivedAt`: the message arrives now, and all clients of the
        // account read the time from the server clock. `import_message` traces
        // this part of the send.
        self.import_message(&outgoing.staging, outgoing.source.clone(), &keywords, None)
    }

    /// The second request of [`MailSync::send_message`]: gives `uid` to the
    /// submission function of the server. [`MailSync::stage_outgoing_message`]
    /// must import `uid` first.
    pub fn submit_staged_message(&self, uid: &Id, outgoing: &Outgoing) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let identity_id = outgoing.identity.to_string();
        let message_uid = uid.to_string();
        tracing::debug!(account_id, identity_id, message_uid, "submitting message");

        self.client
            .submit_email(
                &self.account_id,
                uid,
                &outgoing.identity,
                outgoing.envelope.clone(),
                outgoing.accepted_patch(),
            )
            .map_err(|error| {
                tracing::warn!(account_id, identity_id, uid = message_uid, %error, "message submission failed");
                error
            })?;
        Ok(())
    }

    /// The identity for a message sent from `address`. A `CamelTransport`
    /// needs it to fill in [`Outgoing::identity`].
    ///
    /// `address` is an addr-spec, as in `Envelope::mail_from` and in the Camel
    /// `from` argument: no display name and no angle brackets. `crate::identity`
    /// selects the identity that covers it, including the domain wildcard of
    /// RFC 8621 §6.
    ///
    /// **Use the envelope sender, not the `From` header.** The two can be
    /// different. The envelope sender is the account that the user selected in
    /// the composer. A `From` header can be a list address or a second author.
    /// RFC 8621 §7 makes the server compare the header with the identity. This
    /// side cannot do that check. A disagreement gives the server refusal and
    /// its reason.
    ///
    /// This function sends one `Identity/get` for each call, with no cache.
    /// The user can change identities from a different client, and a send is
    /// not frequent. A cache can then submit through a removed identity, or
    /// not find a new identity.
    ///
    /// If no identity covers the address, this function gives
    /// [`SyncError::NoIdentity`]. It does not use the first identity of the
    /// account, because the message would then go out under a different name.
    pub fn identity_for(&self, address: &str) -> Result<Id, SyncError> {
        let identities = self.client.identities(&self.account_id)?;
        let found = identity::best_match(&identities, address)
            .ok_or_else(|| SyncError::NoIdentity(address.to_owned()))?;
        // RFC 8620 §5.1 gives each `/get` record an id. A submission cannot
        // name an identity without an id, so report the violation.
        found.id.clone().ok_or_else(|| {
            SyncError::protocol(format!(
                "Identity/get returned an identity for {address} without an id"
            ))
        })
    }

    /// The two mailboxes for an outgoing message. A `CamelTransport` needs them
    /// to fill in [`Outgoing::staging`] and [`Outgoing::destination`].
    ///
    /// [`OutgoingMailboxes::of`] selects the mailboxes and handles a missing
    /// mailbox. This function adds the round trip: one `Mailbox/get` for each
    /// send, with no cache.
    ///
    /// The reason is the same as for [`MailSync::identity_for`]. A cached
    /// folder tree can stage a message in a mailbox that a different client
    /// deleted, or not find a new Sent folder. A send is not frequent, and it
    /// already includes an upload.
    ///
    /// This function does not use the [`FolderTree`] of the store, although
    /// that saves one request. The transport is a separate `CamelService` with
    /// no pointer to the store (see `crate::transport` in `jmap-mail`). A link
    /// here would add a relation that Camel does not have.
    ///
    /// If the account has no mailbox for an outgoing message, this function
    /// gives [`SyncError::NoOutgoingFolder`]. The caller gets this error before
    /// it uploads anything.
    pub fn outgoing_mailboxes(&self) -> Result<OutgoingMailboxes, SyncError> {
        let (_, tree) = self.folder_tree()?;
        OutgoingMailboxes::of(&tree).ok_or(SyncError::NoOutgoingFolder)
    }

    /// All `Quota` objects of the account (RFC 9425 §3). `get_quota_info_sync`
    /// in `jmap-mail` reads them to give the quota of a folder.
    ///
    /// One `Quota/get` with `ids: null`, which is how RFC 9425 asks for all
    /// quotas. The store does not know the quota ids before this call. JMAP
    /// quotas apply to the account, not to a mailbox. The caller selects the
    /// quotas that apply to the folder.
    pub fn quotas(&self) -> Result<Vec<jmap_proto::quota::Quota>, SyncError> {
        Ok(self.client.quotas(&self.account_id)?)
    }

    /// Sets whether the user wants to see a folder. Both vfuncs of
    /// `CamelSubscribable` use this write.
    ///
    /// One method for both vfuncs, because on the wire this is one
    /// `Mailbox/set` update. RFC 8621 §2 gives a mailbox the `isSubscribed`
    /// property. `subscribe_folder_sync` and `unsubscribe_folder_sync` only set
    /// different values.
    ///
    /// The value goes to the server, not to a local list in Evolution. That is
    /// the purpose of the property: a subscription applies to the account, so
    /// a folder hidden on one device is hidden on all devices.
    ///
    /// This function always writes, with no "no change" check as in
    /// [`MailSync::set_keywords`]. A keyword change already contains the
    /// difference. Here, only a folder listing can tell the current value, and
    /// a listing is a cache. A change from a different client makes it
    /// incorrect, and the check would then drop the write of the user. One
    /// more round trip is the safer cost.
    ///
    /// A mailbox that the account does not hold gives
    /// [`SyncError::NoSuchFolder`]. This is equivalent to
    /// [`MailSync::set_keywords`] for a missing message. A different client can
    /// delete the folder, and the subscription is then not necessary. All
    /// other refusals stay [`SyncError::Client`].
    pub fn set_subscribed(&self, mailbox: &Id, subscribed: bool) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let mailbox_id = mailbox.to_string();
        tracing::debug!(
            account_id,
            mailbox_id,
            subscribed,
            "setting mail folder subscription"
        );
        self.client
            .mailbox_update(
                &self.account_id,
                mailbox,
                serde_json::json!({ "isSubscribed": subscribed }),
            )
            .map_err(|error| {
                let error = folder_error(mailbox, error);
                tracing::warn!(account_id, mailbox_id, subscribed, %error, "mail folder subscription change failed");
                error
            })
    }

    /// Makes a folder (`create_folder_sync`).
    ///
    /// The return value is the full folder, not only its id. Camel needs it:
    /// `camel_store_create_folder_sync` gives the `CamelFolderInfo` directly to
    /// the folder tree of Evolution. With only the id, the caller must list the
    /// account again to get the Camel path. This crate makes the path from the
    /// parent path and the encoded name.
    ///
    /// `parent` is the folder above the new folder. It is a [`FolderInfo`],
    /// not an [`Id`]: the request needs the id, and the answer needs the path.
    /// Only the tree of the caller has both.
    ///
    /// **The name comes from the server if it is not blank, and from the
    /// request if it is blank.** RFC 8620 §5.3 lets a server return only the
    /// properties that it set for a created record. Thus an unchanged `name`
    /// can be absent from the response. [`Mailbox::name`] is not an `Option`,
    /// so an absent name is blank. A blank name must not give an empty path.
    /// A non-blank name shows a server normalisation (for example, trimmed
    /// whitespace or Unicode normalisation). `created_resource` in
    /// `jmap-collection-sync` does the same for address books and calendars.
    /// If this function ignores the server name, the cached path stays
    /// incorrect until the next full listing. [`created_folder_name`] makes the
    /// selection. The RFC always guarantees the id.
    ///
    /// The counts are zero and there are no children. A new mailbox has no
    /// mail and no child mailboxes yet. The role is `None`, because the request
    /// sets no role. A role read from the response would bypass the role
    /// selection of [`FolderTree`], which prevents two inboxes in one account.
    ///
    /// This function reads `isSubscribed` from the response if the server sent
    /// it. RFC 8621 §2 lets the server select the default, and the client
    /// cannot calculate it. If the server sent no value, the default is `true`.
    /// The user asked for this folder, so the user wants to see it. `false`
    /// would hide the new folder until the next listing.
    ///
    /// A refusal (for example, a duplicate sibling name or a missing parent)
    /// stays [`SyncError::Client`]. The server reason is a message for the
    /// user, and this crate cannot add to it.
    pub fn create_folder(
        &self,
        parent: Option<&FolderInfo>,
        name: &str,
    ) -> Result<FolderInfo, SyncError> {
        let requested = Mailbox {
            name: name.to_owned(),
            parent_id: parent.map(|parent| parent.id.clone()),
            ..Mailbox::default()
        };
        let account_id = self.account_id().to_string();
        tracing::debug!(account_id, name, "creating mail folder");
        let created = match self.client.mailbox_create(&self.account_id, &requested) {
            Ok(created) => created,
            Err(error) => {
                tracing::warn!(account_id, name, %error, "mail folder create failed");
                return Err(error.into());
            }
        };
        let id = created
            .id
            .ok_or_else(|| SyncError::protocol("Mailbox/set created a mailbox without an id"))?;
        let name = created_folder_name(&created.name, name);

        Ok(FolderInfo {
            id,
            path: path::join(
                parent.map(|parent| parent.path.as_str()),
                &path::encode_component(name),
            ),
            display_name: name.to_owned(),
            role: None,
            total: 0,
            unread: 0,
            subscribed: created.is_subscribed.unwrap_or(true),
            // The server was not asked for `myRights` on a create, and the
            // caller who just created the mailbox is its owner.
            may_add_items: None,
            children: Vec::new(),
        })
    }

    /// Removes a folder (`delete_folder_sync`).
    ///
    /// This function uses the mailbox id, not the path, as all other writes
    /// here do. The caller got the folder from a listing, and a rename from a
    /// different client can already make that path incorrect.
    ///
    /// This function does not send `onDestroyRemoveEmails`. In RFC 8621 §2.5,
    /// this argument also removes all messages in the folder. Camel did not ask
    /// for that, and it would delete the mail of the user. Instead, the server
    /// refuses with `mailboxHasChild` or `mailboxHasEmail`. These refusals stay
    /// unchanged as [`SyncError::Client`], for the caller to show to the user.
    ///
    /// These refusals have no variant of their own, but a missing folder has.
    /// A variant is necessary only when Camel has an error code for it. Camel
    /// has no code for these two. A conversion would lose the server
    /// description.
    ///
    /// A mailbox that the account does not hold gives
    /// [`SyncError::NoSuchFolder`], as in [`MailSync::set_subscribed`]. If a
    /// different client removed the folder first, the result is what the user
    /// asked for. It is not a failure.
    pub fn delete_folder(&self, mailbox: &Id) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let mailbox_id = mailbox.to_string();
        tracing::debug!(account_id, mailbox_id, "deleting mail folder");
        if let Err(error) = self.client.mailbox_destroy(&self.account_id, mailbox) {
            tracing::warn!(account_id, mailbox_id, %error, "mail folder delete failed");
            return Err(folder_error(mailbox, error));
        }
        Ok(())
    }

    /// Renames a folder, and moves it (`rename_folder_sync`).
    ///
    /// One method for both, because Camel uses one vfunc for both. A Camel path
    /// contains the folder name and its position. Thus "Work/Notes" to
    /// "Archive/Notes" and "Work/Notes" to "Work/Minutes" are the same call. On
    /// the wire, they are the `name` and `parentId` properties of one
    /// `Mailbox/set` update.
    ///
    /// **This function always sends both properties**, also `parentId: null`
    /// for a move to the top level. To send only changed values, this function
    /// needs the earlier values. Only the listing of the caller has them, and
    /// that is a cache. A move from a different client can make it incorrect,
    /// and the write would then be lost. The user asked for a position, and
    /// that position includes both properties. Without `parentId`, the returned
    /// path could be incorrect.
    ///
    /// The return value is the new Camel path of the folder, for the reason
    /// that [`MailSync::create_folder`] returns a full folder. The caller uses
    /// that path as the folder key, and only this crate can make it from a
    /// mailbox name. A rename does not change the counts, role, subscription,
    /// or children of the folder.
    ///
    /// `name` is a mailbox name, not a path component, as in
    /// [`MailSync::create_folder`]. A `/` in it is part of the name, and this
    /// function encodes it as `%2F`.
    ///
    /// This function does not use `ifInState`, for the reason that
    /// [`MailSync::set_keywords`] gives.
    ///
    /// A mailbox that the account does not hold gives
    /// [`SyncError::NoSuchFolder`], as in all other writes that name a
    /// mailbox. All other refusals stay [`SyncError::Client`]. Examples are a
    /// duplicate sibling name, a missing parent, or a move into the subtree of
    /// the folder. Each is a message for the user, and Camel has no code for
    /// them.
    pub fn rename_folder(
        &self,
        folder: &Id,
        parent: Option<&FolderInfo>,
        name: &str,
    ) -> Result<String, SyncError> {
        let account_id = self.account_id().to_string();
        let mailbox_id = folder.to_string();
        tracing::debug!(account_id, mailbox_id, name, "renaming mail folder");
        self.client
            .mailbox_update(
                &self.account_id,
                folder,
                serde_json::json!({
                    "name": name,
                    "parentId": parent.map(|parent| parent.id.as_str()),
                }),
            )
            .map_err(|error| {
                let error = folder_error(folder, error);
                tracing::warn!(account_id, mailbox_id, name, %error, "mail folder rename failed");
                error
            })?;

        Ok(path::join(
            parent.map(|parent| parent.path.as_str()),
            &path::encode_component(name),
        ))
    }

    /// One `Email/set` update. A `notFound` refusal gives
    /// [`SyncError::NoSuchMessage`], because the message is gone.
    fn update_email(&self, uid: &Id, patch: serde_json::Value) -> Result<(), SyncError> {
        self.client
            .email_update(&self.account_id, uid, patch)
            .map_err(|error| match &error {
                jmap_client::Error::Set(set_error)
                    if set_error.error_type == jmap_proto::error::set::NOT_FOUND =>
                {
                    SyncError::NoSuchMessage(uid.clone())
                }
                _ => SyncError::Client(error),
            })
    }

    /// The message ids of a mailbox, oldest first, from all pages of the
    /// server answer.
    fn message_ids(&self, mailbox: &Id) -> Result<Vec<Id>, SyncError> {
        self.paged_query(Filter::condition(EmailQueryFilter::in_mailbox(
            mailbox.clone(),
        )))
    }

    /// The message ids of a mailbox that agree with `filter`, for a search on
    /// the server (`folder.rs` in `jmap-mail` translates a Camel search
    /// expression). `Email/query` has no mailbox limit of its own. Thus this
    /// function adds the mailbox condition with an AND, as
    /// [`Self::message_ids`] does.
    pub fn search(
        &self,
        mailbox: &Id,
        filter: Filter<EmailQueryFilter>,
    ) -> Result<Vec<Id>, SyncError> {
        self.paged_query(Filter::and([
            Filter::condition(EmailQueryFilter::in_mailbox(mailbox.clone())),
            filter,
        ]))
    }

    /// Gets all pages of `Email/query` for `filter`, oldest first. It stops
    /// when the server does not report a limit, or returns an empty page.
    /// It fails when the ids go past the id budget of the client, or do not
    /// agree with the `total` of the server. EDS removes each cached message
    /// that a partial list does not have.
    fn paged_query(&self, filter: Filter<EmailQueryFilter>) -> Result<Vec<Id>, SyncError> {
        let mut ids: Vec<Id> = Vec::new();
        let mut budget = self.client.id_budget();

        for _ in 0..MAX_QUERY_PAGES {
            let mut request = QueryRequest::new(self.account_id.clone()).filter(filter.clone());
            request.sort = Some(vec![Comparator::ascending("receivedAt")]);
            request.position = ids.len() as i64;
            request.calculate_total = true;
            let arguments = self.client.single_call(
                &[CAPABILITY_CORE, CAPABILITY_MAIL],
                "Email/query",
                &request,
            )?;
            let response: QueryResponse =
                serde_json::from_value(arguments).map_err(jmap_client::Error::from)?;
            let capped = response.limit.is_some();
            let answered = !response.ids.is_empty();
            // Each id costs memory, also when a page repeats it.
            if !response.ids.iter().all(|id| budget.charge(id.as_str())) {
                return Err(SyncError::protocol(
                    "Email/query reports more ids than the budget for one sync",
                ));
            }
            ids.extend(response.ids);
            // No limit: all remaining ids are in this page. A limited empty
            // page: no ids remain.
            if !capped || !answered {
                return match response.total {
                    Some(total) if total != ids.len() as u64 => Err(SyncError::protocol(format!(
                        "Email/query gave {} ids, but reports a total of {total}",
                        ids.len()
                    ))),
                    _ => Ok(ids),
                };
            }
        }
        Err(SyncError::protocol(
            "Email/query never stopped reporting a limited answer",
        ))
    }

    /// The maximum number of ids in one `Email/get` for this account.
    ///
    /// This is the `maxObjectsInGet` of the server, if it has one. A request
    /// for more ids fails fully with `requestTooLarge`. RFC 8620 §2 requires
    /// the limit. If the server does not supply it, this function uses a small
    /// fallback value.
    ///
    /// The value also has a maximum. A server can advertise a limit larger than
    /// a mailbox. One `/get` for fifty thousand messages makes Evolution wait
    /// with the folder half open. Chunks keep each response small, at the
    /// cost of more round trips.
    fn objects_in_get(&self) -> usize {
        let advertised = self
            .client
            .session()
            .max_objects_in_get()
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|limit| *limit > 0)
            .unwrap_or(FALLBACK_OBJECTS_IN_GET);
        advertised.min(MAX_OBJECTS_PER_GET)
    }
}

/// The properties that a delta requests: the summary row properties, plus
/// `mailboxIds`.
///
/// `mailboxIds` is not in [`SUMMARY_PROPERTIES`]. A listing already knows the
/// mailbox, because its `Email/query` names one mailbox. [`MessageSummary`]
/// and `CamelFolderSummary` rows have no field for it. Only a delta needs it,
/// to put each message in `present` or `absent`.
fn filing_properties() -> Vec<&'static str> {
    let mut properties = SUMMARY_PROPERTIES.to_vec();
    properties.push("mailboxIds");
    properties
}

/// The name for a new folder: `server_name` if it is not blank (a server
/// normalisation), otherwise `requested_name` (RFC 8620 §5.3: no change).
///
/// `created_resource` in `jmap-collection-sync` makes the same blank or
/// non-blank selection for address books and calendars. A Fastmail server
/// sent no `name` in a create response, and did not echo the requested name.
fn created_folder_name<'a>(server_name: &'a str, requested_name: &'a str) -> &'a str {
    if server_name.trim().is_empty() {
        requested_name
    } else {
        server_name
    }
}

/// Converts a mailbox write error. A `notFound` refusal gives
/// [`SyncError::NoSuchFolder`], because the mailbox is gone.
///
/// The three writes that name a mailbox (subscribe, delete, rename) use this
/// function. Thus they cannot give different errors for the same `notFound`.
fn folder_error(mailbox: &Id, error: jmap_client::Error) -> SyncError {
    match &error {
        jmap_client::Error::Set(set_error)
            if set_error.error_type == jmap_proto::error::set::NOT_FOUND =>
        {
            SyncError::NoSuchFolder(mailbox.clone())
        }
        _ => SyncError::Client(error),
    }
}

/// The maximum number of messages in a delta before a full listing of the
/// mailbox costs less.
///
/// Both options cost `Email/get` round trips, and these take almost all of the
/// refresh time. The delta fetches each message that it names. `held` does not
/// limit that number, because `Email/changes` reports on the full account.
/// The listing fetches the mailbox. The row count of the caller is the only
/// free estimate of the mailbox size. Thus a delta that names more messages
/// than the mailbox has rows costs more than a listing.
///
/// The minimum is one `Email/get`. A listing always needs more than one round
/// trip: the state, a query, and one `/get` for each page. Thus a delta that
/// fits in one `/get` always costs less. Without this minimum, an empty mailbox
/// would list again after each change anywhere in the account.
fn catch_up_limit(held: usize, objects_in_get: usize) -> usize {
    held.max(objects_in_get)
}

/// The media type of an uploaded message. RFC 2046 §5.2.1 defines it for an
/// encapsulated RFC 5322 message.
const MESSAGE_MEDIA_TYPE: &str = "message/rfc822";

/// The maximum number of `Email/query` pages. Without it, a server that always
/// returns a limited page would block the thread. No real mailbox gets near
/// this number, so a server that reaches it is broken.
const MAX_QUERY_PAGES: usize = 1024;

/// The value to use when the server does not publish `maxObjectsInGet`. It is
/// small, so all servers accept it. The cost is more round trips for a server
/// that does not obey the RFC.
const FALLBACK_OBJECTS_IN_GET: usize = 50;

/// The maximum number of ids in one `Email/get` from this client, for all
/// server limits.
const MAX_OBJECTS_PER_GET: usize = 500;

#[cfg(test)]
mod tests {
    use super::created_folder_name;

    /// The mock server always returns the requested name unchanged. Thus a live
    /// test cannot show the blank or normalised cases, so this test calls the
    /// function directly.
    #[test]
    fn a_blank_server_name_falls_back_to_the_requested_one() {
        assert_eq!(created_folder_name("", "Projects"), "Projects");
        assert_eq!(created_folder_name("   ", "Projects"), "Projects");
    }

    #[test]
    fn a_non_blank_server_name_overrides_the_requested_one() {
        assert_eq!(created_folder_name("Projets", "Projects"), "Projets");
    }
}
