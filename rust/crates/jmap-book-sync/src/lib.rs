// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book synchronisation, in the shape `EBookMetaBackend` asks for.
//!
//! One EDS address book source is one JMAP address book, and this crate is
//! the whole of what syncing it means: which cards exist, what a card looks
//! like as a vCard, what changed since a state string, and how an edit made
//! in Evolution turns into a `ContactCard/set`. Each entry point corresponds
//! to one vfunc — [`BookSync::list_existing`] to `list_existing_sync`,
//! [`BookSync::save_contact`] to `save_contact_sync`, and so on.
//!
//! It deliberately knows nothing about GObject or the Evolution headers, so
//! the interesting half of the backend is testable against `jmap-mockd` on
//! any machine. The subclass on top is left with lifecycle and marshalling.

pub mod error;
pub mod patch;

use std::collections::BTreeMap;

use jmap_client::{ChangeSet, Client};
use jmap_proto::contacts::{ContactCard, ContactCardQueryFilter};
use jmap_proto::{Id, State};
use jmap_vcard::{card_to_vcard, vcard_to_card};
use serde_json::Value;

pub use error::SyncError;

/// One contact, as the meta backend wants it: an identifier, a change token
/// and the object itself.
///
/// This is the payload of an `EBookMetaBackendInfo`. `uid` is the JMAP id —
/// see the crate docs of `jmap-vcard` for why it, and not the JSContact
/// `uid`, is what EDS keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    pub uid: String,
    pub revision: String,
    pub vcard: String,
}

/// What changed in the address book since a given state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// The state to pass to the next [`BookSync::get_changes`].
    pub new_state: State,
    /// Cards that were created or modified, already rendered.
    pub changed: Vec<ContactInfo>,
    /// Identifiers that are gone from *this* address book, whether they were
    /// destroyed or merely moved elsewhere.
    pub removed: Vec<String>,
}

/// Synchronises one JMAP address book.
pub struct BookSync {
    client: Client,
    account_id: Id,
    address_book_id: Id,
}

impl BookSync {
    pub fn new(client: Client, account_id: Id, address_book_id: Id) -> Self {
        Self {
            client,
            account_id,
            address_book_id,
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn account_id(&self) -> &Id {
        &self.account_id
    }

    pub fn address_book_id(&self) -> &Id {
        &self.address_book_id
    }

    /// Every card in this address book, with the state that listing is
    /// current as of — `list_existing_sync`.
    pub fn list_existing(&self) -> Result<(State, Vec<ContactInfo>), SyncError> {
        let query = self.client.contact_query(
            &self.account_id,
            ContactCardQueryFilter::in_address_book(self.address_book_id.clone()),
        )?;
        let response = self.client.contact_get(&self.account_id, &query.ids)?;
        let contacts = response
            .list
            .iter()
            .map(ContactInfo::render)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((response.state, contacts))
    }

    /// One card by identifier — `load_contact_sync`.
    ///
    /// Membership of this address book is not checked: EDS asks by the
    /// identity it was given, and a card that has moved out is reported gone
    /// by [`BookSync::get_changes`] rather than by making loads fail.
    pub fn load_contact(&self, uid: &str) -> Result<ContactInfo, SyncError> {
        ContactInfo::render(&self.fetch(uid)?)
    }

    /// Store a vCard — `save_contact_sync`.
    ///
    /// With no `existing_uid` this is a create: the vCard's `UID` is a name
    /// Evolution invented locally (`pas-id-…`), never a JMAP id, so it is
    /// dropped and the server assigns the real one. Otherwise it is an
    /// edit, sent as a PatchObject that names only the properties the vCard
    /// mapping covers — see [`patch`].
    pub fn save_contact(
        &self,
        vcard: &str,
        existing_uid: Option<&str>,
    ) -> Result<ContactInfo, SyncError> {
        let mut card = vcard_to_card(vcard)?;
        let Some(uid) = existing_uid else {
            card.id = None;
            card.address_book_ids = Some(BTreeMap::from([(self.address_book_id.clone(), true)]));
            let account_id = self.account_id().to_string();
            let address_book_id = self.address_book_id().to_string();
            tracing::debug!(account_id, address_book_id, "creating contact card");
            let stored = match self.client.contact_create(&self.account_id, &card) {
                Ok(stored) => stored,
                Err(error) => {
                    tracing::warn!(
                        account_id,
                        address_book_id,
                        %error,
                        "contact card create failed"
                    );
                    return Err(error.into());
                }
            };
            // RFC 8620 §5.3 only requires the server to report properties
            // it set itself, so `stored` may carry nothing but `id` (a real
            // deployment does exactly this — see `tests/terse_create.rs`).
            // Render from a fresh load rather than `stored` directly, so the
            // vCard handed back to EDS always reflects what was actually
            // filed, not merely what a chatty server happened to echo.
            let id = stored
                .id
                .as_ref()
                .ok_or_else(|| SyncError::protocol("ContactCard/set created a card without an id"))?
                .to_string();
            return self.load_contact(&id);
        };

        let current = self.fetch(uid)?;
        let patch = patch::diff(&current, &card);
        if patch.is_empty() {
            return ContactInfo::render(&current);
        }
        let account_id = self.account_id().to_string();
        let address_book_id = self.address_book_id().to_string();
        tracing::debug!(account_id, address_book_id, uid, "updating contact card");
        if let Err(error) =
            self.client
                .contact_update(&self.account_id, &Id::from(uid), Value::Object(patch))
        {
            tracing::warn!(
                account_id,
                address_book_id,
                uid,
                %error,
                "contact card update failed"
            );
            return Err(error.into());
        }
        self.load_contact(uid)
    }

    /// Destroy a card — `remove_contact_sync`.
    pub fn remove_contact(&self, uid: &str) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let address_book_id = self.address_book_id().to_string();
        tracing::debug!(account_id, address_book_id, uid, "removing contact card");
        if let Err(error) = self
            .client
            .contact_destroy(&self.account_id, &Id::from(uid))
        {
            tracing::warn!(
                account_id,
                address_book_id,
                uid,
                %error,
                "contact card destroy failed"
            );
            return Err(error.into());
        }
        Ok(())
    }

    /// What changed since `since` — `get_changes_sync`.
    ///
    /// Fails with a [`SyncError::is_cannot_calculate_changes`] error if the
    /// state is too old for the server, which the caller answers by listing
    /// the book in full.
    pub fn get_changes(&self, since: &State) -> Result<Changes, SyncError> {
        self.classify(
            self.client
                .all_changes(&self.account_id, "ContactCard", since)?,
        )
    }

    /// Turn a raw `/changes` delta into the two lists the meta backend takes.
    ///
    /// `ContactCard/changes` is account-wide, so most of the work is deciding
    /// what a card that is *not* in this book means. The created/updated
    /// distinction is what makes that decidable without consulting the local
    /// cache: a card that shows up as **updated** and is not ours may have
    /// just been moved out, and has to be reported gone or Evolution keeps
    /// showing a contact the book no longer contains; a card that shows up as
    /// **created** and is not ours was never in this book, so it is simply
    /// not our business.
    ///
    /// The delta arrives normalised — [`jmap_client::Client::all_changes`] has
    /// already decided what an id named by several pages is — so no card is
    /// both a candidate and a removal.
    fn classify(&self, delta: ChangeSet) -> Result<Changes, SyncError> {
        let mut removed: Vec<String> = delta.destroyed.iter().map(Id::to_string).collect();
        let candidates: Vec<Id> = delta.created.union(&delta.updated).cloned().collect();
        let mut changed = Vec::new();

        if !candidates.is_empty() {
            let response = self.client.contact_get(&self.account_id, &candidates)?;
            for card in &response.list {
                let Some(id) = &card.id else {
                    return Err(SyncError::protocol(
                        "ContactCard/get returned a card without an id",
                    ));
                };
                if self.holds(card) {
                    changed.push(ContactInfo::render(card)?);
                } else if delta.updated.contains(id) {
                    removed.push(id.to_string());
                }
            }
            // Gone between the /changes call and the /get: only interesting
            // for a card that already existed.
            removed.extend(
                response
                    .not_found
                    .iter()
                    .filter(|id| delta.updated.contains(*id))
                    .map(Id::to_string),
            );
        }

        Ok(Changes {
            new_state: delta.new_state,
            changed,
            removed,
        })
    }

    /// Whether `card` is filed in the address book this instance syncs.
    fn holds(&self, card: &ContactCard) -> bool {
        card.address_book_ids
            .as_ref()
            .is_some_and(|books| books.get(&self.address_book_id).copied().unwrap_or(false))
    }

    fn fetch(&self, uid: &str) -> Result<ContactCard, SyncError> {
        let id = Id::from(uid);
        let response = self
            .client
            .contact_get(&self.account_id, std::slice::from_ref(&id))?;
        response
            .list
            .into_iter()
            .next()
            .ok_or_else(|| SyncError::NotFound(uid.to_owned()))
    }
}

impl ContactInfo {
    /// Render a card, deriving its revision from the result.
    fn render(card: &ContactCard) -> Result<Self, SyncError> {
        let uid = card
            .id
            .as_ref()
            .ok_or_else(|| SyncError::protocol("ContactCard/get returned a card without an id"))?
            .to_string();
        let vcard = card_to_vcard(card);
        Ok(Self {
            revision: revision_of(&vcard),
            uid,
            vcard,
        })
    }
}

/// The change token for a rendered card.
///
/// JSContact's `updated` timestamp is the obvious candidate and the wrong
/// one: RFC 9553 leaves it optional, so a server that omits it would make
/// every card look unchanged forever. A digest of the vCard is always
/// available, and it is a *better* token than a timestamp — it changes
/// exactly when something EDS can see changes, so a server-side edit to a
/// property this mapping drops does not churn every client's cache.
///
/// FNV-1a rather than `DefaultHasher`: revisions are persisted in the EDS
/// cache and compared across restarts, and `DefaultHasher`'s output is
/// explicitly not stable between Rust releases.
fn revision_of(vcard: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in vcard.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
