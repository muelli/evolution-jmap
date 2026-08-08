// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! In-memory server state: per-account typed object stores with id
//! allocation, per-type state counters, and an append-only changes log.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::{Id, State};

/// Everything the mock server knows. Tests hold this behind
/// `Arc<Mutex<..>>` (via `MockServer::state()`) to seed data and make
/// white-box assertions.
pub struct ServerState {
    pub session_state: u64,
    pub accounts: BTreeMap<Id, AccountState>,
    /// Capability URNs to leave out of the session document, as
    /// [`crate::MockServerBuilder::without_capability`] asked. A real account
    /// need not offer all four, and a client that resolves an account under
    /// the wrong capability has to be able to notice.
    pub omitted_capabilities: BTreeSet<String>,
    /// How many ids one `/changes` response may carry, as
    /// [`crate::MockServerBuilder::changes_page_size`] asked. `None` answers
    /// every change at once.
    pub changes_page_size: Option<u64>,
    /// How many ids one `Email/get` may name, as
    /// [`crate::MockServerBuilder::objects_in_get`] asked — advertised in the
    /// session document and enforced. `None` advertises
    /// [`crate::DEFAULT_OBJECTS_IN_GET`] and enforces it just the same.
    pub objects_in_get: Option<u64>,
    /// How many ids one `Email/query` response may carry, as
    /// [`crate::MockServerBuilder::query_page_size`] asked. `None` answers the
    /// whole result at once.
    pub query_page_size: Option<u64>,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            session_state: 1,
            accounts: BTreeMap::new(),
            omitted_capabilities: BTreeSet::new(),
            changes_page_size: None,
            objects_in_get: None,
            query_page_size: None,
        }
    }

    pub fn add_account(&mut self, id: impl Into<Id>, name: impl Into<String>) -> &mut AccountState {
        let id = id.into();
        self.accounts.insert(id.clone(), AccountState::new(name));
        self.accounts.get_mut(&id).expect("just inserted")
    }

    pub fn account(&self, id: &Id) -> Option<&AccountState> {
        self.accounts.get(id)
    }

    pub fn account_mut(&mut self, id: &Id) -> Option<&mut AccountState> {
        self.accounts.get_mut(id)
    }

    pub fn session_state(&self) -> State {
        State::new(self.session_state.to_string())
    }

    /// The `maxObjectsInGet` this server advertises, which is also the one it
    /// enforces. The two being one number is the point: a mock that advertised
    /// a limit it did not apply would let a client that ignores the session
    /// document pass.
    pub fn objects_in_get(&self) -> u64 {
        self.objects_in_get.unwrap_or(crate::DEFAULT_OBJECTS_IN_GET)
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

/// One account's data.
pub struct AccountState {
    pub name: String,
    pub mailboxes: Store<jmap_proto::mail::Mailbox>,
    pub emails: Store<jmap_proto::mail::Email>,
    pub identities: Store<jmap_proto::mail::Identity>,
    pub submissions: Store<jmap_proto::mail::EmailSubmission>,
    /// Every accepted `EmailSubmission` — what a real server would hand to
    /// its SMTP queue. Tests assert against this.
    pub outbox: Vec<RecordedSubmission>,
    pub address_books: Store<jmap_proto::contacts::AddressBook>,
    pub contact_cards: Store<jmap_proto::contacts::ContactCard>,
    pub calendars: Store<jmap_proto::calendars::Calendar>,
    pub calendar_events: Store<jmap_proto::calendars::CalendarEvent>,
    pub blobs: BTreeMap<Id, Blob>,
    next_blob_id: u64,
}

impl AccountState {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mailboxes: Store::new("M"),
            emails: Store::new("E"),
            identities: Store::new("I"),
            submissions: Store::new("S"),
            outbox: Vec::new(),
            address_books: Store::new("AB"),
            contact_cards: Store::new("C"),
            calendars: Store::new("CAL"),
            calendar_events: Store::new("CE"),
            blobs: BTreeMap::new(),
            next_blob_id: 1,
        }
    }

    pub fn alloc_blob_id(&mut self) -> Id {
        let id = Id::new(format!("B{}", self.next_blob_id));
        self.next_blob_id += 1;
        id
    }

    pub fn add_blob(&mut self, content_type: impl Into<String>, data: Vec<u8>) -> Id {
        let id = self.alloc_blob_id();
        self.blobs.insert(
            id.clone(),
            Blob {
                content_type: content_type.into(),
                data,
            },
        );
        id
    }
}

/// An uploaded or seeded binary blob.
pub struct Blob {
    pub content_type: String,
    pub data: Vec<u8>,
}

/// A submission the mock accepted for "delivery".
pub struct RecordedSubmission {
    pub id: Id,
    pub email_id: Id,
    pub identity_id: Id,
    pub envelope: jmap_proto::mail::Envelope,
}

/// What happened to an object, for the changes log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Updated,
    Destroyed,
}

/// One changes-log entry: object `id` changed in the transition *to*
/// `state`.
#[derive(Debug, Clone)]
pub struct Change {
    pub state: u64,
    pub kind: ChangeKind,
    pub id: Id,
}

/// A typed object store with a monotonically increasing state counter and a
/// per-type prefixed id allocator (`M1`, `M2`, … for mailboxes and so on —
/// purely for debuggability; ids stay opaque on the wire).
pub struct Store<T> {
    prefix: &'static str,
    next_id: u64,
    state: u64,
    objects: BTreeMap<Id, T>,
    changes: Vec<Change>,
}

impl<T> Store<T> {
    pub fn new(prefix: &'static str) -> Self {
        Self {
            prefix,
            next_id: 1,
            state: 1,
            objects: BTreeMap::new(),
            changes: Vec::new(),
        }
    }

    pub fn alloc_id(&mut self) -> Id {
        let id = Id::new(format!("{}{}", self.prefix, self.next_id));
        self.next_id += 1;
        id
    }

    pub fn state(&self) -> State {
        State::new(self.state.to_string())
    }

    pub fn state_counter(&self) -> u64 {
        self.state
    }

    pub fn get(&self, id: &Id) -> Option<&T> {
        self.objects.get(id)
    }

    pub fn get_mut(&mut self, id: &Id) -> Option<&mut T> {
        self.objects.get_mut(id)
    }

    pub fn contains(&self, id: &Id) -> bool {
        self.objects.contains_key(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Id, &T)> {
        self.objects.iter()
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Insert without touching state — for seeding fixtures.
    pub fn seed(&mut self, value: T) -> Id {
        let id = self.alloc_id();
        self.objects.insert(id.clone(), value);
        id
    }

    /// Insert under a caller-chosen id without touching state.
    pub fn seed_with_id(&mut self, id: Id, value: T) {
        self.objects.insert(id, value);
    }

    /// Apply a batch of mutations as one state transition (one `/set` call
    /// bumps the state exactly once, however many objects it touched).
    ///
    /// The callback stages mutations through [`Transaction`]; if it stages at
    /// least one change the state counter advances and the changes are
    /// logged.
    pub fn transaction<R>(&mut self, f: impl FnOnce(&mut Transaction<'_, T>) -> R) -> R {
        let mut transaction = Transaction {
            store: self,
            staged: Vec::new(),
        };
        let result = f(&mut transaction);
        let staged = std::mem::take(&mut transaction.staged);
        if !staged.is_empty() {
            self.state += 1;
            let state = self.state;
            self.changes.extend(
                staged
                    .into_iter()
                    .map(|(kind, id)| Change { state, kind, id }),
            );
        }
        result
    }

    /// All changes since the given state counter value.
    pub fn changes_since(&self, since: u64) -> impl Iterator<Item = &Change> {
        self.changes
            .iter()
            .filter(move |change| change.state > since)
    }
}

/// Mutation staging handle used inside [`Store::transaction`].
pub struct Transaction<'a, T> {
    store: &'a mut Store<T>,
    staged: Vec<(ChangeKind, Id)>,
}

impl<T> Transaction<'_, T> {
    pub fn alloc_id(&mut self) -> Id {
        self.store.alloc_id()
    }

    pub fn get(&self, id: &Id) -> Option<&T> {
        self.store.objects.get(id)
    }

    pub fn contains(&self, id: &Id) -> bool {
        self.store.objects.contains_key(id)
    }

    pub fn create(&mut self, id: Id, value: T) {
        self.store.objects.insert(id.clone(), value);
        self.staged.push((ChangeKind::Created, id));
    }

    pub fn update(&mut self, id: &Id, value: T) -> bool {
        if !self.store.objects.contains_key(id) {
            return false;
        }
        self.store.objects.insert(id.clone(), value);
        self.staged.push((ChangeKind::Updated, id.clone()));
        true
    }

    pub fn destroy(&mut self, id: &Id) -> bool {
        if self.store.objects.remove(id).is_none() {
            return false;
        }
        self.staged.push((ChangeKind::Destroyed, id.clone()));
        true
    }
}
