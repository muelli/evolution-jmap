// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail synchronisation, in the shape Camel asks for.
//!
//! One JMAP account is one `CamelJmapStore`, and this crate is what syncing it
//! means: which folders exist, what is in them, and what a message looks like.
//! Each entry point corresponds to one Camel vfunc — [`MailSync::folder_tree`]
//! to `get_folder_info_sync`, [`MailSync::messages`] to what a folder's
//! `CamelFolderSummary` is filled from, [`MailSync::message_source`] to what
//! `get_message_sync` parses — and more as the store grows.
//!
//! Like `jmap-book-sync` and `jmap-cal-sync`, it knows nothing about GObject
//! or the Camel headers, so the interesting half of the provider is testable
//! against `jmap-mockd` on any machine. The two mappings that have no
//! counterpart on the addressbook and calendar side, and are therefore where
//! the work is, are the path encoding — a mailbox name is a display string, a
//! Camel path is an identifier — and the tree itself, which JMAP models with parent
//! pointers and Camel with a linked forest.

pub(crate) mod date;
pub mod error;
pub mod folder;
pub mod keywords;
pub mod mailboxes;
pub mod message;
pub(crate) mod path;
pub(crate) mod pointer;

use std::collections::{BTreeMap, BTreeSet};

use jmap_client::Client;
use jmap_proto::mail::{Email, EmailQueryFilter, Mailbox};
use jmap_proto::methods::Comparator;
use jmap_proto::{Id, State};

pub use error::SyncError;
pub use folder::{FolderInfo, FolderRole, FolderTree};
pub use keywords::{KeywordChange, Keywords};
pub use mailboxes::Filing;
pub use message::{MessageFlags, MessageSummary, SOURCE_PROPERTIES, SUMMARY_PROPERTIES};

/// What a folder-list refresh found.
///
/// A delta is not applied folder by folder, so this is not one: a Camel path
/// is built from a mailbox's ancestors, and `Mailbox/changes` reporting a
/// renamed parent says nothing about the descendants whose paths just moved
/// with it. The account's mailbox list is one `Mailbox/get`, so the honest
/// answer to any change at all is the tree again — and the delta's real worth
/// is the case where it reports nothing, which is nearly every time it is
/// asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderUpdate {
    /// Nothing changed. The state carried is the one to ask from next time,
    /// which may be later than the one asked with.
    Unchanged(State),
    /// The tree as it is now, and the state that listing is current as of.
    Rebuilt { state: State, tree: FolderTree },
}

/// What a mailbox refresh found, when it was able to ask what *changed* rather
/// than what is there.
///
/// The three answers are three different questions the server was able to
/// settle, not three degrees of confidence:
///
/// - [`MessageUpdate::Unchanged`] is one round trip saying the folder is
///   already right, which is what nearly every poll gets.
/// - [`MessageUpdate::Changed`] is a delta, and it is deliberately not phrased
///   as created/updated/destroyed. `Email/changes` reports on the *account's*
///   messages and a folder is asking about one mailbox; JMAP files a message by
///   changing its `mailboxIds`, which is an ordinary update to it. So a delta
///   naming a message says only that something about it changed, never whether
///   that something moved it in or out of the mailbox being refreshed — and the
///   only honest answer is to look each named message up and report which
///   mailbox it is in now. `present` is the rows this mailbox holds for the
///   messages that moved, whole rather than as bare uids; `absent` is the uids
///   that are not in it any more, whether they were destroyed, filed elsewhere,
///   or never in it at all.
/// - [`MessageUpdate::Relisted`] is the whole mailbox, for a state the server
///   cannot calculate from — or for a delta so large that listing is the
///   cheaper way to answer the same question, per [`catch_up_limit`].
///
/// The caller diffs `present` and `absent` against the rows it already has,
/// because it is the only side that knows them: a message that moved into this
/// mailbox and one whose flags changed while it sat here are the same delta on
/// the wire, and which of the two it is depends on whether there is a row for
/// it.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageUpdate {
    /// Nothing in the account's mail changed. The state carried is the one to
    /// ask from next time, which may be later than the one asked with.
    Unchanged(State),
    /// What the mailbox holds for the messages that moved, and what it does not
    /// hold any more — with the state that answer is current as of.
    Changed {
        state: State,
        /// Rows of this mailbox, oldest first, like a listing's.
        present: Vec<MessageSummary>,
        /// Uids that are not in this mailbox, in the order a set puts them.
        absent: Vec<Id>,
    },
    /// The mailbox listed again, and the state that listing is current as of.
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

    /// Every folder of the account, and the state the listing is current as
    /// of — `get_folder_info_sync`.
    ///
    /// The whole tree in one `Mailbox/get`, not a subtree per call: JMAP has no
    /// way to ask for one level, mailbox lists are small, and Camel's
    /// `CAMEL_STORE_FOLDER_INFO_RECURSIVE` asks for all of it anyway.
    ///
    /// The state comes back with it because a folder list without one can only
    /// ever be re-fetched in full; it is what [`MailSync::folder_tree_since`]
    /// takes.
    pub fn folder_tree(&self) -> Result<(State, FolderTree), SyncError> {
        let response = self.client.mailbox_get(&self.account_id)?;
        Ok((response.state, FolderTree::from_mailboxes(&response.list)?))
    }

    /// The folder tree again, but only if the account's mailboxes moved since
    /// `since` — the refresh half of `get_folder_info_sync`.
    ///
    /// One `Mailbox/changes` for the common case, which is a store asking
    /// whether anything happened and being told no. Anything else costs the
    /// full listing, for the reason [`FolderUpdate`] gives.
    ///
    /// A state the server cannot calculate from — too old, or from some other
    /// server entirely — is answered with the listing rather than reported.
    /// The EDS meta backends pass that condition up because EDS knows how to
    /// diff a collection against its cache; Camel has nothing of the kind, so
    /// a store that reported it would be a folder tree that never recovers.
    pub fn folder_tree_since(&self, since: &State) -> Result<FolderUpdate, SyncError> {
        match self.client.all_changes(&self.account_id, "Mailbox", since) {
            Ok(changes) if changes.is_empty() => Ok(FolderUpdate::Unchanged(changes.new_state)),
            Ok(_) => self.rebuild(),
            Err(error) if error.is_cannot_calculate_changes() => self.rebuild(),
            Err(error) => Err(error.into()),
        }
    }

    /// The listing, labelled with its own state rather than the delta's: the
    /// tree is what was walked, and the account may have moved again between
    /// the two calls.
    fn rebuild(&self) -> Result<FolderUpdate, SyncError> {
        let (state, tree) = self.folder_tree()?;
        Ok(FolderUpdate::Rebuilt { state, tree })
    }

    /// Every message in one mailbox, oldest first, and the state that listing
    /// can be brought forward from — what a folder's summary is filled from.
    ///
    /// Two steps, not the one round-trip `Email/query`+`Email/get`
    /// back-reference the client also offers: chaining them sends every
    /// matching id straight into the `/get`, and a mailbox may hold more ids
    /// than one `/get` is allowed to name. Asking first and fetching second is
    /// what makes the fetch divisible.
    ///
    /// Oldest first because that is the order a summary is built in and the
    /// order Camel numbers messages in, and `receivedAt` rather than the `Date`
    /// header because the header is the sender's clock — a message with a wrong
    /// one would sort into the wrong place forever.
    ///
    /// ## The state is read first, and that is not an accident
    ///
    /// It costs an extra round trip — an `Email/get` naming no ids — and the
    /// alternative would be free: the `/get`s below carry a state of their own.
    /// But that state is the one *after* the listing was taken, and a message
    /// that arrived between the query and the fetch is then a message the query
    /// never named and no later delta will ever mention, because it changed
    /// before the state a delta would be asked from. It would be missing from
    /// the folder until something forced a full listing again.
    ///
    /// Reading the state first has the opposite failure, which is not one: the
    /// next delta re-reports changes this listing already has. Every one of them
    /// is a message looked up again and a row written again with what it already
    /// said.
    pub fn messages(&self, mailbox: &Id) -> Result<(State, Vec<MessageSummary>), SyncError> {
        let state = self.client.email_state(&self.account_id)?;
        let ids = self.message_ids(mailbox)?;

        // `/get` may answer in any order (RFC 8620 §5.1), so the query's order
        // is restored below rather than assumed here.
        let mut by_uid: BTreeMap<Id, MessageSummary> = BTreeMap::new();
        for email in self.fetch(&ids, SUMMARY_PROPERTIES)? {
            let summary = MessageSummary::from_email(&email)?;
            by_uid.insert(summary.uid.clone(), summary);
        }

        // An id the query named and the `/get` did not answer for is a message
        // deleted between the two calls: it is gone, which is not a failure and
        // not something to keep a row for. `remove` also settles the other side
        // of the same race — a message that shifted position and came back on
        // two pages is listed once.
        Ok((
            state,
            ids.iter().filter_map(|id| by_uid.remove(id)).collect(),
        ))
    }

    /// What one mailbox looks like now, given what it looked like at `since` —
    /// the refresh half of what fills a folder's summary.
    ///
    /// One `Email/changes` for the common case, which is a folder asking
    /// whether anything happened and being told no. Anything the account did
    /// change costs one `Email/get` per chunk of messages it names, which is
    /// still the whole answer for a mailbox that gained one message where a
    /// listing would have fetched every row it already had.
    ///
    /// What comes back is [`MessageUpdate`], and the reason it is phrased as
    /// present/absent rather than as the delta's own created/updated/destroyed
    /// is documented there: `Email/changes` is an account-wide answer to a
    /// question about one mailbox, and membership has to be re-read rather than
    /// inferred. `destroyed` is the one part taken at its word — a message that
    /// is gone is gone from every mailbox, and there is nothing left to look up.
    ///
    /// A state the server cannot calculate from — too old, or from some other
    /// server entirely — is answered with the mailbox rather than reported, the
    /// judgement [`MailSync::folder_tree_since`] makes about the same condition
    /// and for the same reason: Camel has nowhere to report it to, so a folder
    /// that failed here would be one that never recovers.
    ///
    /// `held` is how many rows the caller already has for this mailbox, and it
    /// is used for one thing: deciding when catching up has stopped being the
    /// cheap answer, per [`catch_up_limit`]. It is asked of the caller rather
    /// than of the server because the caller has it for free and the server
    /// would charge a round trip for it, and it is only ever a *cost* estimate —
    /// a caller that passes a wrong one gets the same rows by a more expensive
    /// route, never different rows.
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
            Err(error) if error.is_cannot_calculate_changes() => return self.relist(mailbox),
            Err(error) => return Err(error.into()),
        };

        // Created and updated are one list here, for the reason `MessageUpdate`
        // gives: which of the two a message is says nothing about whether it is
        // in this mailbox, and that is the only question being asked.
        let touched: Vec<Id> = changes
            .created
            .iter()
            .chain(changes.updated.iter())
            .cloned()
            .collect();

        // How far this is worth following. Every id above is one the fetch
        // below has to look up before it can say whether the mailbox holds it,
        // so a delta from a state a fortnight old is every message the account
        // touched in a fortnight — and listing the one mailbox answers the same
        // question for the price of the rows it actually has.
        //
        // `destroyed` is not counted, because it costs nothing: those ids are
        // taken at the delta's word and never fetched.
        if touched.len() > catch_up_limit(held, self.objects_in_get()) {
            return self.relist(mailbox);
        }

        let mut absent = changes.destroyed;
        let mut present = Vec::new();
        for email in self.fetch(&touched, &filing_properties())? {
            let filed = email
                .mailbox_ids
                .as_ref()
                .is_some_and(|mailboxes| mailboxes.get(mailbox).copied().unwrap_or(false));
            let summary = MessageSummary::from_email(&email)?;
            match filed {
                true => present.push(summary),
                false => {
                    absent.insert(summary.uid);
                }
            }
        }

        // A message the delta named and the `/get` did not answer for was
        // destroyed between the two calls — the same race a listing settles by
        // dropping the id, settled here by reporting it gone, because a folder
        // may well be holding a row for it.
        let answered: BTreeSet<&Id> = present.iter().map(|message| &message.uid).collect();
        let unanswered: Vec<Id> = touched
            .into_iter()
            .filter(|id| !answered.contains(id) && !absent.contains(id))
            .collect();
        absent.extend(unanswered);

        // The order a listing produces, for rows that are appended to the same
        // summary: oldest first by the server's clock, and by uid where a server
        // gave two messages the same time or none at all — so that a refresh is
        // not a different answer each time it is asked.
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

    /// The mailbox listed again, labelled with its own state rather than the
    /// delta's — as [`MailSync::rebuild`] does for the folder tree, and for the
    /// same reason: the listing is what was walked.
    fn relist(&self, mailbox: &Id) -> Result<MessageUpdate, SyncError> {
        let (state, messages) = self.messages(mailbox)?;
        Ok(MessageUpdate::Relisted { state, messages })
    }

    /// The `Email` objects for `ids`, in however many `Email/get` calls the
    /// account's limit takes, in whatever order the server answered.
    ///
    /// No calls at all for an empty list, which is what a delta of nothing but
    /// destroyed messages amounts to.
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

    /// The RFC 5322 bytes of one message — what `get_message_sync` parses into
    /// a `CamelMimeMessage`.
    ///
    /// ## Why the blob id is fetched rather than remembered
    ///
    /// [`MessageSummary`] carries one, and it is thrown away: a
    /// `CamelFolderSummary` row has no field to keep it in, the same problem
    /// the folder's own mailbox id has and without the folder's solution — a
    /// summary row is Camel's struct, not ours, and there are as many of them
    /// as there are messages in the account. So a uid is all this call can be
    /// given, and the blob id is one `Email/get` away.
    ///
    /// That is a round trip per message opened, and it is also the only version
    /// of this that stays correct: RFC 8621 §4.1 makes an `Email` immutable but
    /// says nothing that stops a server reissuing blob ids, and RFC 8620 §6.2
    /// lets it forget one at any time. A cached blob id would turn every such
    /// server into a mailbox that reads fine until it suddenly does not.
    ///
    /// ## Failures
    ///
    /// A uid the account does not hold is [`SyncError::NoSuchMessage`] rather
    /// than a client error — a summary row outliving its message is ordinary,
    /// not a broken account. A message the server returns *without* a `blobId`
    /// is the protocol violation it is; there is no fallback, because
    /// reassembling a message out of its body parts would produce different
    /// bytes than the ones it was signed as.
    pub fn message_source(&self, uid: &Id) -> Result<Vec<u8>, SyncError> {
        let email = self
            .client
            .email_get(
                &self.account_id,
                std::slice::from_ref(uid),
                Some(SOURCE_PROPERTIES),
            )?
            .into_iter()
            // An answer naming some *other* message is not this message's
            // bytes, and a server that sends one has answered a question
            // nobody asked; treated as the message being absent, which it is.
            .find(|email| email.id.as_ref() == Some(uid))
            .ok_or_else(|| SyncError::NoSuchMessage(uid.clone()))?;

        let blob_id = email.blob_id.ok_or_else(|| {
            SyncError::protocol(format!("Email/get returned {uid} without a blobId"))
        })?;

        // The `{name}` of the download template is a filename suggestion for a
        // browser saving the response; nothing reads it back, and the uid is
        // the one name that is certainly safe in a URL — RFC 8620 §1.2 limits
        // an id to URL-safe characters.
        Ok(self
            .client
            .download_blob(&self.account_id, &blob_id, uid.as_str())?)
    }

    /// Puts one message's keyword change on the server — the write half of
    /// what a folder synchronises.
    ///
    /// A row Camel marked read, important, or labelled is this: an `Email/set`
    /// update carrying only the keywords that differ, for the reasons
    /// [`crate::keywords`] gives. A change with nothing in it costs no request
    /// at all, because Camel marks a row as needing a write for reasons that
    /// are not keywords and a provider that asked the server about each of them
    /// would spend a round trip per row on every synchronisation.
    ///
    /// A uid the account no longer holds is [`SyncError::NoSuchMessage`], the
    /// same judgement [`MailSync::message_source`] makes about the same
    /// situation: another client destroying the message is ordinary, and the
    /// flag change is simply moot. Every other refusal stays the server's own
    /// [`SyncError::Client`] — a keyword the server will not accept and a
    /// mailbox gone read-only are things the user has to be told about.
    ///
    /// Not `ifInState`: the state a folder holds is its listing's, and a
    /// conditional write would fail for any change to any *other* message in
    /// the account. Keyword changes commute — this is a patch of named members,
    /// not a replacement — so the concurrency that matters is per keyword, and
    /// sending only what changed is what handles it.
    pub fn set_keywords(&self, uid: &Id, change: &KeywordChange) -> Result<(), SyncError> {
        if change.is_empty() {
            return Ok(());
        }
        self.update_email(uid, change.patch())
    }

    /// Files one message into another mailbox — `transfer_messages_to_sync`.
    ///
    /// A copy and a move are one `Email/set` update over `mailboxIds`, for the
    /// reasons [`crate::mailboxes`] gives; a [`Filing`] that would leave the
    /// message where it is costs no request at all, like a keyword change that
    /// changes nothing.
    ///
    /// One message per call, as with [`MailSync::set_keywords`], although Camel
    /// hands its vfunc a whole list of uids: one `Email/set` may carry many
    /// updates, but it applies them as one state change, and a transfer that
    /// half-succeeded would then be a single failure with no way to say which
    /// messages moved. A request per message is more round trips and an answer
    /// per message, which is what the caller has to report anyway.
    ///
    /// A uid the account no longer holds is [`SyncError::NoSuchMessage`], the
    /// judgement every other write here makes about the same situation. A
    /// destination the account does not have is *not* that: the message is
    /// fine, and a folder Camel still shows after the server deleted it is
    /// something the user has to be told about, so it stays the server's own
    /// [`SyncError::Client`].
    ///
    /// Not `ifInState`, for the reason [`MailSync::set_keywords`] gives: the
    /// state a folder holds is its listing's, and a conditional write would
    /// fail for any change to any other message in the account. A patch of
    /// named members commutes with changes to the members it does not name.
    pub fn file_message(&self, uid: &Id, filing: &Filing) -> Result<(), SyncError> {
        if filing.is_empty() {
            return Ok(());
        }
        self.update_email(uid, filing.patch())
    }

    /// Says whether the user wants to see a folder — the write behind
    /// `CamelSubscribable`'s two vfuncs.
    ///
    /// One method rather than two, because on the wire it is one `Mailbox/set`
    /// update with two possible values: RFC 8621 §2 gives a mailbox an
    /// `isSubscribed`, and `subscribe_folder_sync` and
    /// `unsubscribe_folder_sync` differ only in what they set it to.
    ///
    /// It goes to the *server* rather than into a list Evolution keeps, which
    /// is the whole reason the property is in the protocol: a subscription is a
    /// decision about an account, and a user who hides a folder on their laptop
    /// means it hidden on their phone too.
    ///
    /// Written unconditionally, with no "it is already that" shortcut of the
    /// kind [`MailSync::set_keywords`] has. The two are not the same case: a
    /// keyword change arrives already knowing what changed, whereas the only
    /// thing that could answer "already subscribed?" here is a folder listing,
    /// which is a *cache* — one that another client's change makes wrong, in
    /// precisely the direction that would swallow the user's write. A round
    /// trip per tick in the subscription editor is the cheaper mistake.
    ///
    /// A mailbox the account no longer holds is [`SyncError::NoSuchFolder`],
    /// the same judgement [`MailSync::set_keywords`] makes about a message that
    /// went away: another client deleting the folder is ordinary, and the
    /// user's decision about it is simply moot. Every other refusal stays the
    /// server's own [`SyncError::Client`].
    pub fn set_subscribed(&self, mailbox: &Id, subscribed: bool) -> Result<(), SyncError> {
        self.client
            .mailbox_update(
                &self.account_id,
                mailbox,
                serde_json::json!({ "isSubscribed": subscribed }),
            )
            .map_err(|error| folder_error(mailbox, error))
    }

    /// Makes a folder — `create_folder_sync`.
    ///
    /// The answer is the folder rather than its id, because that is what Camel
    /// asks for: `camel_store_create_folder_sync` hands the `CamelFolderInfo`
    /// it gets straight to Evolution's folder tree, and a caller given an id
    /// would have to list the account again to learn the one thing the id does
    /// not carry — the Camel *path*, which is this crate's invention and is
    /// built here out of the parent's path and the encoded name.
    ///
    /// `parent` is the folder the new one hangs under, and it is a
    /// [`FolderInfo`] rather than an [`Id`] for exactly that reason: the id is
    /// what the request needs and the path is what the answer needs, and only
    /// the caller's tree has both.
    ///
    /// **What the answer is built from is what was *sent*, not what came
    /// back.** RFC 8620 §5.3 lets a server return, for a created record, only
    /// the properties it set itself — so `name` and `parentId` may legitimately
    /// be absent from the object in the response, and reading the path out of
    /// it would give an empty one against a perfectly correct server. The id is
    /// the property a create exists to learn, and it is the one the RFC
    /// guarantees.
    ///
    /// The counts are zero and there are no children, which is not optimism: a
    /// mailbox that did not exist a moment ago has had nowhere for mail to
    /// arrive in, and nothing can hang under it yet. The role is `None` for a
    /// different kind of reason — none is requested, and a role read back from
    /// the response would be this function assigning one outside
    /// [`FolderTree`]'s arbitration, which is what keeps an account from
    /// showing two inboxes.
    ///
    /// `isSubscribed`, in contrast, *is* read from the response when the server
    /// sent it: RFC 8621 §2 leaves the default to the server, and it is the one
    /// property here whose value the client cannot work out. `true` when the
    /// server said nothing, because a folder the user has just asked for is one
    /// they want to see — the other guess hides the folder Evolution was told
    /// to make until the next listing.
    ///
    /// A refusal — a name a sibling already has, a parent that is gone — stays
    /// the server's own [`SyncError::Client`], because the reason is a sentence
    /// for the user and this crate has nothing to add to it.
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
        let created = self.client.mailbox_create(&self.account_id, &requested)?;
        let id = created
            .id
            .ok_or_else(|| SyncError::protocol("Mailbox/set created a mailbox without an id"))?;

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
            children: Vec::new(),
        })
    }

    /// Removes a folder — `delete_folder_sync`.
    ///
    /// By mailbox id rather than by path, like every other write here: the
    /// caller named a folder out of a listing, and the path that folder had is
    /// the part of the listing another client's rename can already have
    /// invalidated.
    ///
    /// No `onDestroyRemoveEmails`. RFC 8621 §2.5 makes that argument the
    /// difference between "remove this folder" and "remove this folder and
    /// everything in it", and the second is not what Camel asked for: a store
    /// that quietly sent it would delete the user's mail on a click that says
    /// nothing about mail. What comes back instead is the server's refusal —
    /// `mailboxHasChild` or `mailboxHasEmail` — and those are kept whole, as
    /// [`SyncError::Client`], for the caller to put in front of the user.
    ///
    /// They deliberately get no variant of their own, unlike the missing folder
    /// below. The test for a variant here is whether Camel has a *code* the
    /// caller could map it onto, and for these two it does not — the reason is
    /// prose either way, and re-encoding the server's own vocabulary into ours
    /// would only lose the description that came with it.
    ///
    /// A mailbox the account no longer holds is [`SyncError::NoSuchFolder`],
    /// the judgement [`MailSync::set_subscribed`] makes about the same
    /// situation: another client having removed the folder first is the outcome
    /// the user asked for, and reporting it as a failure would turn someone
    /// else's tidying into a broken account.
    pub fn delete_folder(&self, mailbox: &Id) -> Result<(), SyncError> {
        self.client
            .mailbox_destroy(&self.account_id, mailbox)
            .map_err(|error| folder_error(mailbox, error))
    }

    /// Renames a folder, and moves it — `rename_folder_sync`.
    ///
    /// One method for both, because Camel asks for both with one vfunc: it
    /// names a folder by path, and a path carries the folder's name *and* where
    /// it hangs, so "Work/Notes" becoming "Archive/Notes" and becoming
    /// "Work/Minutes" arrive here identically. On the wire they are the two
    /// properties of one `Mailbox/set` update, `name` and `parentId`.
    ///
    /// **Both are always sent**, including a `parentId` of `null` for a folder
    /// moving up to the top level. Sending only what looks changed would need a
    /// before-picture, and the only one available is the caller's listing —
    /// a cache, which another client's move has already made wrong in exactly
    /// the direction that would swallow this write. What the user asked for is
    /// where the folder should *be*, and that is a statement about both
    /// properties; a patch that left `parentId` out would answer with a path
    /// the folder is not at.
    ///
    /// The answer is the folder's new Camel path, for the reason
    /// [`MailSync::create_folder`] answers with a whole folder: the caller keys
    /// the folder by that string and cannot build it, because the mapping from
    /// a mailbox name to a path component is this crate's. Everything else
    /// about the folder — its counts, its role, its subscription, what hangs
    /// under it — a rename does not touch, so there is nothing else to report.
    ///
    /// `name` is a mailbox name, not a path component, the reading
    /// [`MailSync::create_folder`] documents: a `/` in it is a character of the
    /// name, and this is where it becomes `%2F`.
    ///
    /// Not `ifInState`, for the reason [`MailSync::set_keywords`] gives.
    ///
    /// A mailbox the account no longer holds is [`SyncError::NoSuchFolder`],
    /// the judgement every other write naming a mailbox makes. Every other
    /// refusal — a name a sibling already has, a parent that is gone, a move
    /// into the folder's own subtree — stays the server's own
    /// [`SyncError::Client`]: each is a sentence for the user, and Camel has no
    /// code to map them onto.
    pub fn rename_folder(
        &self,
        folder: &Id,
        parent: Option<&FolderInfo>,
        name: &str,
    ) -> Result<String, SyncError> {
        self.client
            .mailbox_update(
                &self.account_id,
                folder,
                serde_json::json!({
                    "name": name,
                    "parentId": parent.map(|parent| parent.id.as_str()),
                }),
            )
            .map_err(|error| folder_error(folder, error))?;

        Ok(path::join(
            parent.map(|parent| parent.path.as_str()),
            &path::encode_component(name),
        ))
    }

    /// One `Email/set` update, with the one refusal that is not a failure
    /// named: a message the account no longer holds.
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

    /// The ids of a mailbox's messages, oldest first, however many pages the
    /// server answers in.
    fn message_ids(&self, mailbox: &Id) -> Result<Vec<Id>, SyncError> {
        let mut ids: Vec<Id> = Vec::new();

        for _ in 0..MAX_QUERY_PAGES {
            let response = self.client.email_query(
                &self.account_id,
                EmailQueryFilter::in_mailbox(mailbox.clone()),
                Some(vec![Comparator::ascending("receivedAt")]),
                None,
                ids.len() as i64,
            )?;
            let capped = response.limit.is_some();
            let answered = !response.ids.is_empty();
            ids.extend(response.ids);
            // No cap means the whole rest of the result set is in hand; a cap
            // that came back empty means the rest of it is nothing.
            if !capped || !answered {
                return Ok(ids);
            }
        }
        Err(SyncError::protocol(
            "Email/query never stopped reporting a limited answer",
        ))
    }

    /// How many ids one `Email/get` of this account may name.
    ///
    /// The server's `maxObjectsInGet` if it published one — asking for more is
    /// a `requestTooLarge` that fails the whole call rather than a short
    /// answer — and otherwise a conservative guess, because RFC 8620 §2
    /// requires the limit to be there and a server that omits it has told us
    /// nothing about what it will take.
    ///
    /// Capped from above as well as below: a server may advertise a limit far
    /// larger than a mailbox, and one `/get` for fifty thousand messages is a
    /// response Evolution waits on with the folder half-open. Chunking bounds
    /// what is in flight at the cost of round-trips it would otherwise make
    /// anyway.
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

/// What a delta has to ask for: a summary row's properties and the one thing a
/// listing never needs, `mailboxIds`.
///
/// Kept out of [`SUMMARY_PROPERTIES`] rather than added to it, because a
/// listing already knows the answer — it asked `Email/query` for one mailbox's
/// messages — and neither [`MessageSummary`] nor a `CamelFolderSummary` row has
/// anywhere to keep it. It is a question only a delta has, and only for as long
/// as it takes to decide which of its two lists a message belongs in.
fn filing_properties() -> Vec<&'static str> {
    let mut properties = SUMMARY_PROPERTIES.to_vec();
    properties.push("mailboxIds");
    properties
}

/// The one refusal a write to a mailbox has to be told apart from the rest:
/// the mailbox is not there any more.
///
/// Shared by the two writes that name a mailbox rather than a message, because
/// the judgement is the same in both and it is the kind that goes wrong by
/// drifting apart: a `notFound` read as an ordinary failure in one place and as
/// a vanished folder in another is the same account reported two ways depending
/// on which vfunc the user happened to trigger.
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

/// The most messages a delta may name before listing the mailbox is the cheaper
/// way to find out what it holds.
///
/// Both sides of the comparison are round trips of `Email/get`, which is what a
/// refresh spends nearly all of its time in. Catching up fetches the messages
/// the delta names — `held` is not a bound on that, because `Email/changes`
/// reports on the whole *account* and most of what it names may be in mailboxes
/// this folder is not. Listing fetches the mailbox, whose size the caller's row
/// count is the one free estimate of: it is what the folder last saw there, and
/// it is exactly the set a listing would fetch again. So a delta that names more
/// messages than the mailbox has rows is a delta that costs more than the
/// listing it saves.
///
/// The floor is one `Email/get`, because a listing is never a single round trip
/// — it is the state, then a query, then a `/get` per page — so a delta that
/// fits in one is cheaper than any listing whatsoever. Without it, an empty or
/// nearly empty mailbox would list itself again every time the account was
/// touched anywhere, which is the opposite of what this bound is for.
fn catch_up_limit(held: usize, objects_in_get: usize) -> usize {
    held.max(objects_in_get)
}

/// A server that answers every `Email/query` with a limited page, without ever
/// running out of ids, would otherwise hang the calling thread. Far above any
/// real mailbox at any real page size; reaching it means the server is broken.
const MAX_QUERY_PAGES: usize = 1024;

/// What to assume when the server does not publish `maxObjectsInGet`. Small
/// enough that no plausible server rejects it, at the price of more round-trips
/// for a server that broke the rules.
const FALLBACK_OBJECTS_IN_GET: usize = 50;

/// The most this client asks for in one `Email/get`, however much the server
/// allows.
const MAX_OBJECTS_PER_GET: usize = 500;
