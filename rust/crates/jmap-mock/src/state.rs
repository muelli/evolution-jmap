// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! In-memory state for the mock server.
//! Store typed objects per account and allocate ids.
//! Track type counters and keep an append-only change log.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::Sender;

use jmap_proto::push::StateChange;
use jmap_proto::{Id, State};

/// Per-account type counters from [`ServerState::type_state_snapshot`].
pub type TypeStateSnapshot = BTreeMap<Id, BTreeMap<&'static str, u64>>;

/// Mock server state for fixture seeding and assertions.
pub struct ServerState {
    pub session_state: u64,
    pub accounts: BTreeMap<Id, AccountState>,
    /// Method names in call order for request-shape assertions.
    pub method_calls: Vec<String>,
    /// Authenticated API requests dispatched to method handlers.
    pub api_requests: usize,
    /// Requests refused with HTTP 401 before dispatch.
    pub unauthorized_responses: usize,
    /// Capability URNs omitted from the session document.
    pub omitted_capabilities: BTreeSet<String>,
    /// Whether `primaryAccounts` is omitted from the session document.
    pub omit_primary_accounts: bool,
    /// Optional `maxCallsInRequest` at `/api`. `None` means unlimited.
    pub calls_in_request: Option<u64>,
    /// Optional max ids in each `/changes` response. `None` returns all ids.
    pub changes_page_size: Option<u64>,
    /// Whether `/changes` keeps returning non-terminating pages.
    pub changes_never_terminates: bool,
    /// Use Stalwart-style `invalidArguments` for invalid `/changes` `sinceState`.
    pub stalwart_shaped_unrecognized_changes_state: bool,
    /// Optional `Email/get` id limit. `None` uses [`crate::DEFAULT_OBJECTS_IN_GET`].
    pub objects_in_get: Option<u64>,
    /// Whether `Email/get` appends objects the client did not request.
    pub email_get_includes_unrequested: bool,
    /// Optional max ids per `Email/query` page. `None` returns all ids.
    pub query_page_size: Option<u64>,
    /// Whether `Email/query` keeps returning capped non-empty pages forever.
    pub query_never_terminates: bool,
    /// Optional `maxSizeRequest` in bytes; `None` means unlimited.
    pub size_request: Option<u64>,
    /// Optional `maxSizeUpload` in bytes; `None` means unlimited.
    pub size_upload: Option<u64>,
    /// Route `/.well-known/jmap` to `/jmap/session` in auth redirect tests.
    pub session_via_redirect: bool,
    /// Optional redirect origin for `GET /.well-known/jmap`.
    /// `None` keeps discovery on this origin.
    pub well_known_redirect_to: Option<String>,
    /// Optional origin override for `apiUrl`, `downloadUrl`, `uploadUrl`, and `eventSourceUrl`.
    pub advertise_origin: Option<String>,
    /// Optional redirect origin for `GET /download/...`.
    /// `None` serves blobs.
    pub download_via_redirect_to: Option<String>,
    /// Whether `GET /download/...` rejects `Accept: application/json` with `406`.
    pub reject_download_accept_json: bool,
    /// Whether `EmailSubmission/set` create omits echoed `identityId` and `emailId`.
    pub terse_submission_create: bool,
    /// Whether `ContactCard/set` create keeps only `id` in `created`.
    pub terse_contact_create: bool,
    /// Optional largest `ContactCard` create payload in JSON octets.
    /// Reject a larger payload as `tooLarge` (RFC 8620 section 5.3).
    pub max_contact_size: Option<u64>,
    /// Whether new `AddressBook` and `Calendar` objects default to `isSubscribed: false`.
    pub new_collections_default_unsubscribed: bool,
    /// Whether `CalendarEvent/set` create returns only `id` in `created`.
    pub terse_calendar_event_create: bool,
    /// Disable scheduling support on this server when true.
    /// Reject `sendSchedulingMessages` in `CalendarEvent/set` as unknown.
    /// See RFC 8620 section 3.9.
    pub without_scheduling: bool,
    /// Whether collection create omits echoed `name` and keeps the server value.
    pub terse_collection_create: bool,
    /// Whether collection create omits `id` to model an RFC-violating server.
    pub id_omitting_collection_create: bool,
    /// Whether collection creates fail with `notCreated/forbidden`.
    pub reject_collection_create: bool,
    /// Optional `maxDelayedSend` value in the submission capability.
    pub max_delayed_send: Option<u64>,
    /// Advertise the Cyrus snooze extension when true.
    /// In this mode, the server accepts `Email.snoozed`.
    pub snooze_extension: bool,
    /// Connected `/eventsource` subscribers for state-change delivery.
    pub event_source: EventSourceHub,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            session_state: 1,
            accounts: BTreeMap::new(),
            method_calls: Vec::new(),
            api_requests: 0,
            unauthorized_responses: 0,
            omitted_capabilities: BTreeSet::new(),
            omit_primary_accounts: false,
            calls_in_request: Some(crate::DEFAULT_CALLS_IN_REQUEST),
            changes_page_size: None,
            changes_never_terminates: false,
            stalwart_shaped_unrecognized_changes_state: false,
            objects_in_get: None,
            email_get_includes_unrequested: false,
            query_page_size: None,
            query_never_terminates: false,
            size_request: Some(crate::DEFAULT_SIZE_REQUEST),
            size_upload: Some(crate::DEFAULT_SIZE_UPLOAD),
            session_via_redirect: false,
            well_known_redirect_to: None,
            advertise_origin: None,
            download_via_redirect_to: None,
            reject_download_accept_json: false,
            terse_submission_create: false,
            terse_contact_create: false,
            max_contact_size: None,
            new_collections_default_unsubscribed: false,
            terse_calendar_event_create: false,
            without_scheduling: false,
            terse_collection_create: false,
            id_omitting_collection_create: false,
            reject_collection_create: false,
            max_delayed_send: None,
            snooze_extension: false,
            event_source: EventSourceHub::new(),
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

    /// Per-account state counters for pushed types and `/changes`.
    pub fn type_state_snapshot(&self) -> TypeStateSnapshot {
        self.accounts
            .iter()
            .map(|(id, account)| {
                let types = BTreeMap::from([
                    ("Mailbox", account.mailboxes.state_counter()),
                    ("Email", account.emails.state_counter()),
                    ("Thread", account.threads.state_counter()),
                    ("ContactCard", account.contact_cards.state_counter()),
                    ("AddressBook", account.address_books.state_counter()),
                    ("Calendar", account.calendars.state_counter()),
                    ("CalendarEvent", account.calendar_events.state_counter()),
                    (
                        "ShareNotification",
                        account.share_notifications.state_counter(),
                    ),
                    (
                        "CalendarEventNotification",
                        account.calendar_event_notifications.state_counter(),
                    ),
                    (
                        "VacationResponse",
                        account.vacation_response.state_counter(),
                    ),
                ]);
                (id.clone(), types)
            })
            .collect()
    }

    /// Effective `maxObjectsInGet` for session output and request checks.
    pub fn objects_in_get(&self) -> u64 {
        self.objects_in_get.unwrap_or(crate::DEFAULT_OBJECTS_IN_GET)
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

/// One `/eventsource` subscriber with an optional `types` filter.
struct Subscriber {
    sender: Sender<Vec<u8>>,
    types: Option<BTreeSet<String>>,
}

/// Active `/eventsource` subscribers (RFC 8620 §7.3).
#[derive(Default)]
pub struct EventSourceHub {
    subscribers: Vec<Subscriber>,
}

impl EventSourceHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one subscriber and return the SSE byte receiver.
    /// `None` means the subscriber listens to all types.
    pub fn subscribe(
        &mut self,
        types: Option<BTreeSet<String>>,
    ) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.subscribers.push(Subscriber { sender, types });
        receiver
    }

    /// Send `change` after optional type filtering.
    /// Drop disconnected subscribers and keep connected non-matches.
    pub fn broadcast(&mut self, change: &StateChange) {
        self.subscribers.retain(
            |subscriber| match filter_for(change, subscriber.types.as_ref()) {
                Some(filtered) => subscriber
                    .sender
                    .send(crate::eventsource::format_state_event(&filtered))
                    .is_ok(),
                None => true,
            },
        );
    }

    /// Return the number of connected `/eventsource` clients.
    /// [`crate::MockServer::wait_for_event_source_subscriber`] reads this value.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }
}

/// Return `change` narrowed to `types`. `None` keeps all types.
/// Return `None` when no account has a matching type.
fn filter_for(change: &StateChange, types: Option<&BTreeSet<String>>) -> Option<StateChange> {
    let Some(types) = types else {
        return Some(change.clone());
    };
    let changed: BTreeMap<Id, BTreeMap<String, State>> = change
        .changed
        .iter()
        .filter_map(|(id, type_state)| {
            let narrowed: BTreeMap<String, State> = type_state
                .iter()
                .filter(|(type_name, _)| types.contains(*type_name))
                .map(|(type_name, state)| (type_name.clone(), state.clone()))
                .collect();
            (!narrowed.is_empty()).then(|| (id.clone(), narrowed))
        })
        .collect();
    (!changed.is_empty()).then(|| StateChange::new(changed))
}

/// Data for one account.
pub struct AccountState {
    pub name: String,
    pub mailboxes: Store<jmap_proto::mail::Mailbox>,
    pub emails: Store<jmap_proto::mail::Email>,
    /// `Thread` objects (RFC 8621 §3), updated with `emails`.
    pub threads: Store<jmap_proto::mail::Thread>,
    pub identities: Store<jmap_proto::mail::Identity>,
    pub submissions: Store<jmap_proto::mail::EmailSubmission>,
    /// `VacationResponse` singleton (RFC 8621 §8.1), seeded as `"singleton"`.
    pub vacation_response: Store<jmap_proto::mail::VacationResponse>,
    /// `Quota` objects (RFC 9425 §2), seeded once and read only for clients.
    pub quotas: Store<jmap_proto::quota::Quota>,
    /// `SieveScript` objects (RFC 9661 §2.1), seeded empty.
    pub sieve_scripts: Store<jmap_proto::sieve::SieveScript>,
    /// Accepted `EmailSubmission` records for test assertions.
    pub outbox: Vec<RecordedSubmission>,
    /// iTIP scheduling messages from `CalendarEvent/set`, stored for assertions.
    pub scheduling_outbox: Vec<RecordedSchedulingMessage>,
    /// `ParticipantIdentity` objects (draft-ietf-jmap-calendars-28 §3), seeded in tests.
    pub participant_identities: Store<jmap_proto::calendars::ParticipantIdentity>,
    pub address_books: Store<jmap_proto::contacts::AddressBook>,
    pub contact_cards: Store<jmap_proto::contacts::ContactCard>,
    pub calendars: Store<jmap_proto::calendars::Calendar>,
    pub calendar_events: Store<jmap_proto::calendars::CalendarEvent>,
    pub principals: Store<jmap_proto::principals::Principal>,
    /// `ShareNotification` records (RFC 9670 §4), keyed by recipient id.
    /// Keep this per account because this mock maps one bearer token to one account.
    pub share_notifications: Store<(Id, jmap_proto::principals::ShareNotification)>,
    /// `CalendarEventNotification` records keyed by recipient id.
    /// This store uses the same one-account mapping model.
    pub calendar_event_notifications: Store<(Id, jmap_proto::calendars::CalendarEventNotification)>,
    /// Principal id reported as `currentUserPrincipalId` (RFC 9670 §2.5).
    /// `None` omits it.
    pub current_user_principal_id: Option<Id>,
    /// Session `isPersonal` value for this account (RFC 8620 §1.6.2).
    /// `false` marks an account shared with the login (RFC 9670).
    pub is_personal: bool,
    pub blobs: BTreeMap<Id, Blob>,
    next_blob_id: u64,
}

impl AccountState {
    pub fn new(name: impl Into<String>) -> Self {
        let mut vacation_response = Store::new("VR");
        vacation_response.seed_with_id(
            Id::from("singleton"),
            jmap_proto::mail::VacationResponse::new(false).with_id("singleton"),
        );
        let mut quotas = Store::new("Q");
        quotas.seed_with_id(
            Id::from("Q1"),
            jmap_proto::quota::Quota::new(
                "Q1",
                "Mail",
                jmap_proto::quota::quota_resource_type::OCTETS,
                0,
                1_073_741_824,
                jmap_proto::quota::quota_scope::ACCOUNT,
                [jmap_proto::quota::quota_data_type::MAIL],
            ),
        );
        Self {
            name: name.into(),
            mailboxes: Store::new("M"),
            emails: Store::new("E"),
            threads: Store::new("T"),
            identities: Store::new("I"),
            submissions: Store::new("S"),
            vacation_response,
            quotas,
            sieve_scripts: Store::new("SV"),
            outbox: Vec::new(),
            scheduling_outbox: Vec::new(),
            participant_identities: Store::new("PI"),
            address_books: Store::new("AB"),
            contact_cards: Store::new("C"),
            calendars: Store::new("CAL"),
            calendar_events: Store::new("CE"),
            principals: Store::new("P"),
            share_notifications: Store::new("SN"),
            calendar_event_notifications: Store::new("CEN"),
            current_user_principal_id: None,
            is_personal: true,
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

/// A submission the mock accepted for delivery.
pub struct RecordedSubmission {
    pub id: Id,
    pub email_id: Id,
    pub identity_id: Id,
    pub envelope: jmap_proto::mail::Envelope,
}

/// Scheduling message queued in [`AccountState::scheduling_outbox`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedSchedulingMessage {
    /// iTIP method from `jmap_proto::calendars::scheduling_method`.
    pub method: String,
    pub event_id: Id,
    pub uid: Option<String>,
    /// Sender `calendarAddress`.
    pub sender: Option<String>,
    /// Recipient `calendarAddress`.
    pub recipient: String,
    /// Instance identifier when this concerns one recurrence only.
    pub recurrence_id: Option<String>,
    /// `VCALENDAR` body with `METHOD`, built by `scheduling::build_ical`.
    pub ical: String,
}

/// One object change kind for the changes log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Updated,
    Destroyed,
}

/// One change-log entry: object `id` changed at state `state`.
#[derive(Debug, Clone)]
pub struct Change {
    pub state: u64,
    pub kind: ChangeKind,
    pub id: Id,
}

/// Typed object store with monotonic state and prefixed ids.
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

    /// Insert without a state bump, for fixture seeding.
    pub fn seed(&mut self, value: T) -> Id {
        let id = self.alloc_id();
        self.objects.insert(id.clone(), value);
        id
    }

    /// Insert under a caller-chosen id without a state bump.
    pub fn seed_with_id(&mut self, id: Id, value: T) {
        self.objects.insert(id, value);
    }

    /// Apply staged mutations as one state transition.
    /// Record one state bump in the change log.
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

    /// All changes after the given state counter value.
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
