// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! In-memory server state: per-account typed object stores with id
//! allocation, per-type state counters, and an append-only changes log.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::Sender;

use jmap_proto::push::StateChange;
use jmap_proto::{Id, State};

/// One [`ServerState::type_state_snapshot`] — every account's state counter
/// per tracked type name, at one point in time.
pub type TypeStateSnapshot = BTreeMap<Id, BTreeMap<&'static str, u64>>;

/// Everything the mock server knows. Tests hold this behind
/// `Arc<Mutex<..>>` (via `MockServer::state()`) to seed data and make
/// white-box assertions.
pub struct ServerState {
    pub session_state: u64,
    pub accounts: BTreeMap<Id, AccountState>,
    /// The name of every method call this server has answered, in the order it
    /// answered them — `Mailbox/get`, `Email/query`, `Email/changes`.
    ///
    /// What it is for is the assertions no amount of reading the account's
    /// objects can make: whether a client asked at all, and which question it
    /// asked. A folder refresh that lists the whole mailbox and one that asks
    /// for a delta leave the account in exactly the same place — the
    /// difference between them is entirely in what went over the wire, and
    /// cheapness that is not asserted is cheapness that quietly goes away.
    pub method_calls: Vec<String>,
    /// How many requests this server has dispatched at the API endpoint —
    /// that is, requests that got past authentication and were answered from
    /// the account rather than refused. [`Self::unauthorized_responses`] is
    /// the other half.
    ///
    /// [`Self::method_calls`] counts calls; this counts round trips, and the
    /// two only differ where it matters — a client that chains `Email/query`
    /// and `Email/get` through a back-reference makes the same two calls as
    /// one that sends them separately, and pays one round trip instead of two.
    /// Nothing about the account afterwards says which it did.
    pub api_requests: usize,
    /// How many requests this server has refused with a 401, at any endpoint.
    ///
    /// A refused request never reaches the dispatcher, so it leaves no trace
    /// in [`Self::api_requests`] or [`Self::method_calls`] — which makes "how
    /// many times did the client try, before it gave up or found a token that
    /// worked" unanswerable without this. Token refresh-and-retry is
    /// exactly a question of that count: a backend that
    /// retried a 401 it had no fresher credentials for would double every
    /// rejection the user's own wrong password caused, and nothing else the
    /// server records would show it.
    pub unauthorized_responses: usize,
    /// Capability URNs to leave out of the session document, as
    /// [`crate::MockServerBuilder::without_capability`] asked. A real account
    /// need not offer all four, and a client that resolves an account under
    /// the wrong capability has to be able to notice.
    pub omitted_capabilities: BTreeSet<String>,
    /// Leave `primaryAccounts` out of the session document entirely, as
    /// [`crate::MockServerBuilder::without_primary_accounts`] asked. RFC 8620
    /// §2 permits a server to omit it outright ("a server that does not
    /// support this concept MUST omit this property"), which is a different,
    /// legal server shape from naming a capability with no primary account —
    /// every capability still lists its accounts under
    /// [`AccountState`]'s own capabilities, so a client is left to infer
    /// which one is primary the way RFC 8620 allows.
    pub omit_primary_accounts: bool,
    /// How many method calls one request may carry, as
    /// [`crate::MockServerBuilder::calls_in_request`] asked — advertised as
    /// `maxCallsInRequest` in the session document and enforced at the API
    /// endpoint.
    ///
    /// `None` is the server that names no limit at all
    /// ([`crate::MockServerBuilder::no_calls_in_request`]), which RFC 8620 §2
    /// does not allow and which a client still has to have an answer for; it
    /// takes a request of any length. The default is
    /// [`crate::DEFAULT_CALLS_IN_REQUEST`], above anything this client chains,
    /// so only a test about the limit meets it.
    pub calls_in_request: Option<u64>,
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
    /// The largest request this server takes at the API endpoint, in octets, as
    /// [`crate::MockServerBuilder::size_request`] asked — advertised as
    /// `maxSizeRequest` in the session document and enforced on the body before
    /// it is parsed.
    ///
    /// `None` is the server that names no limit at all
    /// ([`crate::MockServerBuilder::no_size_request`]), which RFC 8620 §2 does
    /// not allow and which a client still has to have an answer for; it takes a
    /// request of any size. The default is [`crate::DEFAULT_SIZE_REQUEST`], far
    /// above anything this client builds, so only a test about the limit meets
    /// it.
    pub size_request: Option<u64>,
    /// The largest upload this server takes, as
    /// [`crate::MockServerBuilder::size_upload`] asked — advertised as
    /// `maxSizeUpload` in the session document and enforced on `/upload/`.
    ///
    /// `None` is the server that names no limit at all
    /// ([`crate::MockServerBuilder::no_size_upload`]), which RFC 8620 §2 does
    /// not allow and which a client still has to have an answer for; it takes
    /// any size. The default is [`crate::DEFAULT_SIZE_UPLOAD`], far above what
    /// any test uploads, so only a test about the limit meets it.
    pub size_upload: Option<u64>,
    /// Serve the session document via a 307 redirect from `/.well-known/jmap`
    /// to `/jmap/session`, as [`crate::MockServerBuilder::session_via_redirect`]
    /// asked — a real deployment (Stalwart among them) shaped exactly this
    /// way is what first exposed `UreqTransport` dropping the `Authorization`
    /// header across the hop. `/jmap/session` itself is never gated by the
    /// usual 401: like the server this reproduces, it answers 200 either way
    /// and simply reports zero accounts when the request arrived
    /// unauthenticated, so the failure this exists to catch is "no primary
    /// account", not a transport error.
    pub session_via_redirect: bool,
    /// State the session document's `apiUrl`/`downloadUrl`/`uploadUrl`/
    /// `eventSourceUrl` on this origin instead of the one this server is
    /// actually reachable on, as
    /// [`crate::MockServerBuilder::advertise_origin`] asked — a real
    /// deployment (Stalwart among them) can advertise a configured public
    /// hostname/scheme that differs from the address a client actually
    /// reached it through (a reverse proxy, NAT boundary, or a TLS listener
    /// nothing routes to). `None` advertises the real origin, matching every
    /// other test.
    pub advertise_origin: Option<String>,
    /// Answer `GET /download/...` with a `302` to the same path on this
    /// origin instead of serving the blob, as
    /// [`crate::MockServerBuilder::download_via_redirect_to`] asked. `None`
    /// serves the blob directly, matching every other test.
    pub download_via_redirect_to: Option<String>,
    /// Answer `GET /download/...` with a `406` instead of the blob when the
    /// request's `Accept` header is exactly `application/json`, as
    /// [`crate::MockServerBuilder::reject_download_accept_json`] asked —
    /// reproducing a server doing RFC 7231 §5.3.2 content negotiation on a
    /// download request that (wrongly) claims to accept only JSON for an
    /// answer that never is JSON. `false` serves the blob regardless of
    /// `Accept`, matching every other test.
    pub reject_download_accept_json: bool,
    /// Omit `identityId`/`emailId` from a created `EmailSubmission`, as
    /// [`crate::MockServerBuilder::terse_submission_create`] asked — RFC 8620
    /// §5.3 says the `created` map need only contain properties "that were
    /// not sent by the client", and a real server (Stalwart, confirmed
    /// against the live deployment) takes this literally: since the client
    /// supplies `identityId`/`emailId` itself when creating a submission,
    /// neither is server-set and a spec-following server may leave both out.
    /// `false` echoes them back in full, matching every other test and this
    /// project's own prior assumption before that finding.
    pub terse_submission_create: bool,
    /// Answer a `ContactCard/set` create with only `id` in the `created`
    /// map, as [`crate::MockServerBuilder::terse_contact_create`] asked —
    /// every other property was already sent by the client, so a
    /// spec-following server (RFC 8620 §5.3) may omit all of it, as a live
    /// Stalwart deployment does. `false` echoes the full card back,
    /// matching every other test and this project's own prior assumption
    /// before that finding.
    pub terse_contact_create: bool,
    /// Default a freshly created `AddressBook`/`Calendar` to
    /// `isSubscribed: false` unless the create explicitly asked for `true`,
    /// as [`crate::MockServerBuilder::new_collections_default_unsubscribed`]
    /// asked — a real server (Stalwart, confirmed against the live
    /// deployment) does this even though both specifications leave a fresh
    /// collection's subscription state unstated, and
    /// `jmap_collection_sync::resources`'s own discovery drops
    /// `isSubscribed == Some(false)` on purpose. `false` leaves the property
    /// exactly as the client sent it (typically absent), matching every
    /// other test and this project's own prior assumption before that
    /// finding.
    pub new_collections_default_unsubscribed: bool,
    /// Answer a `CalendarEvent/set` create with only `id` in the `created`
    /// map, as [`crate::MockServerBuilder::terse_calendar_event_create`]
    /// asked — every other property was already sent by the client, so a
    /// spec-following server (RFC 8620 §5.3) may omit all of it, as a live
    /// Stalwart deployment does. `false` echoes the full event back,
    /// matching every other test and this project's own prior assumption
    /// before that finding.
    pub terse_calendar_event_create: bool,
    /// Omit `name` from a created `AddressBook`/`Calendar`'s `created` entry,
    /// as [`crate::MockServerBuilder::terse_collection_create`] asked — the
    /// client supplied `name` itself and the server accepted it unchanged, so
    /// a spec-following server (RFC 8620 §5.3) may leave it out, as a live
    /// Fastmail deployment does. Unlike `terse_contact_create`/
    /// `terse_calendar_event_create`, this strips only `name`: `isDefault`,
    /// `myRights` and (for calendars) `color` are genuinely server-computed
    /// here. `false` echoes `name` back, matching every other test and this
    /// project's own prior assumption before that finding.
    pub terse_collection_create: bool,
    /// `maxDelayedSend` to advertise on the submission account capability, as
    /// [`crate::MockServerBuilder::max_delayed_send`] asked (RFC 8621 §7.1).
    /// `None` advertises an empty submission capability object, matching
    /// every other test and every deployment that does not support SMTP
    /// FUTURERELEASE.
    pub max_delayed_send: Option<u64>,
    /// Every currently connected `/eventsource` client (RFC 8620 §7.3), so
    /// [`crate::MockServer::push_state_change`] has someone to push to.
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
            objects_in_get: None,
            query_page_size: None,
            size_request: Some(crate::DEFAULT_SIZE_REQUEST),
            size_upload: Some(crate::DEFAULT_SIZE_UPLOAD),
            session_via_redirect: false,
            advertise_origin: None,
            download_via_redirect_to: None,
            reject_download_accept_json: false,
            terse_submission_create: false,
            terse_contact_create: false,
            new_collections_default_unsubscribed: false,
            terse_calendar_event_create: false,
            terse_collection_create: false,
            max_delayed_send: None,
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

    /// Every account's state counter for the six types RFC 8620 push (and
    /// this mock's own `/changes` methods) track, as of right now.
    ///
    /// [`crate::dispatch::handle_api`] takes one of these before and one
    /// after running a request's method calls, and diffs them to find out
    /// what actually changed — the automatic half of JMAP Push, which no
    /// individual `*/set` handler has to know about.
    pub fn type_state_snapshot(&self) -> TypeStateSnapshot {
        self.accounts
            .iter()
            .map(|(id, account)| {
                let types = BTreeMap::from([
                    ("Mailbox", account.mailboxes.state_counter()),
                    ("Email", account.emails.state_counter()),
                    ("ContactCard", account.contact_cards.state_counter()),
                    ("AddressBook", account.address_books.state_counter()),
                    ("Calendar", account.calendars.state_counter()),
                    ("CalendarEvent", account.calendar_events.state_counter()),
                    (
                        "ShareNotification",
                        account.share_notifications.state_counter(),
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

/// One `/eventsource` client: the channel its pushed SSE bytes go out on,
/// and the `types` URI Template parameter (RFC 8620 §7.3) it registered
/// with — `None` for "no filter, subscribed to everything" (an absent
/// parameter, or the literal `*`).
struct Subscriber {
    sender: Sender<Vec<u8>>,
    types: Option<BTreeSet<String>>,
}

/// The `/eventsource` clients currently connected (RFC 8620 §7.3).
///
/// [`Self::broadcast`] takes the structured `StateChange` rather than
/// pre-formatted bytes so it can narrow `changed` to each subscriber's own
/// `types` filter before formatting — a subscriber whose filter matches
/// none of a change's types is sent nothing at all, rather than an empty
/// `StateChange` it would have no way to tell apart from a real one naming
/// zero types.
#[derive(Default)]
pub struct EventSourceHub {
    subscribers: Vec<Subscriber>,
}

impl EventSourceHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new subscriber with the `types` filter it connected with
    /// (`None` = every type), returning the receiving end it reads pushed
    /// bytes from until this hub (or its own disconnect) drops the sending
    /// end.
    pub fn subscribe(
        &mut self,
        types: Option<BTreeSet<String>>,
    ) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.subscribers.push(Subscriber { sender, types });
        receiver
    }

    /// Send `change`, narrowed to each subscriber's own `types` filter, to
    /// every currently connected subscriber whose filter matches at least
    /// one of its changed types — dropping any subscriber whose receiving
    /// end has gone away (the client disconnected). A subscriber whose
    /// filter matches nothing in `change` is left connected but sent
    /// nothing, same as a request that mutated nothing at all.
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

    /// How many `/eventsource` clients are connected right now — what
    /// [`crate::MockServer::wait_for_event_source_subscriber`] polls.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }
}

/// Narrow `change`'s `changed` map to the type names in `types` (`None` =
/// every type, so `change` passes through unchanged); an account whose
/// entry has no matching type is dropped entirely, and `None` is returned
/// when nothing survives — the signal to send this subscriber nothing.
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

/// One account's data.
pub struct AccountState {
    pub name: String,
    pub mailboxes: Store<jmap_proto::mail::Mailbox>,
    pub emails: Store<jmap_proto::mail::Email>,
    pub identities: Store<jmap_proto::mail::Identity>,
    pub submissions: Store<jmap_proto::mail::EmailSubmission>,
    /// The `VacationResponse` singleton (RFC 8621 §8.1). Always holds exactly
    /// one object, seeded at account creation under the fixed id
    /// `"singleton"`, since the type has no create or destroy: `Store<T>`'s
    /// state counter and changes log are reused unchanged rather than
    /// building a separate mechanism for one object.
    pub vacation_response: Store<jmap_proto::mail::VacationResponse>,
    /// `Quota` objects (RFC 9425 §2). Read-only from the client's side (the
    /// RFC defines no `Quota/set`), seeded at account creation the same way
    /// `vacation_response` is.
    pub quotas: Store<jmap_proto::quota::Quota>,
    /// `SieveScript` objects (RFC 9265 §2). Unlike `quotas`, empty on a fresh
    /// account: scripts are client-created, so there is nothing to seed.
    pub sieve_scripts: Store<jmap_proto::sieve::SieveScript>,
    /// Every accepted `EmailSubmission` — what a real server would hand to
    /// its SMTP queue. Tests assert against this.
    pub outbox: Vec<RecordedSubmission>,
    pub address_books: Store<jmap_proto::contacts::AddressBook>,
    pub contact_cards: Store<jmap_proto::contacts::ContactCard>,
    pub calendars: Store<jmap_proto::calendars::Calendar>,
    pub calendar_events: Store<jmap_proto::calendars::CalendarEvent>,
    pub principals: Store<jmap_proto::principals::Principal>,
    /// `ShareNotification` records (RFC 9670 §4), one per grant change. Kept
    /// in the same account as the shared object itself rather than in a
    /// separate per-recipient account, since this mock models one principal
    /// per bearer token in a single account rather than a real server's
    /// distinct account per principal — the recipient the notification is
    /// for is carried alongside it (the `Id`) and used to filter
    /// `ShareNotification/get`/`/query` to only the caller it belongs to.
    pub share_notifications: Store<(Id, jmap_proto::principals::ShareNotification)>,
    /// The principal that answers RFC 9670 §2.5's `currentUserPrincipalId` —
    /// "which principal is *me* in this account". `None` until a test seeds
    /// one; the session document then omits the property rather than naming a
    /// principal that does not exist.
    pub current_user_principal_id: Option<Id>,
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
            identities: Store::new("I"),
            submissions: Store::new("S"),
            vacation_response,
            quotas,
            sieve_scripts: Store::new("SV"),
            outbox: Vec::new(),
            address_books: Store::new("AB"),
            contact_cards: Store::new("C"),
            calendars: Store::new("CAL"),
            calendar_events: Store::new("CE"),
            principals: Store::new("P"),
            share_notifications: Store::new("SN"),
            current_user_principal_id: None,
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
    /// The callback stages mutations through `Transaction`; if it stages at
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
