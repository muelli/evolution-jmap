// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Driving a real JMAP server end to end — the recipe this project's own
//! "Integration testing (parallel track)" plan asks for, and the
//! real-server half of the "real-server readiness" priority.
//!
//! Every other test in this crate is against `jmap-mockd`, which answers
//! exactly what the fixture told it to and nothing a real deployment's own
//! quirks would add: capability objects with fields this client has never
//! seen, an account list shaped differently than the mock's single seeded
//! one, limits that are actually enforced rather than left unset. None of
//! that shows up until a real server is on the other end of the wire — which
//! is what this file is for, and why it is not part of the default suite: it
//! needs a server this repository does not run, reachable over the network,
//! with an account already provisioned on it.
//!
//! ## Running it
//!
//! ```console
//! $ export JMAP_LIVE_SERVER_URL=https://jmap.example.com
//! $ export JMAP_LIVE_SERVER_USER=me@example.com
//! $ export JMAP_LIVE_SERVER_PASSWORD=...        # or JMAP_LIVE_SERVER_TOKEN for Bearer
//! $ export JMAP_LIVE_SERVER_REBASE_URLS=1       # only if apiUrl names an unreachable origin
//! $ cargo test -p evolution-jmap-client --features live-server -- --ignored
//! ```
//!
//! `JMAP_LIVE_SERVER_REBASE_URLS` is [`Client::builder`]'s
//! `rebase_urls_to_origin`: set it when the deployment's session document
//! names an `apiUrl`/`downloadUrl`/`uploadUrl`/`eventSourceUrl` this runner
//! cannot route to even though `JMAP_LIVE_SERVER_URL` itself is reachable —
//! a reverse proxy, NAT boundary, or (the case this exists for) a configured
//! public hostname advertised over `https` when only a plain-`http` listener
//! on a different address answers. Leave it unset against a deployment whose
//! session already names a reachable origin.
//!
//! `docs/manual-test-live-server.md` has the full recipe, including how to
//! provision the disposable Stalwart VM this is meant to run against first
//! (the harness repository's `harness/gcp/create-stalwart.sh`).
//!
//! Gated twice over — the `live-server` feature, so a plain `cargo test`
//! never even compiles this file, and `#[ignore]`, so `cargo test --features
//! live-server` still does not run it without `--ignored` — because unlike
//! every other test in this workspace it reaches outside the process, and it
//! must never turn a routine `cargo test` into a network call that fails on a
//! machine with no such server configured.
//!
//! ## What this deliberately does not do
//!
//! Write to any account it did not create for the purpose. Most of the
//! deployment's real mail is not this suite's to touch — even the
//! disposable Stalwart VM is meant to be reused across runs rather than
//! reseeded — so most tests here are read-only: session discovery,
//! `Core/echo`, and listing what already exists. `Mailbox/set` round-trips
//! (create, rename, destroy) are covered against the mock, where they cost
//! nothing, *and* against a dedicated throwaway account here (see
//! [`mailbox_create_rename_then_destroy_round_trips_through_the_real_api`])
//! — the one exception, and scoped to an account this suite seeded for
//! exactly this test.

use std::collections::BTreeMap;
use std::env;
use std::time::{Duration, Instant};

use jmap_client::eventsource::{SharedHeaders, expand_url};
use jmap_client::{CancelFlag, Client, Credentials, Error, EventSourceSubscription};
use jmap_proto::Id;
use jmap_proto::blob::{BlobGetRequest, BlobUploadRequest, UploadBlob};
use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventNotificationQueryFilter, CalendarEventParseRequest,
    CalendarEventQueryFilter, Participant, ParticipantIdentity, RecurrenceRule,
};
use jmap_proto::contacts::{
    AddressBook, ContactCard, ContactCardParseRequest, ContactCardQueryFilter, OnlineService,
};
use jmap_proto::error::method;
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailImport, EmailQueryFilter,
    EmailSubmissionQueryFilter, Mailbox, keyword, role,
};
use jmap_proto::methods::{
    BlobCopyRequest, ChangesRequest, ChangesResponse, Comparator, GetRequest, GetResponse,
    SetRequest,
};
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::quota::{Quota, quota_resource_type, quota_scope};
use jmap_proto::request::{Request, ResultReference};
use jmap_proto::session::{
    CAPABILITY_BLOB, CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
    CAPABILITY_PRINCIPALS, CAPABILITY_QUOTA, CAPABILITY_SUBMISSION,
};
use jmap_proto::sieve::{CAPABILITY_SIEVE, SieveScript};
use jmap_proto::state::UtcDate;
use serde_json::json;

const CAPABILITY_CONTACTS_PARSE: &str = "urn:ietf:params:jmap:contacts:parse";

/// A value unique to this process invocation, for naming a record so a
/// concurrent or prior run's leftover can never be mistaken for this run's
/// own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// The origin and credentials this run was pointed at, or a panic naming the
/// variable that is missing.
///
/// A panic and not a skip: this test is never run by accident (it needs both
/// the feature and `--ignored`), so reaching here with the environment
/// unset is a misconfigured invocation of a deliberately-requested test, not
/// an environment this suite should quietly tolerate.
fn connect() -> Client {
    let origin = env::var("JMAP_LIVE_SERVER_URL").expect(
        "set JMAP_LIVE_SERVER_URL to the server's origin, e.g. https://jmap.example.com \
         (see docs/manual-test-live-server.md)",
    );

    let credentials = match env::var("JMAP_LIVE_SERVER_TOKEN") {
        Ok(token) => Credentials::bearer(token),
        Err(_) => {
            let user = env::var("JMAP_LIVE_SERVER_USER").expect(
                "set JMAP_LIVE_SERVER_USER and JMAP_LIVE_SERVER_PASSWORD, or \
                 JMAP_LIVE_SERVER_TOKEN for Bearer",
            );
            let password = env::var("JMAP_LIVE_SERVER_PASSWORD")
                .expect("set JMAP_LIVE_SERVER_PASSWORD alongside JMAP_LIVE_SERVER_USER");
            Credentials::basic(user, password)
        }
    };

    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, credentials)
        .expect("could not fetch the session document from JMAP_LIVE_SERVER_URL")
}

/// The session document names the core capability and at least one account —
/// RFC 8620 §2 requires the former of every conforming server, mock or real.
/// The latter is not spelled out as a hard requirement the way the capability
/// is (this project's own `jmap-mockd` does not put `core` itself in
/// `primaryAccounts`, matching a real server rather than over-asserting on
/// it), but a session naming zero accounts is not one a test account can
/// reach anything through, so it is worth failing loudly on rather than
/// letting every later test fail confusingly instead.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn the_session_names_the_core_capability() {
    let client = connect();
    assert!(
        client.session().capabilities.contains_key(CAPABILITY_CORE),
        "a conforming server always advertises {CAPABILITY_CORE}"
    );
    assert!(
        !client.session().accounts.is_empty(),
        "the credentials this test was given reach no account at all"
    );
}

/// `Core/echo` round-trips an arbitrary JSON value unchanged (RFC 8620 §4) —
/// the smallest proof that a method call reaches this server's API endpoint
/// and comes back parsed as this client expects, rather than merely that its
/// session document does.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn echo_round_trips_through_the_real_api_endpoint() {
    let client = connect();
    let sent = json!({"night-shift": "real-server readiness"});
    assert_eq!(client.echo(sent.clone()).unwrap(), sent);
}

/// If the account has the mail capability, `Mailbox/get` answers with at
/// least one mailbox — every real mailbox has an Inbox. An account with no
/// mail capability at all (a contacts-or-calendars-only test account) is not
/// a failure of this client's, so that case is reported and skipped rather
/// than asserted on: the point of this test is capability-negotiation
/// robustness, which cuts both ways — tolerating what a real deployment
/// does not offer is as much a part of it as reading what it does. That is
/// also why the account id comes from [`Client::primary_account`] rather
/// than reading `Session::primary_accounts` directly: a real server is
/// allowed to omit `primaryAccounts` altogether (RFC 8620 §2), and the
/// robust resolver still finds the account by capability in that case —
/// this test should skip only when there truly is no mail-capable account,
/// not merely because the server left the shortcut out.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn mail_capable_accounts_list_a_non_empty_mailbox_set() {
    let client = connect();
    let Ok(account_id) = client.primary_account(CAPABILITY_MAIL) else {
        eprintln!("server names no primary account for {CAPABILITY_MAIL}; skipping");
        return;
    };

    let mailboxes = client.mailbox_get(&account_id).unwrap();
    assert!(
        !mailboxes.list.is_empty(),
        "a mail-capable account has at least an Inbox"
    );
}

/// If the account has the contacts capability, `AddressBook/get` answers —
/// proof that this client's `AddressBook` type, exercised until now only
/// against `jmap-mockd`'s own fixtures, deserialises what a real server
/// actually sends. Deliberately not asserting a non-empty list the way the
/// mail test asserts an Inbox: unlike a mailbox, nothing requires a fresh
/// account to have created an address book yet, so the round trip succeeding
/// is the claim, not what it returns. An account with no contacts capability
/// at all is reported and skipped, the same tolerance the mail test applies.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn contacts_capable_accounts_can_list_their_address_books() {
    let client = connect();
    let Ok(account_id) = client.primary_account(CAPABILITY_CONTACTS) else {
        eprintln!("server names no primary account for {CAPABILITY_CONTACTS}; skipping");
        return;
    };

    client.address_books(&account_id).unwrap();
}

/// The calendars capability's half of the same proof: `Calendar/get`
/// deserialises against a real server's own JSON. See
/// `contacts_capable_accounts_can_list_their_address_books` for why this does
/// not assert a non-empty list either.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn calendars_capable_accounts_can_list_their_calendars() {
    let client = connect();
    let Ok(account_id) = client.primary_account(CAPABILITY_CALENDARS) else {
        eprintln!("server names no primary account for {CAPABILITY_CALENDARS}; skipping");
        return;
    };

    client.calendars(&account_id).unwrap();
}

/// Credentials for the tests in this file that write, kept deliberately
/// separate from [`connect`]'s: those are handed to every read-only test, so
/// whatever account someone points them at (their own mailbox, an
/// operator's shared test login) must never be the one a `Mailbox/set`
/// lands on. This account exists only if `JMAP_LIVE_SERVER_WRITE_USER`/
/// `_PASSWORD` are set to a login seeded for exactly this purpose — see
/// `docs/manual-test-live-server.md`'s "write-path test" section for the
/// `stw seed` recipe. Absent, not just empty, so the base
/// read-only suite runs without ever needing them: this returns `None`
/// rather than panicking, and the caller skips.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the write-test account"),
    )
}

/// A second throwaway account, distinct from [`connect_for_write`]'s, that
/// the send-email test delivers a message *to* — proof of actual intra-server
/// delivery, not just that `EmailSubmission/set` was accepted. Same
/// present-or-skip shape as `connect_for_write`; see
/// `docs/manual-test-live-server.md`'s "send-email test" section for how to
/// seed it (`stw seed`, same domain, a different local part).
fn connect_recipient() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the recipient account"),
    )
}

/// The one mutating test in this file: `Mailbox/set` creates a folder,
/// renames it (`mailbox_update`'s `PatchObject` path — what `jmap-mail`'s
/// Camel port sends whenever a user renames a folder), reads it back
/// through `Mailbox/get` after each step, then destroys it — proof this
/// client's write path round-trips against a real server's own semantics
/// (id assignment, state changes), not just `jmap-mockd`'s fixtures, which
/// already cover the same shape at no risk. Scoped to the throwaway account
/// [`connect_for_write`] describes; skipped, not failed, when that account
/// is not configured.
///
/// Also checks `Client::all_changes` (RFC 8620 §5.2's `/changes`, the
/// primitive every EDS meta-backend's `get_changes_sync` drives) after each
/// mutation: the mailbox's id must show up in the right bucket
/// (`created`/`updated`/`destroyed`) since the state captured just before
/// that mutation. `jmap-mockd`'s state tokens are this crate's own
/// invention; a real server's tokens, pagination (`hasMoreChanges`), and
/// created/updated/destroyed classification are Stalwart's, not fixed by
/// this workspace, so this is the first place they are exercised end to
/// end. Additionally checks the create-then-rename window as a whole,
/// since the state captured before either: RFC 8620 §5.2's fold rule says
/// an object created and updated within one `/changes` window is reported
/// as created only, and this is the first time that rule is checked
/// against a real server rather than just the mock.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn mailbox_create_rename_then_destroy_round_trips_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let state_before_create = client
        .mailbox_get(&account_id)
        .expect("Mailbox/get failed against the real server")
        .state;

    // Unique per run so a prior run's leftover (e.g. a destroy that failed)
    // cannot be mistaken for this run's own mailbox.
    let name = format!("agent-livewrite-{}", unique_suffix());
    let mailbox = Mailbox {
        name: name.clone(),
        ..Mailbox::default()
    };

    let created = client
        .mailbox_create(&account_id, &mailbox)
        .expect("Mailbox/set create failed against the real server");
    let id = created
        .id
        .clone()
        .expect("the server named the new mailbox");

    let round_tripped = client
        .mailbox_get(&account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.id.as_ref() == Some(&id));
    assert_eq!(
        round_tripped.map(|mailbox| mailbox.name),
        Some(name),
        "the created mailbox does not show up in Mailbox/get afterwards"
    );

    let changes_after_create = client
        .all_changes(&account_id, "Mailbox", &state_before_create)
        .expect("Mailbox/changes failed against the real server");
    assert!(
        changes_after_create.created.contains(&id),
        "Mailbox/changes since before the create does not list the new mailbox as created"
    );

    let state_before_rename = client.mailbox_get(&account_id).unwrap().state;
    let renamed_name = format!("agent-livewrite-renamed-{}", unique_suffix());
    client
        .mailbox_update(&account_id, &id, json!({"name": renamed_name}))
        .expect("Mailbox/set update failed against the real server");

    let round_tripped_after_rename = client
        .mailbox_get(&account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.id.as_ref() == Some(&id));
    assert_eq!(
        round_tripped_after_rename.map(|mailbox| mailbox.name),
        Some(renamed_name),
        "the renamed mailbox does not show the new name in Mailbox/get afterwards"
    );

    let changes_after_rename = client
        .all_changes(&account_id, "Mailbox", &state_before_rename)
        .expect("Mailbox/changes failed against the real server");
    assert!(
        changes_after_rename.updated.contains(&id),
        "Mailbox/changes since before the rename does not list the mailbox as updated"
    );

    // RFC 8620 §5.2's fold rule: an object created and then updated inside
    // one `/changes` window is reported as created, not also updated,
    // because the caller never saw the pre-update state. `ChangeSet::
    // classify` implements this client-side and is already mock-tested
    // (`jmap-client/tests/changes.rs`); this is the first time it is
    // checked against a real server's own `/changes`, which is free to
    // classify the single-response case however it likes as long as the
    // fold comes out right. `state_before_create` spans both the create
    // and the rename above.
    let changes_since_before_create = client
        .all_changes(&account_id, "Mailbox", &state_before_create)
        .expect("Mailbox/changes failed against the real server");
    assert!(
        changes_since_before_create.created.contains(&id),
        "Mailbox/changes spanning both the create and the rename does not list the mailbox as created"
    );
    assert!(
        !changes_since_before_create.updated.contains(&id),
        "a mailbox created and renamed inside one /changes window should classify as created, not updated"
    );

    let state_before_destroy = client.mailbox_get(&account_id).unwrap().state;
    client
        .mailbox_destroy(&account_id, &id)
        .expect("Mailbox/set destroy failed against the real server");

    let changes_after_destroy = client
        .all_changes(&account_id, "Mailbox", &state_before_destroy)
        .expect("Mailbox/changes failed against the real server");
    assert!(
        changes_after_destroy.destroyed.contains(&id),
        "Mailbox/changes since before the destroy does not list the mailbox as destroyed"
    );
}

/// `AddressBook/set` create then destroy — the JMAP calls the collection
/// backend's `create_resource_sync`/`delete_resource_sync` vfuncs
/// issue when a user does "New Address Book" or
/// deletes one, as opposed to [`contact_card_create_update_then_destroy_round_trips_through_the_real_api`]
/// below, which creates a *card inside* the account's existing default
/// address book. `Client::address_book_create`/`address_book_destroy` are
/// mock-tested (`jmap-client/tests/contacts.rs`) but had never been run
/// against a real server before this test: does the server actually let a
/// non-default address book be created this way, and does the created id
/// show up in the collection's own `AddressBook/get` list.
///
/// Confirmed via `Client::address_books` (a list check) rather than
/// `Client::all_changes`, unlike the mailbox/contact/event tests: this crate
/// has no `address_book_get`-style method that exposes a `state` token for
/// this type (`address_books` discards it, matching what D1's own mock tests
/// needed), and adding one is out of scope for a coverage-only increment.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn address_book_create_then_destroy_round_trips_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");

    let name = format!("agent-livewrite-{}", unique_suffix());
    let book = AddressBook {
        name: name.clone(),
        ..AddressBook::default()
    };

    let created = client
        .address_book_create(&account_id, &book)
        .expect("AddressBook/set create failed against the real server");
    let id = created
        .id
        .clone()
        .expect("the server named the new address book");

    let round_tripped = client
        .address_books(&account_id)
        .expect("AddressBook/get failed against the real server")
        .into_iter()
        .find(|book| book.id.as_ref() == Some(&id));
    assert_eq!(
        round_tripped.map(|book| book.name),
        Some(name),
        "the created address book does not show up in AddressBook/get afterwards"
    );

    client
        .address_book_destroy(&account_id, &id)
        .expect("AddressBook/set destroy failed against the real server");

    let still_present = client
        .address_books(&account_id)
        .expect("AddressBook/get failed against the real server")
        .into_iter()
        .any(|book| book.id.as_ref() == Some(&id));
    assert!(
        !still_present,
        "the destroyed address book still shows up in AddressBook/get afterwards"
    );
}

/// The contacts capability's half of the write-path proof: `ContactCard/set`
/// creates a card in the account's default address book, reads it back
/// through `ContactCard/get`, renames it (`contact_update`'s `PatchObject`
/// path — what `jmap-book-sync` sends whenever a user edits a contact),
/// reads it back again, then destroys it. Relies on Stalwart
/// auto-provisioning one default address book per account (confirmed by
/// hand before this test was written) rather than creating one first — the
/// same assumption the mailbox test makes about a default Inbox.
///
/// Also checks `Client::all_changes` after each mutation, same as
/// [`mailbox_create_rename_then_destroy_round_trips_through_the_real_api`]:
/// `jmap-book-sync`'s own polling loop drives `ContactCard/changes`
/// (`lib.rs:206`), and a real server's state tokens and
/// created/updated/destroyed classification for this type had no
/// live-server coverage until now.
///
/// Also checks `Client::contact_query` right after the create and right
/// after the destroy: `jmap-book-sync`'s `list_existing_sync` enumerates an
/// address book via exactly `ContactCardQueryFilter::in_address_book`
/// (`lib.rs:88`), the backend's actual listing path, which — unlike
/// `get`/`set`/`changes` above — had no live-server coverage at all before
/// this test.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn contact_card_create_update_then_destroy_round_trips_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let book_id = client
        .address_books(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the address book");

    let state_before_create = client
        .contact_get(&account_id, &[])
        .expect("ContactCard/get failed against the real server")
        .state;

    let full_name = format!("agent-livewrite-{}", unique_suffix());
    let card = ContactCard::simple(
        book_id.clone(),
        &full_name,
        "agent-livewrite@example.invalid",
    );

    let created = client
        .contact_create(&account_id, &card)
        .expect("ContactCard/set create failed against the real server");
    let id = created.id.clone().expect("the server named the new card");

    let round_tripped = client
        .contact_get(&account_id, std::slice::from_ref(&id))
        .unwrap()
        .list
        .into_iter()
        .next();
    assert_eq!(
        round_tripped
            .and_then(|card| card.name)
            .and_then(|name| name.full),
        Some(full_name),
        "the created card does not show up in ContactCard/get afterwards"
    );

    let changes_after_create = client
        .all_changes(&account_id, "ContactCard", &state_before_create)
        .expect("ContactCard/changes failed against the real server");
    assert!(
        changes_after_create.created.contains(&id),
        "ContactCard/changes since before the create does not list the new card as created"
    );

    let query_after_create = client
        .contact_query(
            &account_id,
            ContactCardQueryFilter::in_address_book(book_id.clone()),
        )
        .expect("ContactCard/query failed against the real server");
    assert!(
        query_after_create.ids.contains(&id),
        "ContactCard/query for the address book does not list the new card"
    );

    let state_before_update = client.contact_get(&account_id, &[]).unwrap().state;
    let renamed_full_name = format!("agent-livewrite-renamed-{}", unique_suffix());
    client
        .contact_update(&account_id, &id, json!({"name/full": renamed_full_name}))
        .expect("ContactCard/set update failed against the real server");

    let round_tripped_after_update = client
        .contact_get(&account_id, std::slice::from_ref(&id))
        .unwrap()
        .list
        .into_iter()
        .next();
    assert_eq!(
        round_tripped_after_update
            .and_then(|card| card.name)
            .and_then(|name| name.full),
        Some(renamed_full_name),
        "the updated card does not show the new name in ContactCard/get afterwards"
    );

    let changes_after_update = client
        .all_changes(&account_id, "ContactCard", &state_before_update)
        .expect("ContactCard/changes failed against the real server");
    assert!(
        changes_after_update.updated.contains(&id),
        "ContactCard/changes since before the update does not list the card as updated"
    );

    let state_before_destroy = client.contact_get(&account_id, &[]).unwrap().state;
    client
        .contact_destroy(&account_id, &id)
        .expect("ContactCard/set destroy failed against the real server");

    let changes_after_destroy = client
        .all_changes(&account_id, "ContactCard", &state_before_destroy)
        .expect("ContactCard/changes failed against the real server");
    assert!(
        changes_after_destroy.destroyed.contains(&id),
        "ContactCard/changes since before the destroy does not list the card as destroyed"
    );

    let query_after_destroy = client
        .contact_query(
            &account_id,
            ContactCardQueryFilter::in_address_book(book_id),
        )
        .expect("ContactCard/query failed against the real server");
    assert!(
        !query_after_destroy.ids.contains(&id),
        "ContactCard/query for the address book still lists the destroyed card"
    );
}

/// `Calendar/set` create, update, then destroy — the calendar counterpart of
/// [`address_book_create_then_destroy_round_trips_through_the_real_api`]
/// above, for the other half of Track D1's collection-creation vfuncs.
/// `Client::calendar_create`/`calendar_destroy` are mock-tested
/// (`jmap-client/tests/calendars.rs`) but had never run against a real
/// server before this test.
///
/// Also checks `Client::calendar_update` (a colour patch), mirroring
/// [`mailbox_create_rename_then_destroy_round_trips_through_the_real_api`]'s
/// create→rename→destroy shape: it is the one pure-client layer of Track
/// D2's calendar-colour write-back (the `ECalMetaBackendClass::
/// source_changed` vfunc's `jmap_cal_sync::CalSync::set_color` issues
/// exactly this call), mock-tested
/// (`jmap-client/tests/calendars.rs::calendar_update_color`) but, unlike the
/// mailbox test's rename, never previously sent to a real server. No
/// `Client::all_changes` check here either, for the same reason the create
/// half already gives: no `_get`-with-explicit-ids method exposes a `state`
/// token for this type.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn calendar_create_then_destroy_round_trips_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");

    let name = format!("agent-livewrite-{}", unique_suffix());
    let calendar = Calendar {
        name: name.clone(),
        ..Calendar::default()
    };

    let created = client
        .calendar_create(&account_id, &calendar)
        .expect("Calendar/set create failed against the real server");
    let id = created
        .id
        .clone()
        .expect("the server named the new calendar");

    let round_tripped = client
        .calendars(&account_id)
        .expect("Calendar/get failed against the real server")
        .into_iter()
        .find(|calendar| calendar.id.as_ref() == Some(&id));
    assert_eq!(
        round_tripped.map(|calendar| calendar.name),
        Some(name),
        "the created calendar does not show up in Calendar/get afterwards"
    );

    client
        .calendar_update(&account_id, &id, json!({"color": "#00ff00"}))
        .expect("Calendar/set update failed against the real server");

    let round_tripped_after_update = client
        .calendars(&account_id)
        .expect("Calendar/get failed against the real server")
        .into_iter()
        .find(|calendar| calendar.id.as_ref() == Some(&id));
    assert_eq!(
        round_tripped_after_update.and_then(|calendar| calendar.color),
        Some("#00ff00".to_string()),
        "the coloured calendar does not show the new colour in Calendar/get afterwards"
    );

    client
        .calendar_destroy(&account_id, &id)
        .expect("Calendar/set destroy failed against the real server");

    let still_present = client
        .calendars(&account_id)
        .expect("Calendar/get failed against the real server")
        .into_iter()
        .any(|calendar| calendar.id.as_ref() == Some(&id));
    assert!(
        !still_present,
        "the destroyed calendar still shows up in Calendar/get afterwards"
    );
}

/// The calendars capability's half of the write-path proof: `CalendarEvent/
/// set` creates an event in the account's default calendar, reads it back
/// through `CalendarEvent/get`, updates it (`event_update`'s `PatchObject`
/// path — what a user editing an event's title in the calendar view sends),
/// reads it back again, then destroys it. Same default-calendar assumption
/// as the contacts test makes about the default address book.
///
/// Also checks `Client::all_changes` after each mutation, same as
/// [`mailbox_create_rename_then_destroy_round_trips_through_the_real_api`]
/// and
/// [`contact_card_create_update_then_destroy_round_trips_through_the_real_api`]:
/// `jmap-cal-sync`'s own polling loop drives `CalendarEvent/changes`, and a
/// real server's state tokens and created/updated/destroyed classification
/// for this type had no live-server coverage until now.
///
/// Also checks `Client::event_query` right after the create and right after
/// the destroy: `jmap-cal-sync::list_existing_sync` enumerates a calendar via
/// exactly `CalendarEventQueryFilter::in_calendar` (`lib.rs:101`), the
/// backend's actual listing path, which — unlike `get`/`set`/`changes`
/// above — had no live-server coverage at all before this test. Mirrors the
/// `ContactCard/query` check
/// [`contact_card_create_update_then_destroy_round_trips_through_the_real_api`]
/// already makes for the address-book listing path.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn calendar_event_create_update_then_destroy_round_trips_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = client
        .calendars(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let state_before_create = client
        .event_get(&account_id, &[])
        .expect("CalendarEvent/get failed against the real server")
        .state;

    let title = format!("agent-livewrite-{}", unique_suffix());
    let event = CalendarEvent::simple(calendar_id.clone(), &title, "2026-08-18T13:00:00", "PT1H");

    let created = client
        .event_create(&account_id, &event)
        .expect("CalendarEvent/set create failed against the real server");
    let id = created.id.clone().expect("the server named the new event");

    let round_tripped = client
        .event_get(&account_id, std::slice::from_ref(&id))
        .unwrap()
        .list
        .into_iter()
        .next();
    assert_eq!(
        round_tripped.and_then(|event| event.title),
        Some(title),
        "the created event does not show up in CalendarEvent/get afterwards"
    );

    let changes_after_create = client
        .all_changes(&account_id, "CalendarEvent", &state_before_create)
        .expect("CalendarEvent/changes failed against the real server");
    assert!(
        changes_after_create.created.contains(&id),
        "CalendarEvent/changes since before the create does not list the new event as created"
    );

    let query_after_create = client
        .event_query(
            &account_id,
            CalendarEventQueryFilter::in_calendar(calendar_id.clone()),
        )
        .expect("CalendarEvent/query failed against the real server");
    assert!(
        query_after_create.ids.contains(&id),
        "CalendarEvent/query for the calendar does not list the new event"
    );

    let state_before_update = client.event_get(&account_id, &[]).unwrap().state;
    let updated_title = format!("agent-livewrite-updated-{}", unique_suffix());
    client
        .event_update(&account_id, &id, json!({"title": updated_title}))
        .expect("CalendarEvent/set update failed against the real server");

    let round_tripped_after_update = client
        .event_get(&account_id, std::slice::from_ref(&id))
        .unwrap()
        .list
        .into_iter()
        .next();
    assert_eq!(
        round_tripped_after_update.and_then(|event| event.title),
        Some(updated_title),
        "the updated event does not show the new title in CalendarEvent/get afterwards"
    );

    let changes_after_update = client
        .all_changes(&account_id, "CalendarEvent", &state_before_update)
        .expect("CalendarEvent/changes failed against the real server");
    assert!(
        changes_after_update.updated.contains(&id),
        "CalendarEvent/changes since before the update does not list the event as updated"
    );

    let state_before_destroy = client.event_get(&account_id, &[]).unwrap().state;
    client
        .event_destroy(&account_id, &id)
        .expect("CalendarEvent/set destroy failed against the real server");

    let changes_after_destroy = client
        .all_changes(&account_id, "CalendarEvent", &state_before_destroy)
        .expect("CalendarEvent/changes failed against the real server");
    assert!(
        changes_after_destroy.destroyed.contains(&id),
        "CalendarEvent/changes since before the destroy does not list the event as destroyed"
    );

    let query_after_destroy = client
        .event_query(
            &account_id,
            CalendarEventQueryFilter::in_calendar(calendar_id),
        )
        .expect("CalendarEvent/query failed against the real server");
    assert!(
        !query_after_destroy.ids.contains(&id),
        "CalendarEvent/query for the calendar still lists the destroyed event"
    );
}

/// The recurring-event counterpart to the test above, and the first
/// real-server exercise of `CalendarEvent`'s singular `recurrenceRule`
/// property (jscalendarbis §3.3.3; this codebase sent RFC 8984's plural
/// `recurrenceRules` array until the fix this test pins). Item 14's own
/// operator verification against Fastmail only ever covered a plain,
/// non-recurring create/edit, so a real, independent JSCalendar
/// implementation had never seen the singular shape — a server that still
/// expected the old array would reject this create with `invalidProperties`,
/// exactly item 14's own original failure mode.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_recurring_event_created_with_the_singular_recurrence_rule_round_trips_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = client
        .calendars(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let title = format!("agent-livewrite-recurring-{}", unique_suffix());
    let mut event = CalendarEvent::simple(calendar_id, &title, "2026-09-07T09:00:00", "PT30M");
    let mut rule = RecurrenceRule::new("daily");
    rule.count = Some(3);
    event.recurrence_rule = Some(rule);

    let created = client
        .event_create(&account_id, &event)
        .expect("CalendarEvent/set create with a recurrenceRule failed against the real server");
    let id = created.id.clone().expect("the server named the new event");

    let round_tripped = client
        .event_get(&account_id, std::slice::from_ref(&id))
        .unwrap()
        .list
        .into_iter()
        .next()
        .expect("the created recurring event does not show up in CalendarEvent/get afterwards");
    let round_tripped_rule = round_tripped
        .recurrence_rule
        .as_ref()
        .expect("the server dropped the recurrenceRule the client sent");
    assert_eq!(round_tripped_rule.frequency, "daily");
    assert_eq!(round_tripped_rule.count, Some(3));

    client
        .event_destroy(&account_id, &id)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// The mail write path's other shape: `Email/import` puts bytes the caller
/// already has into the store, rather than `Mailbox/set`'s create-from-
/// properties. Uploads a small RFC 5322 message via [`Client::upload_blob`],
/// imports it into the account's Inbox, confirms it via `Email/get`, marks it
/// read (`email_update`'s `PatchObject` path — what
/// `jmap-mail-sync::MailSync::set_keywords` sends whenever a user marks a
/// message read/unread or flags it), confirms the keyword via another
/// `Email/get`, downloads the blob back through [`Client::download_blob`],
/// then destroys the message.
///
/// Does not assert the downloaded bytes equal the uploaded bytes verbatim:
/// RFC 8621 §4.8 lets a server repair or re-serialize an imported message
/// (adding a `Received` header, say), so that would be a legitimate answer,
/// not a client bug. Instead it checks the downloaded length against the
/// `size` `Email/get` itself reports, and that the message's (unique, so a
/// leftover from a prior run cannot be mistaken for this one) subject
/// survived the round trip.
///
/// Also checks `Client::all_changes` after each of import/update/destroy,
/// same as the mailbox/contact/event tests: `Client::email_state` (an
/// `Email/get` naming no ids) supplies the "since" state `Email/changes`
/// needs, since `email_get` itself never exposes one (it splits large id
/// lists across several `Email/get` calls and only keeps their `list`s).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn email_import_update_then_destroy_round_trips_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let inbox_id = client
        .mailbox_get(&account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::INBOX))
        .expect("the write-test account needs an Inbox")
        .id
        .expect("the server named the Inbox");

    let subject = format!("agent-livewrite-{}", unique_suffix());
    let message = format!(
        "From: agent-livewrite@example.invalid\r\n\
         To: agent-livewrite@example.invalid\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         It arrived as bytes and it stays bytes.\r\n"
    );

    let upload = client
        .upload_blob(&account_id, "message/rfc822", message.clone().into_bytes())
        .expect("blob upload failed against the real server");

    let state_before_import = client
        .email_state(&account_id)
        .expect("Email/get failed against the real server");

    let imported = client
        .email_import(
            &account_id,
            &EmailImport::new(upload.blob_id, inbox_id.clone()),
        )
        .expect("Email/import failed against the real server");
    let id = imported.id.clone().expect("the server named the new email");

    let changes_after_import = client
        .all_changes(&account_id, "Email", &state_before_import)
        .expect("Email/changes failed against the real server");
    assert!(
        changes_after_import.created.contains(&id),
        "Email/changes since before the import does not list the new message as created"
    );

    // Same call `jmap-mail-sync::message_ids` makes to list a mailbox's
    // messages (`in_mailbox` filter, ascending `receivedAt`, unbounded
    // position) — the folder-listing path itself, not just get/set/changes
    // on an id already known.
    let queried_after_import = client
        .email_query(
            &account_id,
            EmailQueryFilter::in_mailbox(inbox_id.clone()),
            Some(vec![Comparator::ascending("receivedAt")]),
            None,
            0,
        )
        .expect("Email/query failed against the real server");
    assert!(
        queried_after_import.ids.contains(&id),
        "Email/query on the Inbox does not list the newly imported message"
    );

    let fetched = client
        .email_get(&account_id, std::slice::from_ref(&id), None)
        .unwrap()
        .into_iter()
        .next()
        .expect("the imported email does not show up in Email/get afterwards");
    assert_eq!(
        fetched.subject,
        Some(subject),
        "the imported email's subject does not match what was uploaded"
    );

    let state_before_update = client
        .email_state(&account_id)
        .expect("Email/get failed against the real server");
    client
        .email_update(
            &account_id,
            &id,
            json!({format!("keywords/{}", keyword::SEEN): true}),
        )
        .expect("Email/set update failed against the real server");

    let changes_after_update = client
        .all_changes(&account_id, "Email", &state_before_update)
        .expect("Email/changes failed against the real server");
    assert!(
        changes_after_update.updated.contains(&id),
        "Email/changes since before the update does not list the message as updated"
    );

    let fetched_after_update = client
        .email_get(&account_id, std::slice::from_ref(&id), None)
        .unwrap()
        .into_iter()
        .next()
        .expect("the updated email does not show up in Email/get afterwards");
    assert_eq!(
        fetched_after_update
            .keywords
            .as_ref()
            .and_then(|keywords| keywords.get(keyword::SEEN))
            .copied(),
        Some(true),
        "the updated email does not show the $seen keyword in Email/get afterwards"
    );
    let size = fetched_after_update
        .size
        .expect("Email/get named a size for the imported email");
    let blob_id = fetched_after_update
        .blob_id
        .clone()
        .expect("Email/get named a blobId for the imported email");

    let downloaded = client
        .download_blob(&account_id, &blob_id, "message.eml", size)
        .expect("blob download failed against the real server");
    assert_eq!(
        downloaded.len() as u64,
        size,
        "the downloaded blob's length does not match the size Email/get reported"
    );

    let state_before_destroy = client
        .email_state(&account_id)
        .expect("Email/get failed against the real server");
    client
        .email_destroy(&account_id, &id)
        .expect("Email/set destroy failed against the real server");

    let changes_after_destroy = client
        .all_changes(&account_id, "Email", &state_before_destroy)
        .expect("Email/changes failed against the real server");
    assert!(
        changes_after_destroy.destroyed.contains(&id),
        "Email/changes since before the destroy does not list the message as destroyed"
    );

    let queried_after_destroy = client
        .email_query(
            &account_id,
            EmailQueryFilter::in_mailbox(inbox_id.clone()),
            Some(vec![Comparator::ascending("receivedAt")]),
            None,
            0,
        )
        .expect("Email/query failed against the real server");
    assert!(
        !queried_after_destroy.ids.contains(&id),
        "Email/query on the Inbox still lists the destroyed message"
    );
}

/// `Client::send_email` — draft creation chained to `EmailSubmission/set` —
/// against a real server's own submission and delivery machinery, not just
/// `jmap-mockd`'s outbox stub (`jmap-client/tests/mail_send.rs`). Every prior
/// live-server session left this out with the same note: "no SMTP path
/// configured for this throwaway account". That is true of *outbound* relay
/// to the public Internet, which a throwaway domain with no MX/DKIM cannot
/// do — but says nothing about *intra-server* delivery between two accounts
/// on the same deployment, which needs no outbound relay at all. This test
/// sends from [`connect_for_write`]'s account to [`connect_recipient`]'s
/// (both on `agent-livewrite.net`) and polls the recipient's `Email/query`
/// for the message actually landing in its Inbox — proof of delivery, not
/// merely that the server accepted the submission.
///
/// Skipped, not failed, when `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD`
/// are not set, same as every other write-path test's environment gate.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn send_email_delivers_to_a_second_account_on_the_real_server() {
    let Some(sender) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the send-email test");
        return;
    };
    let Some(recipient) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the send-email test"
        );
        return;
    };

    let sender_email = env::var("JMAP_LIVE_SERVER_WRITE_USER").unwrap();
    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();

    let sender_account_id = sender
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let drafts_id = sender
        .mailbox_get(&sender_account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::DRAFTS))
        .expect("the write-test account needs a Drafts mailbox")
        .id
        .expect("the server named the Drafts mailbox");
    let identity_id = sender
        .identities(&sender_account_id)
        .unwrap()
        .into_iter()
        .find(|identity| identity.email == sender_email)
        .expect("the write-test account needs a sending identity for its own address")
        .id
        .expect("the server named the identity");

    let subject = format!("agent-livewrite-send-{}", unique_suffix());
    let draft = Email {
        mailbox_ids: Some([(drafts_id, true)].into()),
        keywords: Some([(keyword::DRAFT.to_owned(), true)].into()),
        from: Some(vec![EmailAddress::new(None, &sender_email)]),
        to: Some(vec![EmailAddress::new(None, &recipient_email)]),
        subject: Some(subject.clone()),
        body_values: Some(
            [(
                "1".to_owned(),
                EmailBodyValue::new("Delivered, not just accepted."),
            )]
            .into(),
        ),
        text_body: Some(vec![EmailBodyPart {
            part_id: Some("1".to_owned()),
            content_type: Some("text/plain".to_owned()),
            ..EmailBodyPart::default()
        }]),
        ..Email::default()
    };

    let (created_email, submission) = sender
        .send_email(&sender_account_id, &draft, &identity_id, None)
        .expect("send_email failed against the real server");
    assert!(
        submission.id.is_some(),
        "the server did not accept the submission"
    );

    let recipient_account_id = recipient
        .primary_account(CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");
    let recipient_inbox_id = recipient
        .mailbox_get(&recipient_account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::INBOX))
        .expect("the recipient account needs an Inbox")
        .id
        .expect("the server named the recipient's Inbox");

    // Local delivery is not necessarily synchronous with the submission
    // response, so poll rather than assume it has already landed.
    let mut delivered_id = None;
    for _ in 0..20 {
        let mut filter = EmailQueryFilter::in_mailbox(recipient_inbox_id.clone());
        filter.subject = Some(subject.clone());
        let found = recipient
            .email_query(&recipient_account_id, filter, None, Some(1), 0)
            .expect("Email/query failed against the real server");
        if let Some(id) = found.ids.into_iter().next() {
            delivered_id = Some(id);
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let delivered_id = delivered_id.unwrap_or_else(|| {
        panic!("the message never showed up in the recipient's Inbox after 20s of polling")
    });

    let delivered = recipient
        .email_get(
            &recipient_account_id,
            std::slice::from_ref(&delivered_id),
            None,
        )
        .unwrap()
        .into_iter()
        .next()
        .expect("the delivered email does not show up in Email/get afterwards");
    assert_eq!(
        delivered.subject,
        Some(subject),
        "the delivered email's subject does not match what was sent"
    );

    // Clean up both sides: the draft-turned-sent copy in the sender's
    // account, and the delivered copy in the recipient's.
    let sent_id = created_email.id.expect("the server named the sent email");
    sender
        .email_destroy(&sender_account_id, &sent_id)
        .expect("Email/set destroy failed for the sender's copy");
    recipient
        .email_destroy(&recipient_account_id, &delivered_id)
        .expect("Email/set destroy failed for the recipient's copy");
}

/// JMAP Sieve (RFC 9661) against a real server: item 48's own live-shape
/// probe, which nothing before this test ran (`jmap-client/tests/sieve.rs`
/// covers create/update/destroy/activate against `jmap-mockd` only). Uploads
/// a trivial valid script (`keep;`, no `require` needed), creates it,
/// confirms `isActive: false`, activates it via `onSuccessActivateScript`
/// and confirms `isActive: true`, then destroys it.
///
/// Two of this test's assertions pin real Stalwart v1.0.0 behaviour that
/// diverges from RFC 9661, found while first writing this probe (a plain
/// `onSuccessActivateScript: null` silently did nothing, so `keep_going`
/// below had no way to reach a destroyable state until the divergence was
/// worked around):
///
/// - `SieveScript/set` with only `"onSuccessActivateScript": null` and no
///   accompanying create/update is a no-op on Stalwart: `isActive` stays
///   `true`. RFC 9661 §2.4 says null "deactivat[es] the currently active
///   script", unconditionally; `jmap-mockd` implements exactly that (see
///   `jmap-mock/src/sieve.rs`), and [`Client::sieve_script_deactivate`]
///   sends exactly the RFC's wire shape. This test does not change that
///   client method to chase the workaround below, since nothing consumes
///   `sieve_script_deactivate` yet (no Evolution filters UI exists) and a
///   future RFC-conformant server must not regress silently if this method
///   were bent to match Stalwart's bug.
/// - The workaround that does work against Stalwart — and that this test
///   uses to reach a destroyable state — is a direct `update: {"isActive":
///   false}`, even though RFC 9661 §2.1 documents `isActive` as
///   server-set, and [`Client::sieve_script_update`]'s own doc comment
///   (written before this test, against the RFC and the mock only) says a
///   server "rejects" that. Stalwart applies it instead.
/// - Destroying the active script fails, but the wire error `type` is
///   `scriptIsActive`, not the RFC-specified `sieveIsActive`
///   ([`jmap_proto::sieve::sieve_set_error::SIEVE_IS_ACTIVE`]). This test
///   asserts the literal string Stalwart actually sends, not the constant.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn sieve_script_create_activate_deactivate_then_destroy_round_trips_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let Ok(account_id) = client.primary_account(CAPABILITY_SIEVE) else {
        eprintln!("server names no primary account for {CAPABILITY_SIEVE}; skipping");
        return;
    };

    let name = format!("agent-livewrite-{}", unique_suffix());
    let upload = client
        .upload_blob(&account_id, "application/sieve", b"keep;\r\n".to_vec())
        .expect("blob upload failed against the real server");

    let created = client
        .sieve_script_create(&account_id, &SieveScript::new(name.clone(), upload.blob_id))
        .expect("SieveScript/set create failed against the real server");
    let id = created.id.clone().expect("the server named the new script");

    let round_tripped = client
        .sieve_scripts(&account_id)
        .expect("SieveScript/get failed against the real server")
        .into_iter()
        .find(|script| script.id.as_ref() == Some(&id))
        .expect("the created script does not show up in SieveScript/get afterwards");
    assert_eq!(round_tripped.name, name);
    assert!(
        !round_tripped.is_active,
        "a freshly created script must not start active"
    );

    client
        .sieve_script_activate(&account_id, &id)
        .expect("SieveScript/set activate failed against the real server");
    let activated = client
        .sieve_scripts(&account_id)
        .unwrap()
        .into_iter()
        .find(|script| script.id.as_ref() == Some(&id))
        .expect("the activated script does not show up in SieveScript/get afterwards");
    assert!(
        activated.is_active,
        "the script does not show isActive: true after onSuccessActivateScript"
    );

    // RFC 9661's own deactivation path: a documented no-op against Stalwart
    // v1.0.0. See this test's doc comment.
    client
        .sieve_script_deactivate(&account_id)
        .expect("SieveScript/set deactivate (RFC shape) was rejected outright");
    let still_active_after_rfc_deactivate = client
        .sieve_scripts(&account_id)
        .unwrap()
        .into_iter()
        .find(|script| script.id.as_ref() == Some(&id))
        .expect("the script does not show up in SieveScript/get afterwards")
        .is_active;
    assert!(
        still_active_after_rfc_deactivate,
        "Stalwart's onSuccessActivateScript: null no-op appears to be fixed: \
         isActive turned false. If this fails, the divergence this test \
         documents is gone — update the doc comment and use \
         sieve_script_deactivate below instead of the direct-patch workaround."
    );

    // Destroying the still-active script is refused; the wire error type is
    // Stalwart's own `scriptIsActive`, not the RFC's `sieveIsActive`.
    match client.sieve_script_destroy(&account_id, &id) {
        Err(jmap_client::Error::Set(set_error)) => {
            assert_eq!(set_error.error_type, "scriptIsActive")
        }
        other => panic!("expected a scriptIsActive SetError, got {other:?}"),
    }

    // The workaround that actually deactivates on Stalwart: a direct patch,
    // despite isActive being modeled as server-set in the RFC.
    client
        .sieve_script_update(&account_id, &id, serde_json::json!({"isActive": false}))
        .expect("the direct isActive:false patch workaround was rejected");
    let deactivated = client
        .sieve_scripts(&account_id)
        .unwrap()
        .into_iter()
        .find(|script| script.id.as_ref() == Some(&id))
        .expect("the deactivated script does not show up in SieveScript/get afterwards");
    assert!(
        !deactivated.is_active,
        "the script still shows isActive: true after the direct-patch workaround"
    );

    client
        .sieve_script_destroy(&account_id, &id)
        .expect("SieveScript/set destroy failed against the real server");
    let still_present = client
        .sieve_scripts(&account_id)
        .expect("SieveScript/get failed against the real server")
        .into_iter()
        .any(|script| script.id.as_ref() == Some(&id));
    assert!(
        !still_present,
        "the destroyed script still shows up in SieveScript/get afterwards"
    );
}

/// JMAP Push (RFC 8620 §7) against a real server. Opens an `EventSource`
/// subscription for the write-test account via
/// [`jmap_client::eventsource::EventSourceSubscription`], creates a mailbox,
/// and confirms a `StateChange` naming `Mailbox` for this account arrives
/// over the wire. Every other exercise of this module was against either a
/// hand-rolled test TCP server (`eventsource.rs`'s own `#[cfg(test)]`
/// module) or `jmap-mockd`'s fixed push loop
/// (`jmap-backend-core/src/push.rs`'s tests) — chunked-transfer framing,
/// TLS negotiation, and the exact `event: state`/`data:` shape a real
/// server sends had no live coverage until now.
///
/// Sleeps briefly after connecting before triggering the mutation: an SSE
/// stream carries no backlog, so a change made before the subscription's
/// `GET` is actually accepted by the server would never be seen, unlike a
/// `/changes` poll which always catches up from a state token.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn push_notifies_a_subscribed_eventsource_of_a_real_mailbox_change() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the push test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let template = client.session().event_source_url.clone();
    assert!(
        !template.trim().is_empty(),
        "the server advertises no eventSourceUrl"
    );

    let url = expand_url(&template, &["Mailbox"], false, 300);
    let headers = client
        .authorization_header()
        .map(|value| vec![("Authorization".to_owned(), value)])
        .unwrap_or_default();
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(headers), CancelFlag::new());

    // Give the connection a moment to be accepted before the mutation below,
    // for the reason this test's doc comment gives.
    std::thread::sleep(Duration::from_millis(500));

    let name = format!("agent-livewrite-push-{}", unique_suffix());
    let mailbox = Mailbox {
        name: name.clone(),
        ..Mailbox::default()
    };
    let created = client
        .mailbox_create(&account_id, &mailbox)
        .expect("Mailbox/set create failed against the real server");
    let id = created
        .id
        .clone()
        .expect("the server named the new mailbox");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut saw_it = false;
    while Instant::now() < deadline {
        let Some(change) = subscription.recv_timeout(Duration::from_secs(1)) else {
            continue;
        };
        if change
            .changed
            .get(&account_id)
            .is_some_and(|type_state| type_state.contains_key("Mailbox"))
        {
            saw_it = true;
            break;
        }
    }
    assert!(
        saw_it,
        "no StateChange naming Mailbox for this account arrived over the \
         EventSource within 20s of creating the mailbox"
    );

    client
        .mailbox_destroy(&account_id, &id)
        .expect("Mailbox/set destroy failed against the real server");
}

/// `Blob/copy` (RFC 8620 §5.7) against a real server: uploading a blob and
/// copying it within the *same* account must produce a distinct blob id that
/// reads back the same content, with the source untouched. This capability
/// is modeled and mock-tested (`blob_management.rs`) but, per
/// `RFC-SUPPORT.md` item 5, was never exercised against a real server at
/// all. `fromAccountId == accountId` rather than two different accounts:
/// see [`blob_copy_into_an_account_you_do_not_own_is_forbidden`]'s doc
/// comment for why a genuine cross-account copy cannot be tested here
/// without ACL sharing this test does not set up.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn blob_copy_within_the_same_real_account_copies_the_content() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the blob-copy test");
        return;
    };

    let account_id = client
        .primary_account(CAPABILITY_BLOB)
        .expect("the write-test account needs the blob capability");

    let text = format!("agent-livewrite-blob-{}", unique_suffix());
    let uploaded = client
        .blob_upload(
            &BlobUploadRequest::new(account_id.clone())
                .create_blob("b0", UploadBlob::from_text(&text, "text/plain")),
        )
        .expect("Blob/upload failed against the real server")
        .created
        .expect("the server named the new blob");
    let source_id = uploaded.get("b0").expect("b0 was created").id.clone();

    let response = client
        .blob_copy(&BlobCopyRequest::new(
            account_id.clone(),
            account_id.clone(),
            [source_id.clone()],
        ))
        .expect("Blob/copy failed against the real server");
    let copied = response.copied.expect("the server copied the blob");
    let copy_id = copied
        .get(&source_id)
        .expect("the source id was copied")
        .clone();
    assert!(
        response
            .not_copied
            .is_none_or(|not_copied| not_copied.is_empty()),
        "the blob copy reported a failure for an id that should have succeeded"
    );

    let source_get = client
        .blob_get(
            &BlobGetRequest::new(account_id.clone(), [source_id]).with_properties(["data:asText"]),
        )
        .expect("Blob/get on the source blob failed against the real server");
    let copy_get = client
        .blob_get(&BlobGetRequest::new(account_id, [copy_id]).with_properties(["data:asText"]))
        .expect("Blob/get on the copied blob failed against the real server");
    assert_eq!(
        source_get.list[0].data_as_text, copy_get.list[0].data_as_text,
        "the copy's content diverged from the source's"
    );
}

/// A genuine `fromAccountId != accountId` copy was the first shape tried
/// here, mirroring the two throwaway accounts every other cross-account test
/// in this file uses. Real Stalwart rejected it outright: `Blob/copy` with a
/// target account the caller does not own answers a `forbidden` method
/// error ("You are not an owner of account ..."), even though `jmap-mockd`'s
/// own implementation allows it unconditionally (`blob_copy_between_two_
/// accounts_copies_the_content` in `blob_management.rs`) — the mock never
/// modeled an ownership check at all. Setting up ACL sharing between the two
/// throwaway accounts so a real cross-account copy could succeed is out of
/// scope for this increment; what is tractable and worth pinning down is
/// that this client surfaces the real server's rejection as a clean
/// [`jmap_client::Error::Method`] rather than a panic or a silently empty
/// `notCopied`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn blob_copy_into_an_account_you_do_not_own_is_forbidden() {
    let Some(from_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the blob-copy test");
        return;
    };
    let Some(to_client) = connect_recipient() else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the blob-copy test");
        return;
    };

    let from_account_id = from_client
        .primary_account(CAPABILITY_BLOB)
        .expect("the write-test account needs the blob capability");
    let to_account_id = to_client
        .primary_account(CAPABILITY_BLOB)
        .expect("the recipient account needs the blob capability");

    let text = format!("agent-livewrite-blob-{}", unique_suffix());
    let uploaded = from_client
        .blob_upload(
            &BlobUploadRequest::new(from_account_id.clone())
                .create_blob("b0", UploadBlob::from_text(&text, "text/plain")),
        )
        .expect("Blob/upload failed against the real server")
        .created
        .expect("the server named the new blob");
    let source_id = uploaded.get("b0").expect("b0 was created").id.clone();

    let error = from_client
        .blob_copy(&BlobCopyRequest::new(
            from_account_id,
            to_account_id,
            [source_id],
        ))
        .expect_err("copying into an account this session does not own must be rejected");
    match error {
        jmap_client::Error::Method(method_error) => {
            assert_eq!(method_error.error_type, "forbidden");
        }
        other => panic!("expected a forbidden Method error, got {other:?}"),
    }
}

/// RFC 9425 Quota against real, computed numbers: `jmap-client/tests/
/// quota.rs` only ever checks that `Client::quotas` deserialises whatever
/// `jmap-mockd`'s fixture was told to answer, never that a quota's `used`
/// figure actually tracks real storage. This imports a message into the
/// write-test account's Inbox and confirms the account's `octets` quota
/// grows afterwards, proof the number Stalwart reports is live rather than
/// static.
///
/// Polls for a few seconds after the import rather than asserting on the
/// very next `Quota/get`: RFC 9425 does not require `used` to be updated
/// synchronously within the same request that changed storage, only that it
/// eventually reflects reality, the same tolerance
/// `push_notifies_a_subscribed_eventsource_of_a_real_mailbox_change` applies
/// to its own asynchronous state.
///
/// Skipped, not failed, when the account advertises the quota capability but
/// `Quota/get` names no per-account `octets` row: confirmed against real
/// Stalwart that this is a legitimate shape, not a bug to work around.
/// Stalwart only materialises a `Quota` object for a scope it actually
/// enforces (RFC 9425 does not require advertising one that never applies),
/// and a freshly seeded account with no configured `maxDiskQuota` enforces
/// none at all — the capability being advertised only promises the *method*
/// works, not that any particular scope is populated. `stw seed` does not
/// set a disk quota by default, so this is the normal state for a throwaway
/// write-test account unless one is configured (see
/// `docs/manual-test-live-server.md`'s write-test account setup).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn quota_used_reflects_a_real_email_import() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    if !client
        .session()
        .accounts
        .get(&account_id)
        .is_some_and(|account| account.has_capability(CAPABILITY_QUOTA))
    {
        eprintln!("the write-test account does not advertise {CAPABILITY_QUOTA}; skipping");
        return;
    }

    let account_octets_quota = |client: &Client| -> Option<Quota> {
        client
            .quotas(&account_id)
            .expect("Quota/get failed against the real server")
            .into_iter()
            .find(|quota| {
                quota.resource_type == quota_resource_type::OCTETS
                    && quota.scope == quota_scope::ACCOUNT
            })
    };

    let Some(before) = account_octets_quota(&client) else {
        eprintln!(
            "the write-test account has no per-account octets quota configured \
             (no maxDiskQuota set); skipping"
        );
        return;
    };

    let inbox_id = client
        .mailbox_get(&account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::INBOX))
        .expect("the write-test account needs an Inbox")
        .id
        .expect("the server named the Inbox");

    let message = format!(
        "From: agent-livewrite@example.invalid\r\n\
         To: agent-livewrite@example.invalid\r\n\
         Subject: agent-quota-{}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Enough bytes to move a quota's used figure. {}\r\n",
        unique_suffix(),
        "padding ".repeat(64)
    );

    let upload = client
        .upload_blob(&account_id, "message/rfc822", message.into_bytes())
        .expect("blob upload failed against the real server");
    let imported = client
        .email_import(&account_id, &EmailImport::new(upload.blob_id, inbox_id))
        .expect("Email/import failed against the real server");
    let id = imported.id.clone().expect("the server named the new email");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after = account_octets_quota(&client)
        .expect("the per-account octets quota disappeared after the import");
    while after.used <= before.used && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        after = account_octets_quota(&client)
            .expect("the per-account octets quota disappeared after the import");
    }

    client
        .email_destroy(&account_id, &id)
        .expect("Email/set destroy failed against the real server");

    assert!(
        after.used > before.used,
        "quota used ({}) did not grow after importing a message (before: {})",
        after.used,
        before.used
    );
}

/// Checks that `book_id` is not visible to `recipient` on `owner_account_id`,
/// tolerating either an outright `forbidden` (the recipient has no shared
/// books at all on that account) or a successful list that simply omits
/// `book_id` (another shared book, from a concurrently running test against
/// the same real account pair, keeps the account visible). Asserting only
/// "not present" rather than "forbidden" is what makes this safe under
/// `cargo test`'s default parallelism.
fn assert_recipient_cannot_see_address_book(
    recipient: &Client,
    owner_account_id: &Id,
    book_id: &Id,
    when: &str,
) {
    match recipient.address_books(owner_account_id) {
        Err(jmap_client::Error::Method(method_error)) => {
            assert_eq!(method_error.error_type, "forbidden", "{when}");
        }
        Ok(books) => {
            assert!(
                books.iter().all(|book| book.id.as_ref() != Some(book_id)),
                "the recipient can still see the address book {when}"
            );
        }
        other => panic!("unexpected AddressBook/get result {when}: {other:?}"),
    }
}

/// Mailbox equivalent of [`assert_recipient_cannot_see_address_book`].
fn assert_recipient_cannot_see_mailbox(
    recipient: &Client,
    owner_account_id: &Id,
    mailbox_id: &Id,
    when: &str,
) {
    match recipient.mailbox_get(owner_account_id) {
        Err(jmap_client::Error::Method(method_error)) => {
            assert_eq!(method_error.error_type, "forbidden", "{when}");
        }
        Ok(response) => {
            assert!(
                response
                    .list
                    .iter()
                    .all(|mailbox| mailbox.id.as_ref() != Some(mailbox_id)),
                "the recipient can still see the mailbox {when}"
            );
        }
        other => panic!("unexpected Mailbox/get result {when}: {other:?}"),
    }
}

/// Calendar equivalent of [`assert_recipient_cannot_see_address_book`].
fn assert_recipient_cannot_see_calendar(
    recipient: &Client,
    owner_account_id: &Id,
    calendar_id: &Id,
    when: &str,
) {
    match recipient.calendars(owner_account_id) {
        Err(jmap_client::Error::Method(method_error)) => {
            assert_eq!(method_error.error_type, "forbidden", "{when}");
        }
        Ok(calendars) => {
            assert!(
                calendars
                    .iter()
                    .all(|calendar| calendar.id.as_ref() != Some(calendar_id)),
                "the recipient can still see the calendar {when}"
            );
        }
        other => panic!("unexpected Calendar/get result {when}: {other:?}"),
    }
}

/// `AddressBook shareWith` (RFC 9610 §2) against a real server: the mock
/// enforces the full grant/widen/revoke lifecycle
/// (`jmap-client/tests/contacts.rs`,
/// `address_book_shared_with_a_principal_is_visible_only_to_them` and
/// `address_book_share_can_be_widened_then_revoked`), built from what
/// `examples/sharing-capability-probe.rs` recorded probing a live Stalwart's
/// wire shape, but nothing here has exercised the same enforcement between
/// two real accounts. This creates an address book on the write-test
/// account, confirms the recipient account gets `forbidden` on it before
/// any grant, shares it, confirms the recipient now sees it with `myRights`
/// computed from the grant, then revokes the grant and confirms `forbidden`
/// returns.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn address_book_shared_with_a_second_real_account_is_visible_only_after_the_grant() {
    let Some(owner) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the ACL-sharing test");
        return;
    };
    let Some(recipient) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the ACL-sharing test"
        );
        return;
    };
    if !owner
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the ACL-sharing test"
        );
        return;
    }

    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();
    let owner_account_id = owner
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_email),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner account cannot resolve the recipient's principal id by email");

    let name = format!("agent-livewrite-acl-{}", unique_suffix());
    let book = owner
        .address_book_create(&owner_account_id, &AddressBook::new(name))
        .expect("AddressBook/set create failed against the real server");
    let book_id = book
        .id
        .clone()
        .expect("the server named the new address book");

    assert_recipient_cannot_see_address_book(
        &recipient,
        &owner_account_id,
        &book_id,
        "before any grant",
    );

    owner
        .address_book_update(
            &owner_account_id,
            &book_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayRead": true}}}),
        )
        .expect("AddressBook/set shareWith failed against the real server");

    let shared_books = recipient
        .address_books(&owner_account_id)
        .expect("AddressBook/get failed for the recipient after the grant");
    let shared = shared_books
        .iter()
        .find(|book| book.id.as_ref() == Some(&book_id))
        .expect("the recipient does not see the shared address book");
    let rights = shared
        .my_rights
        .as_ref()
        .expect("myRights is computed from the grant");
    assert_eq!(rights.may_read, Some(true));

    owner
        .address_book_update(
            &owner_account_id,
            &book_id,
            json!({format!("shareWith/{}", recipient_principal_id.as_str()): null}),
        )
        .expect("AddressBook/set revoke failed against the real server");

    assert_recipient_cannot_see_address_book(
        &recipient,
        &owner_account_id,
        &book_id,
        "after the grant is revoked",
    );

    owner
        .address_book_destroy(&owner_account_id, &book_id)
        .expect("AddressBook/set destroy failed against the real server (cleanup)");
}

/// The same grant/widen/revoke enforcement as
/// [`address_book_shared_with_a_second_real_account_is_visible_only_after_the_grant`],
/// checked for `Mailbox` instead of `AddressBook`: mirrors the mock's own
/// `mailbox_shared_with_a_principal_is_visible_only_to_them`
/// (`jmap-client/tests/mail_folders.rs`) but between two real accounts on
/// Stalwart, since `MailboxRights` is a distinct shape
/// (`mayReadItems`/`mayAddItems`) from `AddressBookRights`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn mailbox_shared_with_a_second_real_account_is_visible_only_after_the_grant() {
    let Some(owner) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the ACL-sharing test");
        return;
    };
    let Some(recipient) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the ACL-sharing test"
        );
        return;
    };
    if !owner
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the ACL-sharing test"
        );
        return;
    }

    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();
    let owner_account_id = owner
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_email),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner account cannot resolve the recipient's principal id by email");

    let name = format!("agent-livewrite-acl-{}", unique_suffix());
    let mailbox = owner
        .mailbox_create(
            &owner_account_id,
            &Mailbox {
                name: name.clone(),
                ..Mailbox::default()
            },
        )
        .expect("Mailbox/set create failed against the real server");
    let mailbox_id = mailbox
        .id
        .clone()
        .expect("the server named the new mailbox");

    assert_recipient_cannot_see_mailbox(
        &recipient,
        &owner_account_id,
        &mailbox_id,
        "before any grant",
    );

    owner
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true, "mayAddItems": true}}}),
        )
        .expect("Mailbox/set shareWith failed against the real server");

    let shared_mailboxes = recipient
        .mailbox_get(&owner_account_id)
        .expect("Mailbox/get failed for the recipient after the grant")
        .list;
    let shared = shared_mailboxes
        .iter()
        .find(|mailbox| mailbox.id.as_ref() == Some(&mailbox_id))
        .expect("the recipient does not see the shared mailbox");
    let rights = shared
        .my_rights
        .as_ref()
        .expect("myRights is computed from the grant");
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_add_items, Some(true));

    owner
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({format!("shareWith/{}", recipient_principal_id.as_str()): null}),
        )
        .expect("Mailbox/set revoke failed against the real server");

    assert_recipient_cannot_see_mailbox(
        &recipient,
        &owner_account_id,
        &mailbox_id,
        "after the grant is revoked",
    );

    owner
        .mailbox_destroy(&owner_account_id, &mailbox_id)
        .expect("Mailbox/set destroy failed against the real server (cleanup)");
}

/// The same grant/revoke enforcement as
/// [`address_book_shared_with_a_second_real_account_is_visible_only_after_the_grant`],
/// checked for `Calendar` instead: mirrors the mock's own
/// `calendar_shared_with_a_principal_is_visible_only_to_them`
/// (`jmap-client/tests/calendars.rs`) but between two real accounts on
/// Stalwart, since `CalendarRights` is a distinct shape
/// (`mayReadItems`/`mayReadFreeBusy`) from the other two.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn calendar_shared_with_a_second_real_account_is_visible_only_after_the_grant() {
    let Some(owner) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the ACL-sharing test");
        return;
    };
    let Some(recipient) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the ACL-sharing test"
        );
        return;
    };
    if !owner
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the ACL-sharing test"
        );
        return;
    }

    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();
    let owner_account_id = owner
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_email),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner account cannot resolve the recipient's principal id by email");

    let name = format!("agent-livewrite-acl-{}", unique_suffix());
    let calendar = owner
        .calendar_create(&owner_account_id, &Calendar::new(name))
        .expect("Calendar/set create failed against the real server");
    let calendar_id = calendar
        .id
        .clone()
        .expect("the server named the new calendar");

    assert_recipient_cannot_see_calendar(
        &recipient,
        &owner_account_id,
        &calendar_id,
        "before any grant",
    );

    owner
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true, "mayReadFreeBusy": true}}}),
        )
        .expect("Calendar/set shareWith failed against the real server");

    let shared_calendars = recipient
        .calendars(&owner_account_id)
        .expect("Calendar/get failed for the recipient after the grant");
    let shared = shared_calendars
        .iter()
        .find(|calendar| calendar.id.as_ref() == Some(&calendar_id))
        .expect("the recipient does not see the shared calendar");
    let rights = shared
        .my_rights
        .as_ref()
        .expect("myRights is computed from the grant");
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_read_free_busy, Some(true));

    owner
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({format!("shareWith/{}", recipient_principal_id.as_str()): null}),
        )
        .expect("Calendar/set revoke failed against the real server");

    assert_recipient_cannot_see_calendar(
        &recipient,
        &owner_account_id,
        &calendar_id,
        "after the grant is revoked",
    );

    owner
        .calendar_destroy(&owner_account_id, &calendar_id)
        .expect("Calendar/set destroy failed against the real server (cleanup)");
}

/// `ShareNotification/get` (RFC 9670 section 4) against a real server: the
/// mock's own coverage (`jmap-client/tests/share_notifications.rs`) proves
/// the recipient reads back a notification for a grant and another for a
/// revoke, but does so from a single-account model where the notification
/// is fetched against the owner's account id. RFC 9670 places the
/// notification in the *recipient's own* account instead, which is what
/// `examples/sharing-capability-probe.rs` queried against a live Stalwart
/// (`bob.single_call(..., "ShareNotification/get", &GetRequest::all(bob_account))`)
/// but never turned into a repeatable test. This shares an address book
/// with the recipient, polls the recipient's own account for the resulting
/// notification, revokes the grant, and polls for the follow-up
/// notification recording the revoke.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn address_book_share_grant_and_revoke_deliver_a_real_share_notification() {
    let Some(owner) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the ShareNotification test"
        );
        return;
    };
    let Some(recipient) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the ShareNotification test"
        );
        return;
    };
    if !owner
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the ShareNotification test"
        );
        return;
    }

    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();
    let owner_account_id = owner
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let recipient_account_id = recipient
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the recipient account needs the contacts capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_email),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner account cannot resolve the recipient's principal id by email");

    let name = format!("agent-livewrite-sharenotif-{}", unique_suffix());
    let book = owner
        .address_book_create(&owner_account_id, &AddressBook::new(name))
        .expect("AddressBook/set create failed against the real server");
    let book_id = book
        .id
        .clone()
        .expect("the server named the new address book");

    let before_count = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient before any grant")
        .len();

    owner
        .address_book_update(
            &owner_account_id,
            &book_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayRead": true}}}),
        )
        .expect("AddressBook/set shareWith failed against the real server");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after_grant = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient after the grant");
    while after_grant.len() <= before_count && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        after_grant = recipient
            .share_notifications(&recipient_account_id)
            .expect("ShareNotification/get failed for the recipient after the grant");
    }
    let granted = after_grant
        .iter()
        .find(|notification| notification.object_id == book_id)
        .expect("the grant produced no ShareNotification for this address book");
    assert_eq!(granted.object_type, "AddressBook");
    assert_eq!(granted.object_account_id, owner_account_id);
    assert_eq!(
        granted.new_rights.as_ref().and_then(|v| v.get("mayRead")),
        Some(&json!(true))
    );

    owner
        .address_book_update(
            &owner_account_id,
            &book_id,
            json!({format!("shareWith/{}", recipient_principal_id.as_str()): null}),
        )
        .expect("AddressBook/set revoke failed against the real server");

    // Diff by id rather than assume an ordering: a real server is free to
    // return newest-first (Stalwart does) while the mock's fixture-backed
    // list happens to grow oldest-first, so neither `.next()` nor
    // `.next_back()` alone is a safe way to name "the revoke's own entry".
    let granted_ids: std::collections::HashSet<_> =
        after_grant.iter().filter_map(|n| n.id.clone()).collect();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after_revoke = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient after the revoke");
    let revoke_entry = |notifications: &[jmap_proto::principals::ShareNotification]| {
        notifications
            .iter()
            .find(|n| n.object_id == book_id && !granted_ids.contains(n.id.as_ref().unwrap()))
            .cloned()
    };
    let mut revoked = revoke_entry(&after_revoke);
    while revoked.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        after_revoke = recipient
            .share_notifications(&recipient_account_id)
            .expect("ShareNotification/get failed for the recipient after the revoke");
        revoked = revoke_entry(&after_revoke);
    }
    let revoked = revoked.expect("the revoke produced no additional ShareNotification");

    // A real server is free to represent "revoked" either by omitting
    // `newRights` entirely (the mock's own shape) or by naming every right
    // explicitly `false` (what Stalwart actually sends); either is RFC 9670
    // compliant, so this checks "no right is granted", not "the field is
    // absent".
    let no_right_granted = revoked.new_rights.as_ref().is_none_or(|rights| {
        rights
            .as_object()
            .is_some_and(|obj| obj.values().all(|v| v != &json!(true)))
    });
    assert!(
        no_right_granted,
        "a right is still granted after a revoke, got {:?}",
        revoked.new_rights
    );

    owner
        .address_book_destroy(&owner_account_id, &book_id)
        .expect("AddressBook/set destroy failed against the real server (cleanup)");
}

/// The same `ShareNotification/get` coverage as
/// [`address_book_share_grant_and_revoke_deliver_a_real_share_notification`],
/// but for a Mailbox grant. AddressBook and Mailbox notifications go through
/// the same server code path, but this had only been confirmed live for
/// AddressBook; the mock's own equivalent
/// (`mailbox_share_grant_delivers_a_share_notification`,
/// jmap-client/tests/share_notifications.rs) covers Mailbox specifically.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn mailbox_share_grant_and_revoke_deliver_a_real_share_notification() {
    let Some(owner) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the ShareNotification test"
        );
        return;
    };
    let Some(recipient) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the ShareNotification test"
        );
        return;
    };
    if !owner
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the ShareNotification test"
        );
        return;
    }

    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();
    let owner_account_id = owner
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let recipient_account_id = recipient
        .primary_account(CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_email),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner account cannot resolve the recipient's principal id by email");

    let name = format!("agent-livewrite-sharenotif-{}", unique_suffix());
    let mailbox = owner
        .mailbox_create(&owner_account_id, &Mailbox::new(name))
        .expect("Mailbox/set create failed against the real server");
    let mailbox_id = mailbox
        .id
        .clone()
        .expect("the server named the new mailbox");

    let before_count = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient before any grant")
        .len();

    owner
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true}}}),
        )
        .expect("Mailbox/set shareWith failed against the real server");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after_grant = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient after the grant");
    while after_grant.len() <= before_count && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        after_grant = recipient
            .share_notifications(&recipient_account_id)
            .expect("ShareNotification/get failed for the recipient after the grant");
    }
    let granted = after_grant
        .iter()
        .find(|notification| notification.object_id == mailbox_id)
        .expect("the grant produced no ShareNotification for this mailbox");
    assert_eq!(granted.object_type, "Mailbox");
    assert_eq!(granted.object_account_id, owner_account_id);
    assert_eq!(
        granted
            .new_rights
            .as_ref()
            .and_then(|v| v.get("mayReadItems")),
        Some(&json!(true))
    );

    owner
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({format!("shareWith/{}", recipient_principal_id.as_str()): null}),
        )
        .expect("Mailbox/set revoke failed against the real server");

    // Diff by id rather than assume an ordering, same reasoning as the
    // AddressBook test above.
    let granted_ids: std::collections::HashSet<_> =
        after_grant.iter().filter_map(|n| n.id.clone()).collect();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after_revoke = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient after the revoke");
    let revoke_entry = |notifications: &[jmap_proto::principals::ShareNotification]| {
        notifications
            .iter()
            .find(|n| n.object_id == mailbox_id && !granted_ids.contains(n.id.as_ref().unwrap()))
            .cloned()
    };
    let mut revoked = revoke_entry(&after_revoke);
    while revoked.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        after_revoke = recipient
            .share_notifications(&recipient_account_id)
            .expect("ShareNotification/get failed for the recipient after the revoke");
        revoked = revoke_entry(&after_revoke);
    }
    let revoked = revoked.expect("the revoke produced no additional ShareNotification");

    // A real server is free to represent "revoked" either by omitting
    // `newRights` entirely or by naming every right explicitly `false`; this
    // checks "no right is granted", not "the field is absent" (same
    // divergence the AddressBook test above documents).
    let no_right_granted = revoked.new_rights.as_ref().is_none_or(|rights| {
        rights
            .as_object()
            .is_some_and(|obj| obj.values().all(|v| v != &json!(true)))
    });
    assert!(
        no_right_granted,
        "a right is still granted after a revoke, got {:?}",
        revoked.new_rights
    );

    owner
        .mailbox_destroy(&owner_account_id, &mailbox_id)
        .expect("Mailbox/set destroy failed against the real server (cleanup)");
}

/// The same `ShareNotification/get` coverage as
/// [`address_book_share_grant_and_revoke_deliver_a_real_share_notification`],
/// but for a Calendar grant. AddressBook, Mailbox and Calendar notifications
/// go through the same server code path, but this had only been confirmed
/// live for AddressBook and Mailbox so far; the mock's own equivalent
/// (`calendar_share_grant_delivers_a_share_notification`,
/// jmap-client/tests/share_notifications.rs) covers Calendar specifically.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn calendar_share_grant_and_revoke_deliver_a_real_share_notification() {
    let Some(owner) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the ShareNotification test"
        );
        return;
    };
    let Some(recipient) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the ShareNotification test"
        );
        return;
    };
    if !owner
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the ShareNotification test"
        );
        return;
    }

    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();
    let owner_account_id = owner
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let recipient_account_id = recipient
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the recipient account needs the calendars capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_email),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner account cannot resolve the recipient's principal id by email");

    let name = format!("agent-livewrite-sharenotif-{}", unique_suffix());
    let calendar = Calendar {
        name: name.clone(),
        ..Calendar::default()
    };
    let calendar = owner
        .calendar_create(&owner_account_id, &calendar)
        .expect("Calendar/set create failed against the real server");
    let calendar_id = calendar
        .id
        .clone()
        .expect("the server named the new calendar");

    let before_count = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient before any grant")
        .len();

    owner
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true}}}),
        )
        .expect("Calendar/set shareWith failed against the real server");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after_grant = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient after the grant");
    while after_grant.len() <= before_count && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        after_grant = recipient
            .share_notifications(&recipient_account_id)
            .expect("ShareNotification/get failed for the recipient after the grant");
    }
    let granted = after_grant
        .iter()
        .find(|notification| notification.object_id == calendar_id)
        .expect("the grant produced no ShareNotification for this calendar");
    assert_eq!(granted.object_type, "Calendar");
    assert_eq!(granted.object_account_id, owner_account_id);
    assert_eq!(
        granted
            .new_rights
            .as_ref()
            .and_then(|v| v.get("mayReadItems")),
        Some(&json!(true))
    );

    owner
        .calendar_update(
            &owner_account_id,
            &calendar_id,
            json!({format!("shareWith/{}", recipient_principal_id.as_str()): null}),
        )
        .expect("Calendar/set revoke failed against the real server");

    // Diff by id rather than assume an ordering, same reasoning as the
    // AddressBook test above.
    let granted_ids: std::collections::HashSet<_> =
        after_grant.iter().filter_map(|n| n.id.clone()).collect();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after_revoke = recipient
        .share_notifications(&recipient_account_id)
        .expect("ShareNotification/get failed for the recipient after the revoke");
    let revoke_entry = |notifications: &[jmap_proto::principals::ShareNotification]| {
        notifications
            .iter()
            .find(|n| n.object_id == calendar_id && !granted_ids.contains(n.id.as_ref().unwrap()))
            .cloned()
    };
    let mut revoked = revoke_entry(&after_revoke);
    while revoked.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        after_revoke = recipient
            .share_notifications(&recipient_account_id)
            .expect("ShareNotification/get failed for the recipient after the revoke");
        revoked = revoke_entry(&after_revoke);
    }
    let revoked = revoked.expect("the revoke produced no additional ShareNotification");

    // A real server is free to represent "revoked" either by omitting
    // `newRights` entirely or by naming every right explicitly `false`; this
    // checks "no right is granted", not "the field is absent" (same
    // divergence the AddressBook test above documents).
    let no_right_granted = revoked.new_rights.as_ref().is_none_or(|rights| {
        rights
            .as_object()
            .is_some_and(|obj| obj.values().all(|v| v != &json!(true)))
    });
    assert!(
        no_right_granted,
        "a right is still granted after a revoke, got {:?}",
        revoked.new_rights
    );

    owner
        .calendar_destroy(&owner_account_id, &calendar_id)
        .expect("Calendar/set destroy failed against the real server (cleanup)");
}

/// `Principal/getAvailability` (draft-ietf-jmap-calendars section 2.2) against a real
/// server: creates an event with an invited participant on the owner's default calendar,
/// queries free/busy for the owner's principal over that window, asserts that the
/// busy interval is reported with confirmed status, then destroys the event and
/// verifies the busy period is no longer reported.
///
/// When a second account is provisioned via `JMAP_LIVE_SERVER_RECIPIENT_USER`, this
/// also verifies that the recipient account cannot read the unshared owner's busy
/// interval (reported as empty, matching RFC 9670 section 4 and calendars draft
/// section 2.2).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn calendar_event_with_participant_reports_busy_period_in_free_busy_query() {
    let Some(owner) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the free/busy test");
        return;
    };
    if !owner
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!("server does not advertise {CAPABILITY_PRINCIPALS}; skipping the free/busy test");
        return;
    }

    let _owner_email = env::var("JMAP_LIVE_SERVER_WRITE_USER").unwrap();
    let recipient_client = connect_recipient();

    let suffix = unique_suffix();
    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER")
        .unwrap_or_else(|_| format!("attendee-{}@agent-livewrite.net", suffix));

    let owner_account_id = owner
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = owner
        .calendars(&owner_account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let current_principal_id = owner
        .session()
        .accounts
        .get(&owner_account_id)
        .and_then(|acct| acct.account_capabilities.get(CAPABILITY_PRINCIPALS))
        .and_then(|cap| cap.get("currentUserPrincipalId"))
        .and_then(|id| id.as_str())
        .map(Id::from);

    let owner_principal_id = current_principal_id
        .or_else(|| {
            owner
                .principal_query(
                    &owner_account_id,
                    PrincipalQueryFilter::email(&_owner_email),
                )
                .ok()
                .and_then(|ids| ids.into_iter().next())
        })
        .or_else(|| {
            owner
                .principals(&owner_account_id)
                .ok()?
                .into_iter()
                .find(|p| p.email.as_deref() == Some(&_owner_email) || p.name == _owner_email)
                .and_then(|p| p.id)
        })
        .expect("the owner account has no principal id for free/busy lookup");

    let hour = 1 + (suffix % 20) as u8;
    let start_time = format!("2026-10-15T{hour:02}:00:00");
    let expected_utc_start = format!("2026-10-15T{hour:02}:00:00Z");
    let expected_utc_end = format!("2026-10-15T{:02}:00:00Z", hour + 1);

    let title = format!("agent-freebusy-{}", suffix);
    let mut participant = Participant::new("Recipient", &recipient_email);
    participant.participation_status = Some("needs-action".to_owned());
    participant.roles = Some([("attendee".to_owned(), true)].into());
    participant.send_to = Some([("imip".to_owned(), format!("mailto:{recipient_email}"))].into());

    let mut participants = BTreeMap::new();
    participants.insert(
        "attendee-1".to_owned(),
        serde_json::to_value(&participant).unwrap(),
    );

    let mut event = CalendarEvent::simple(calendar_id, &title, &start_time, "PT1H");
    event.participants = Some(participants);

    let created = owner
        .event_create(&owner_account_id, &event)
        .expect("CalendarEvent/set create failed against the real server");
    let event_id = created.id.clone().expect("the server named the new event");

    let periods = owner
        .get_availability(
            &owner_account_id,
            &owner_principal_id,
            "2026-10-15T00:00:00Z",
            "2026-10-16T00:00:00Z",
            false,
        )
        .expect("Principal/getAvailability failed against the real server");

    let busy_slot = periods.iter().find(|p| {
        p.utc_start.as_str() == expected_utc_start && p.utc_end.as_str() == expected_utc_end
    });
    assert!(
        busy_slot.is_some(),
        "owner free/busy query did not report the busy interval {expected_utc_start}..{expected_utc_end}, got: {periods:?}"
    );
    assert_eq!(
        busy_slot.unwrap().busy_status,
        "confirmed",
        "expected confirmed busy status for the scheduled event"
    );

    if let Some(ref recipient) = recipient_client {
        let recipient_account_id = recipient
            .primary_account(CAPABILITY_CALENDARS)
            .expect("the recipient account needs the calendars capability");
        let recipient_view = recipient
            .get_availability(
                &recipient_account_id,
                &owner_principal_id,
                &*expected_utc_start,
                &*expected_utc_end,
                false,
            )
            .expect("Principal/getAvailability failed for recipient query");
        assert!(
            recipient_view.is_empty(),
            "recipient saw ungranted owner free/busy, expected empty list: {recipient_view:?}"
        );
    }

    owner
        .event_destroy(&owner_account_id, &event_id)
        .expect("CalendarEvent/set destroy failed against the real server (cleanup)");

    let periods_after = owner
        .get_availability(
            &owner_account_id,
            &owner_principal_id,
            "2026-10-15T00:00:00Z",
            "2026-10-16T00:00:00Z",
            false,
        )
        .expect("Principal/getAvailability failed against the real server after destroy");

    let busy_after = periods_after.iter().find(|p| {
        p.utc_start.as_str() == expected_utc_start && p.utc_end.as_str() == expected_utc_end
    });
    assert!(
        busy_after.is_none(),
        "busy interval still reported after destroying the event"
    );
}

/// `ContactCard/parse` (RFC 9610 §3.4) against a real server: uploads a vCard
/// blob, parses it into a typed JSContact `ContactCard`, asserts the parsed
/// properties (name and email), verifies `properties` projection filtering,
/// confirms that invalid vCard data is placed in `notParsable`, and proves the
/// parsed card can be stored via `ContactCard/set` create and then destroyed.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn contact_card_parse_parses_an_uploaded_vcard_blob_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the contact card parse test"
        );
        return;
    };
    let Ok(account_id) = client.primary_account(CAPABILITY_CONTACTS) else {
        eprintln!("server names no primary account for {CAPABILITY_CONTACTS}; skipping");
        return;
    };
    if !client
        .session()
        .capabilities
        .contains_key(CAPABILITY_CONTACTS_PARSE)
    {
        eprintln!("server does not advertise {CAPABILITY_CONTACTS_PARSE}; skipping");
        return;
    }

    let suffix = unique_suffix();
    let full_name = format!("Vera Live-{}", suffix);
    let email_addr = format!("vera-{}@example.com", suffix);
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         FN:{full_name}\r\n\
         N:Live-{suffix};Vera;;;\r\n\
         EMAIL;TYPE=WORK:{email_addr}\r\n\
         END:VCARD\r\n"
    );

    let upload = client
        .upload_blob(&account_id, "text/vcard", vcard.into_bytes())
        .expect("blob upload failed against the real server");

    let response = client
        .contact_card_parse(&ContactCardParseRequest::new(
            account_id.clone(),
            [upload.blob_id.clone()],
        ))
        .expect("ContactCard/parse failed against the real server");

    let parsed_map = response
        .parsed
        .expect("ContactCard/parse response missing parsed map");
    let mut parsed_card = parsed_map
        .get(&upload.blob_id)
        .expect("uploaded blobId missing from parsed map")
        .clone();
    assert_eq!(
        parsed_card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some(full_name.as_str()),
        "parsed card name full mismatch"
    );
    let emails = parsed_card
        .emails
        .as_ref()
        .expect("parsed card missing emails map");
    assert!(
        emails.values().any(|e| e.address == email_addr),
        "parsed card emails missing {email_addr}: {emails:?}"
    );

    // Verify properties projection filtering.
    let filtered_response = client
        .contact_card_parse(
            &ContactCardParseRequest::new(account_id.clone(), [upload.blob_id.clone()])
                .properties(["name"]),
        )
        .expect("ContactCard/parse with properties failed against the real server");
    let filtered_card = filtered_response
        .parsed
        .as_ref()
        .and_then(|p| p.get(&upload.blob_id))
        .cloned()
        .expect("uploaded blobId missing in filtered parsed map");
    assert_eq!(
        filtered_card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some(full_name.as_str())
    );
    assert!(
        filtered_card.emails.is_none(),
        "properties=['name'] filter should have omitted emails"
    );

    // Verify unparsable content is placed in notParsable.
    let bad_upload = client
        .upload_blob(&account_id, "text/vcard", b"not a valid vcard".to_vec())
        .expect("bad blob upload failed against the real server");
    let bad_response = client
        .contact_card_parse(&ContactCardParseRequest::new(
            account_id.clone(),
            [bad_upload.blob_id.clone()],
        ))
        .expect("ContactCard/parse on bad blob failed against the real server");
    let not_parsable = bad_response
        .not_parsable
        .expect("expected notParsable list in response");
    assert!(
        not_parsable.contains(&bad_upload.blob_id),
        "bad blob id not reported in notParsable: {not_parsable:?}"
    );

    // Verify that the parsed ContactCard can be filed into an address book via
    // ContactCard/set create, and clean it up afterwards.
    let books = client
        .address_books(&account_id)
        .expect("AddressBook/get failed against the real server");
    let book_id = books
        .iter()
        .find(|b| b.is_default == Some(true))
        .or_else(|| books.first())
        .and_then(|b| b.id.clone())
        .expect("account has no address book for the test");

    parsed_card.address_book_ids = Some([(book_id.clone(), true)].into());
    let created = client
        .contact_create(&account_id, &parsed_card)
        .expect("ContactCard/set create with parsed card failed against the real server");
    let card_id = created.id.expect("the server named the new card");

    let fetched = client
        .contact_get(&account_id, std::slice::from_ref(&card_id))
        .expect("ContactCard/get failed for created card")
        .list
        .into_iter()
        .next()
        .expect("created card not found in ContactCard/get");
    assert_eq!(
        fetched.name.as_ref().and_then(|n| n.full.as_deref()),
        Some(full_name.as_str())
    );

    // Clean up: destroy the created card and verify it is gone.
    client
        .contact_destroy(&account_id, &card_id)
        .expect("ContactCard/set destroy failed against the real server (cleanup)");

    let fetched_after = client
        .contact_get(&account_id, std::slice::from_ref(&card_id))
        .expect("ContactCard/get failed after destroy");
    assert!(
        fetched_after.list.is_empty(),
        "destroyed card still returned in ContactCard/get"
    );
}

/// `ParticipantIdentity/*` (draft-ietf-jmap-calendars-28 §3) against a real server:
/// exercises `Client::participant_identities` (`ParticipantIdentity/get`), validates
/// that `ParticipantIdentity/get` with unknown ids returns `notFound`, proves that
/// server-enforced validation rules reject client-supplied `id`, client-supplied
/// `isDefault`, missing `calendarAddress`, and unassigned `calendarAddress` with
/// `invalidProperties` errors, confirms that destroying an unknown identity returns
/// `notFound`, and (if an identity is provisioned for the write account) proves that
/// `participant_identity_update` and `participant_identity_set_default` round-trip.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn participant_identity_lifecycle_and_validation_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping participant identity test"
        );
        return;
    };
    let Ok(account_id) = client.primary_account(CAPABILITY_CALENDARS) else {
        eprintln!("server names no primary account for {CAPABILITY_CALENDARS}; skipping");
        return;
    };
    if !client
        .session()
        .capabilities
        .contains_key(CAPABILITY_CALENDARS)
    {
        eprintln!("server does not advertise {CAPABILITY_CALENDARS}; skipping");
        return;
    }

    // 1. Fetch all participant identities visible to this account.
    let identities = client
        .participant_identities(&account_id)
        .expect("ParticipantIdentity/get failed against the real server");

    // 2. Querying unknown ids via ParticipantIdentity/get reports them in notFound.
    let unknown_id = Id::new("nonexistent-pi-id");
    let not_found_arg = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CALENDARS],
            "ParticipantIdentity/get",
            &GetRequest::ids(account_id.clone(), [unknown_id.as_str()]),
        )
        .expect("ParticipantIdentity/get with ids failed against the real server");
    let not_found_list: Vec<Id> = serde_json::from_value(
        not_found_arg
            .get("notFound")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new())),
    )
    .expect("could not deserialize notFound list");
    assert_eq!(not_found_list, vec![unknown_id]);

    // 3. Destroying a nonexistent identity returns notFound.
    match client.participant_identity_destroy(&account_id, &Id::new("nonexistent-pi-destroy")) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
        }
        other => panic!("expected notFound SetError for unknown identity destroy, got {other:?}"),
    }

    // 4. Server-enforced creation constraints:
    // a. Client-supplied id must be rejected with invalidProperties (property 'id').
    match client.participant_identity_create(
        &account_id,
        &ParticipantIdentity::new("Agent").with_id("client-supplied-pi-id"),
    ) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "invalidProperties");
            assert!(
                err.properties
                    .as_deref()
                    .unwrap_or(&[])
                    .contains(&"id".to_string()),
                "expected property 'id' in invalidProperties: {err:?}"
            );
        }
        other => {
            panic!("expected invalidProperties SetError for client-supplied id, got {other:?}")
        }
    }

    // b. Client-supplied isDefault must be rejected with invalidProperties (property 'isDefault').
    match client.participant_identity_create(
        &account_id,
        &ParticipantIdentity::new("Agent").is_default(false),
    ) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "invalidProperties");
            assert!(
                err.properties
                    .as_deref()
                    .unwrap_or(&[])
                    .contains(&"isDefault".to_string()),
                "expected property 'isDefault' in invalidProperties: {err:?}"
            );
        }
        other => {
            panic!(
                "expected invalidProperties SetError for client-supplied isDefault, got {other:?}"
            )
        }
    }

    // c. Missing calendarAddress (mandatory per draft-ietf-jmap-calendars-28 §3) must be rejected.
    match client.participant_identity_create(&account_id, &ParticipantIdentity::new("Agent")) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "invalidProperties");
            assert!(
                err.properties
                    .as_deref()
                    .unwrap_or(&[])
                    .contains(&"calendarAddress".to_string()),
                "expected property 'calendarAddress' in invalidProperties: {err:?}"
            );
        }
        other => {
            panic!("expected invalidProperties SetError for missing calendarAddress, got {other:?}")
        }
    }

    // d. Unconfigured calendarAddress must be rejected with invalidProperties.
    match client.participant_identity_create(
        &account_id,
        &ParticipantIdentity::new("Agent")
            .with_calendar_address("mailto:unconfigured-address@example.invalid"),
    ) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "invalidProperties");
            assert!(
                err.properties
                    .as_deref()
                    .unwrap_or(&[])
                    .contains(&"calendarAddress".to_string()),
                "expected property 'calendarAddress' in invalidProperties: {err:?}"
            );
        }
        other => {
            panic!(
                "expected invalidProperties SetError for unconfigured calendarAddress, got {other:?}"
            )
        }
    }

    // 5. If this account already has provisioned participant identities, verify update and default selection.
    if let Some(first) = identities.first() {
        assert!(
            first.id.is_some(),
            "identity must have a server-assigned id"
        );
        assert!(
            first.calendar_address.is_some(),
            "identity must have a calendarAddress"
        );
        let id = first.id.as_ref().unwrap();
        let original_name = first.name.clone();
        let updated_name = format!("{original_name} (updated)");

        // Update identity name via PatchObject
        client
            .participant_identity_update(&account_id, id, json!({"name": updated_name}))
            .expect("ParticipantIdentity/set update failed against real server");

        let read_back = client
            .participant_identities(&account_id)
            .expect("ParticipantIdentity/get failed after update");
        let updated_item = read_back
            .iter()
            .find(|pi| pi.id.as_ref() == Some(id))
            .expect("updated identity missing from get");
        assert_eq!(updated_item.name, updated_name);

        // Restore original identity name
        client
            .participant_identity_update(&account_id, id, json!({"name": original_name}))
            .expect("ParticipantIdentity/set restore update failed");

        // Verify setting default
        client
            .participant_identity_set_default(&account_id, id)
            .expect("ParticipantIdentity/set onSuccessSetIsDefault failed");
    }
}

/// `CalendarEventNotification/*` (draft-ietf-jmap-calendars-28 §8):
/// exercises `Client::calendar_event_notifications` (`CalendarEventNotification/get`),
/// validates that querying unknown ids via `CalendarEventNotification/get` reports
/// them in `notFound`, exercises `Client::calendar_event_notification_query`
/// (`CalendarEventNotification/query`) with empty and time-range filters, verifies
/// `CalendarEventNotification/changes` via `Client::changes`, verifies server-enforced
/// constraints rejecting direct `create` and `update` calls with `forbidden`, and
/// confirms that destroying a nonexistent notification with a formatted id returns `notFound`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn calendar_event_notification_lifecycle_and_validation_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping calendar event notification test"
        );
        return;
    };
    let Ok(account_id) = client.primary_account(CAPABILITY_CALENDARS) else {
        eprintln!("server names no primary account for {CAPABILITY_CALENDARS}; skipping");
        return;
    };
    if !client
        .session()
        .capabilities
        .contains_key(CAPABILITY_CALENDARS)
    {
        eprintln!("server does not advertise {CAPABILITY_CALENDARS}; skipping");
        return;
    }

    // 1. Fetch all calendar event notifications visible to this account (draft §8).
    let notifications = client
        .calendar_event_notifications(&account_id)
        .expect("CalendarEventNotification/get failed against the real server");

    // 2. Querying unknown ids via CalendarEventNotification/get reports them in notFound.
    let unknown_id = Id::new("nonexistent-cen-id");
    let not_found_arg = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CALENDARS],
            "CalendarEventNotification/get",
            &GetRequest::ids(account_id.clone(), [unknown_id.as_str()]),
        )
        .expect("CalendarEventNotification/get with ids failed against the real server");
    let not_found_list: Vec<Id> = serde_json::from_value(
        not_found_arg
            .get("notFound")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new())),
    )
    .expect("could not deserialize notFound list");
    assert_eq!(not_found_list, vec![unknown_id]);

    // 3. Querying notifications via CalendarEventNotification/query:
    // a. Empty filter returns the id list matching get count.
    let query_ids = client
        .calendar_event_notification_query(&account_id, CalendarEventNotificationQueryFilter::new())
        .expect("CalendarEventNotification/query failed against the real server");
    assert_eq!(query_ids.len(), notifications.len());

    // b. Filter with after/before time window executes cleanly.
    let filtered_ids = client
        .calendar_event_notification_query(
            &account_id,
            CalendarEventNotificationQueryFilter::new()
                .with_after(UtcDate::new("2026-01-01T00:00:00Z"))
                .with_before(UtcDate::new("2027-01-01T00:00:00Z")),
        )
        .expect("CalendarEventNotification/query with time filter failed against the real server");
    assert!(filtered_ids.len() <= query_ids.len());

    // 4. Server-enforced creation and update constraints:
    // CalendarEventNotification objects are strictly server-created (draft §8).
    // a. Direct create call must be rejected with forbidden.
    let create_resp = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CALENDARS],
            "CalendarEventNotification/set",
            &json!({
                "accountId": account_id,
                "create": {
                    "new_notif": {
                        "eventId": "evt-dummy",
                        "created": "2026-10-01T00:00:00Z",
                    }
                }
            }),
        )
        .expect("CalendarEventNotification/set create call failed on wire");
    assert_eq!(
        create_resp["notCreated"]["new_notif"]["type"],
        json!("forbidden"),
        "expected forbidden error when client attempts to create CalendarEventNotification"
    );

    // b. Direct update call must be rejected with forbidden.
    let update_resp = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CALENDARS],
            "CalendarEventNotification/set",
            &json!({
                "accountId": account_id,
                "update": {
                    "u1": {
                        "comment": "forbidden update",
                    }
                }
            }),
        )
        .expect("CalendarEventNotification/set update call failed on wire");
    assert_eq!(
        update_resp["notUpdated"]["u1"]["type"],
        json!("forbidden"),
        "expected forbidden error when client attempts to update CalendarEventNotification"
    );

    // 5. Querying changes via CalendarEventNotification/changes:
    if let Some(state_str) = not_found_arg.get("state").and_then(|s| s.as_str()) {
        let state = jmap_proto::State::new(state_str);
        let changes = client
            .changes(&account_id, "CalendarEventNotification", &state)
            .expect("CalendarEventNotification/changes failed against the real server");
        assert_eq!(changes.old_state, state);
    }

    // 6. Destroying a nonexistent notification with a formatted id reports notFound.
    match client.calendar_event_notification_destroy(&account_id, &Id::new("n000000000000")) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
        }
        other => {
            panic!("expected notFound SetError for unknown notification destroy, got {other:?}")
        }
    }

    // 7. If any notification exists for this account, verify destroy roundtrip.
    if let Some(first) = notifications.first()
        && let Some(id) = &first.id
    {
        client
            .calendar_event_notification_destroy(&account_id, id)
            .expect("destroying existing CalendarEventNotification failed");
    }
}

/// `EmailSubmission/*` (RFC 8621 §7):
/// exercises `Client::email_submission_query` (`EmailSubmission/query`) with empty,
/// undoStatus, emailIds, threadIds, identityIds, and time-range filters,
/// verifies `Client::email_submission_get` (`EmailSubmission/get`) returns matching
/// submissions and populates `notFound` for unknown ids,
/// verifies `EmailSubmission/changes` via `Client::changes` and `Client::all_changes`,
/// confirms that canceling an unknown submission via `Client::cancel_email_submission` returns `notFound`,
/// validates server-enforced creation constraints rejecting client-supplied `id`,
/// missing `emailId`/`identityId`, and invalid properties with `invalidProperties`,
/// and verifies that destroying a nonexistent submission via `EmailSubmission/set` reports `notFound`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn email_submission_lifecycle_and_validation_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping email submission test");
        return;
    };
    let Ok(account_id) = client
        .primary_account(CAPABILITY_SUBMISSION)
        .or_else(|_| client.primary_account(CAPABILITY_MAIL))
    else {
        eprintln!(
            "server names no primary account for {CAPABILITY_SUBMISSION} or {CAPABILITY_MAIL}; skipping"
        );
        return;
    };
    if !client
        .session()
        .capabilities
        .contains_key(CAPABILITY_SUBMISSION)
    {
        eprintln!("server does not advertise {CAPABILITY_SUBMISSION}; skipping");
        return;
    }

    // 1. Query submissions via Client::email_submission_query (RFC 8621 §7.3):
    // a. Empty filter returns all submission IDs visible to this account.
    let all_submission_ids = client
        .email_submission_query(&account_id, EmailSubmissionQueryFilter::new())
        .expect("EmailSubmission/query with empty filter failed against real server");

    // b. Filter by undoStatus ("pending" and "final").
    let pending_ids = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new().with_undo_status("pending"),
        )
        .expect("EmailSubmission/query with undoStatus=pending failed against real server");
    assert!(pending_ids.len() <= all_submission_ids.len());

    let final_ids = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new().with_undo_status("final"),
        )
        .expect("EmailSubmission/query with undoStatus=final failed against real server");
    assert!(final_ids.len() <= all_submission_ids.len());

    // c. Filter with time range executes cleanly.
    let time_filtered_ids = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new()
                .after(UtcDate::new("2026-01-01T00:00:00Z"))
                .before(UtcDate::new("2027-01-01T00:00:00Z")),
        )
        .expect("EmailSubmission/query with time range failed against real server");
    assert!(time_filtered_ids.len() <= all_submission_ids.len());

    // d. Filter by nonexistent emailIds / threadIds / identityIds returns empty.
    let non_matching_ids = client
        .email_submission_query(
            &account_id,
            EmailSubmissionQueryFilter::new()
                .with_email_ids([Id::new("nonexistent-email-id")])
                .with_thread_ids([Id::new("nonexistent-thread-id")])
                .with_identity_ids([Id::new("nonexistent-identity-id")]),
        )
        .expect("EmailSubmission/query with non-matching ids failed against real server");
    assert!(non_matching_ids.is_empty());

    // 2. Fetch submissions via Client::email_submission_get (RFC 8621 §7.4):
    // a. Unknown id is omitted from the list returned by email_submission_get.
    let unknown_id = Id::new("nonexistent-submission-id");
    let missing_list = client
        .email_submission_get(&account_id, [unknown_id.clone()])
        .expect("EmailSubmission/get with unknown id failed against real server");
    assert!(missing_list.is_empty());

    // b. Raw EmailSubmission/get confirms unknown id is placed in notFound.
    let get_raw = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/get",
            &GetRequest::ids(account_id.clone(), [unknown_id.as_str()]),
        )
        .expect("EmailSubmission/get single_call failed on wire");
    let not_found_list: Vec<Id> = serde_json::from_value(
        get_raw
            .get("notFound")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new())),
    )
    .expect("could not deserialize notFound list");
    assert_eq!(not_found_list, vec![unknown_id]);

    // c. If any submissions exist, verify email_submission_get returns them with valid ids.
    if !all_submission_ids.is_empty() {
        let fetched = client
            .email_submission_get(&account_id, all_submission_ids.iter().cloned())
            .expect("EmailSubmission/get for existing submissions failed");
        assert_eq!(fetched.len(), all_submission_ids.len());
        for submission in &fetched {
            assert!(submission.id.is_some());
        }
    }

    // 3. Querying changes via EmailSubmission/changes (RFC 8620 §5.2):
    if let Some(state_str) = get_raw.get("state").and_then(|s| s.as_str()) {
        let state = jmap_proto::State::new(state_str);
        let changes = client
            .changes(&account_id, "EmailSubmission", &state)
            .expect("EmailSubmission/changes failed against real server");
        assert_eq!(changes.old_state, state);

        let change_set = client
            .all_changes(&account_id, "EmailSubmission", &state)
            .expect("EmailSubmission all_changes failed against real server");
        assert_eq!(change_set.new_state, changes.new_state);
    }

    // 4. Canceling a nonexistent submission via Client::cancel_email_submission returns notFound.
    match client.cancel_email_submission(&account_id, &Id::new("nonexistent-cancel-sub")) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
        }
        other => {
            panic!("expected notFound SetError for nonexistent submission cancel, got {other:?}")
        }
    }

    // 5. Server-enforced creation constraints on EmailSubmission/set (RFC 8621 §7):
    // a. Client-supplied id must be rejected with invalidProperties (property 'id').
    let id_reject_resp = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/set",
            &json!({
                "accountId": account_id,
                "create": {
                    "sub_client_id": {
                        "id": "client-supplied-submission-id",
                        "emailId": "nonexistent-email-id",
                        "identityId": "nonexistent-identity-id"
                    }
                }
            }),
        )
        .expect("EmailSubmission/set create call failed on wire");
    assert_eq!(
        id_reject_resp["notCreated"]["sub_client_id"]["type"],
        json!("invalidProperties"),
        "expected invalidProperties when client supplies an id for EmailSubmission"
    );
    let id_props: Vec<String> =
        serde_json::from_value(id_reject_resp["notCreated"]["sub_client_id"]["properties"].clone())
            .expect("could not parse properties array");
    assert!(
        id_props.contains(&"id".to_string()) || id_props.contains(&"emailId".to_string()),
        "expected property 'id' or 'emailId' in invalidProperties: {id_props:?}"
    );

    // b. Missing emailId and identityId must be rejected with invalidProperties.
    let empty_reject_resp = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/set",
            &json!({
                "accountId": account_id,
                "create": {
                    "sub_empty": {}
                }
            }),
        )
        .expect("EmailSubmission/set empty create call failed on wire");
    assert_eq!(
        empty_reject_resp["notCreated"]["sub_empty"]["type"],
        json!("invalidProperties"),
        "expected invalidProperties when emailId and identityId are omitted"
    );

    // c. Nonexistent emailId must be rejected with invalidProperties (property 'emailId').
    let nonexistent_email_resp = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/set",
            &json!({
                "accountId": account_id,
                "create": {
                    "sub_no_email": {
                        "emailId": "nonexistent-email-id",
                        "identityId": "nonexistent-identity-id"
                    }
                }
            }),
        )
        .expect("EmailSubmission/set nonexistent email create call failed on wire");
    assert_eq!(
        nonexistent_email_resp["notCreated"]["sub_no_email"]["type"],
        json!("invalidProperties")
    );

    // 6. Destroying a nonexistent submission via EmailSubmission/set reports notFound.
    let destroy_resp = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/set",
            &json!({
                "accountId": account_id,
                "destroy": ["nonexistent-destroy-sub"]
            }),
        )
        .expect("EmailSubmission/set destroy call failed on wire");
    assert_eq!(
        destroy_resp["notDestroyed"]["nonexistent-destroy-sub"]["type"],
        json!("notFound"),
        "expected notFound in notDestroyed for unknown submission destroy"
    );
}

/// Exercise server-enforced request and call limits: maxCallsInRequest, maxObjectsInGet,
/// and maxObjectsInSet. Validates that requests conforming to the advertised limits succeed,
/// calls exceeding limits are rejected with the documented RFC 8620 error types, and
/// Client::email_get transparently chunks across maxObjectsInGet.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn server_call_and_object_limits_enforced_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    // 1. Audit core capability advertised limits.
    let max_calls = client
        .session()
        .max_calls_in_request()
        .expect("session core capability must advertise maxCallsInRequest");
    let max_objects_in_get = client
        .session()
        .max_objects_in_get()
        .expect("session core capability must advertise maxObjectsInGet");
    let max_objects_in_set = client
        .session()
        .max_objects_in_set()
        .expect("session core capability must advertise maxObjectsInSet");

    assert_eq!(max_calls, 16);
    assert_eq!(max_objects_in_get, 500);
    assert_eq!(max_objects_in_set, 500);

    // 2. Provoke maxCallsInRequest:
    // a. Exactly maxCallsInRequest (16) calls in one request succeed with 200 OK.
    let mut req_16 = Request::new([CAPABILITY_CORE]);
    for i in 0..max_calls {
        req_16 = req_16
            .call("Core/echo", &json!({"index": i}), format!("c{i}"))
            .unwrap();
    }
    let resp_16 = client
        .api_call(&req_16)
        .expect("request with maxCallsInRequest calls must succeed");
    assert_eq!(resp_16.method_responses.len(), max_calls as usize);

    // b. Exceeding maxCallsInRequest by 1 call (17 calls) is refused whole with HTTP 400
    // and Problem Details limit: maxCallsInRequest.
    let mut req_17 = Request::new([CAPABILITY_CORE]);
    for i in 0..=max_calls {
        req_17 = req_17
            .call("Core/echo", &json!({"index": i}), format!("c{i}"))
            .unwrap();
    }
    let err_17 = client
        .api_call(&req_17)
        .expect_err("request exceeding maxCallsInRequest must be rejected");
    match err_17 {
        Error::Http {
            status,
            problem: Some(problem),
        } => {
            assert_eq!(status, 400);
            assert_eq!(problem.error_type, "urn:ietf:params:jmap:error:limit");
            assert_eq!(
                problem.extra.get("limit"),
                Some(&json!("maxCallsInRequest"))
            );
        }
        other => panic!("expected HTTP 400 limit error, got: {other:?}"),
    }

    // 3. Provoke maxObjectsInGet:
    // a. Single call with exactly maxObjectsInGet (500) ids succeeds with 200 OK.
    let ids_500: Vec<Id> = (0..max_objects_in_get)
        .map(|i| Id::new(format!("limit-probe-{i}")))
        .collect();
    let get_500 = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/get",
            &json!({"accountId": account_id, "ids": ids_500}),
        )
        .expect("Email/get with maxObjectsInGet ids must succeed");
    let not_found_500: Vec<String> =
        serde_json::from_value(get_500["notFound"].clone()).expect("notFound must be a list");
    assert_eq!(not_found_500.len(), max_objects_in_get as usize);

    // b. Single call with maxObjectsInGet + 1 (501) ids is refused with requestTooLarge.
    let ids_501: Vec<Id> = (0..=max_objects_in_get)
        .map(|i| Id::new(format!("limit-probe-{i}")))
        .collect();
    let err_get_501 = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/get",
            &json!({"accountId": account_id, "ids": ids_501}),
        )
        .expect_err("Email/get with 501 ids must be rejected with requestTooLarge");
    match err_get_501 {
        Error::Method(method_error) => {
            assert_eq!(method_error.error_type, method::REQUEST_TOO_LARGE);
        }
        other => panic!("expected requestTooLarge MethodError, got: {other:?}"),
    }

    // c. High-level Client::email_get with 501 ids transparently splits across requests
    // bounding chunks to maxObjectsInGet, avoiding requestTooLarge and returning all results.
    let emails = client
        .email_get(&account_id, &ids_501, None)
        .expect("Client::email_get must split across maxObjectsInGet and succeed");
    assert!(emails.is_empty(), "nonexistent probe ids yield empty list");

    // 4. Provoke maxObjectsInSet:
    // a. Single call with exactly maxObjectsInSet (500) destroy ids succeeds with 200 OK.
    let destroy_500: Vec<Id> = (0..max_objects_in_set)
        .map(|i| Id::new(format!("limit-destroy-probe-{i}")))
        .collect();
    let set_500 = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/set",
            &json!({"accountId": account_id, "destroy": destroy_500}),
        )
        .expect("Email/set destroy with maxObjectsInSet ids must succeed");
    assert!(
        set_500["notDestroyed"].is_object(),
        "expected notDestroyed object for nonexistent ids"
    );

    // b. Single call with maxObjectsInSet + 1 (501) destroy ids is refused with requestTooLarge.
    let destroy_501: Vec<Id> = (0..=max_objects_in_set)
        .map(|i| Id::new(format!("limit-destroy-probe-{i}")))
        .collect();
    let err_set_501 = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/set",
            &json!({"accountId": account_id, "destroy": destroy_501}),
        )
        .expect_err("Email/set destroy with 501 ids must be rejected with requestTooLarge");
    match err_set_501 {
        Error::Method(method_error) => {
            assert_eq!(method_error.error_type, method::REQUEST_TOO_LARGE);
        }
        other => panic!("expected requestTooLarge MethodError for Email/set, got: {other:?}"),
    }
}

/// Protocol edge cases (RFC 8620 §3.7 and §5.3) against a real server:
/// 1. Optimistic locking with `ifInState`: verifies that `ContactCard/set` with a
///    mismatched (outdated) state is rejected with MethodError `stateMismatch` and
///    leaves the data untouched, while a matching `ifInState` succeeds.
/// 2. Back-reference failure when referenced call fails (RFC 8620 §3.7):
///    request calling unknown method `c0` followed by `ContactCard/get` `c1` with
///    `#ids` referencing `c0` returns `unknownMethod` for `c0` and
///    `invalidResultReference` for `c1`.
/// 3. Back-reference failure when referenced call succeeds but property path is missing:
///    request calling `Core/echo` `c0` followed by `ContactCard/get` `c1` with `#ids`
///    referencing nonexistent path in `c0` returns `Core/echo` for `c0` and
///    `invalidResultReference` for `c1`.
/// 4. Successful back-reference resolution: chaining `ContactCard/query` ->
///    `ContactCard/get` via `ids_ref` in a single request resolves `/ids` into `#ids`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn protocol_edges_optimistic_locking_and_backreference_failures_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the protocol edges test"
        );
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");

    let books = client
        .address_books(&account_id)
        .expect("AddressBook/get failed against the real server");
    let book_id = books
        .iter()
        .find(|b| b.is_default == Some(true))
        .or_else(|| books.first())
        .and_then(|b| b.id.clone())
        .expect("account has no address book for the test");

    // 1. Optimistic locking: verify ifInState mismatch rejection.
    let state_before = client
        .contact_get(&account_id, &[])
        .expect("contact_get failed")
        .state;

    let full_name = format!("agent-edge-{}", unique_suffix());
    let card = ContactCard::simple(book_id.clone(), &full_name, "agent-edge@example.invalid");
    let created = client
        .contact_create(&account_id, &card)
        .expect("contact_create failed");
    let card_id = created.id.expect("server must return id for created card");

    let state_after_create = client
        .contact_get(&account_id, &[])
        .expect("contact_get failed")
        .state;
    assert_ne!(state_before, state_after_create);

    // Mismatched state must be rejected with stateMismatch without destroying the card.
    let mismatch_request = SetRequest::<ContactCard>::new(account_id.clone())
        .destroy(card_id.clone())
        .if_in_state(state_before.clone());
    let mismatch_err = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
            "ContactCard/set",
            &mismatch_request,
        )
        .expect_err("ContactCard/set with mismatched ifInState must fail");
    match mismatch_err {
        Error::Method(err) => {
            assert_eq!(err.error_type, method::STATE_MISMATCH);
        }
        other => panic!("expected stateMismatch MethodError, got {other:?}"),
    }

    // Verify card still exists.
    let fetched = client
        .contact_get(&account_id, std::slice::from_ref(&card_id))
        .expect("contact_get failed");
    assert_eq!(fetched.list.len(), 1);

    // Matching state succeeds in destroying the card.
    let match_request = SetRequest::<ContactCard>::new(account_id.clone())
        .destroy(card_id.clone())
        .if_in_state(state_after_create);
    let match_resp = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
            "ContactCard/set",
            &match_request,
        )
        .expect("ContactCard/set with matching ifInState must succeed");
    let destroyed: Vec<Id> = serde_json::from_value(match_resp["destroyed"].clone())
        .expect("destroyed must be an array of IDs");
    assert!(destroyed.contains(&card_id));

    // 2. Back-reference failure when referenced call fails.
    let mut get_failed = GetRequest::all(account_id.clone());
    get_failed.ids_ref = Some(ResultReference {
        result_of: "c0".to_owned(),
        name: "ContactCard/query".to_owned(),
        path: "/ids".to_owned(),
    });
    let req_failed = Request::new([CAPABILITY_CORE, CAPABILITY_CONTACTS])
        .call("ContactCard/nonexistentQuery", &json!({}), "c0")
        .unwrap()
        .call("ContactCard/get", &get_failed, "c1")
        .unwrap();
    let resp_failed = client
        .api_call(&req_failed)
        .expect("api_call must succeed at HTTP level");
    assert_eq!(resp_failed.method_responses.len(), 2);

    assert!(resp_failed.method_responses[0].is_error());
    let err0: jmap_proto::error::MethodError = resp_failed.method_responses[0].parse().unwrap();
    assert_eq!(err0.error_type, method::UNKNOWN_METHOD);

    assert!(resp_failed.method_responses[1].is_error());
    let err1: jmap_proto::error::MethodError = resp_failed.method_responses[1].parse().unwrap();
    assert_eq!(err1.error_type, method::INVALID_RESULT_REFERENCE);

    // 3. Back-reference failure when referenced call succeeds but property path is missing.
    let mut get_bad_path = GetRequest::all(account_id.clone());
    get_bad_path.ids_ref = Some(ResultReference {
        result_of: "c0".to_owned(),
        name: "Core/echo".to_owned(),
        path: "/nonexistentPath".to_owned(),
    });
    let req_bad_path = Request::new([CAPABILITY_CORE, CAPABILITY_CONTACTS])
        .call("Core/echo", &json!({"greeting": "hello"}), "c0")
        .unwrap()
        .call("ContactCard/get", &get_bad_path, "c1")
        .unwrap();
    let resp_bad_path = client
        .api_call(&req_bad_path)
        .expect("api_call must succeed at HTTP level");
    assert_eq!(resp_bad_path.method_responses.len(), 2);
    assert_eq!(resp_bad_path.method_responses[0].name, "Core/echo");
    assert!(resp_bad_path.method_responses[1].is_error());
    let err_bad_path: jmap_proto::error::MethodError =
        resp_bad_path.method_responses[1].parse().unwrap();
    assert_eq!(err_bad_path.error_type, method::INVALID_RESULT_REFERENCE);

    // 4. Successful back-reference resolution chaining Principal/query -> Principal/get.
    let mut get_success = GetRequest::all(account_id.clone());
    get_success.ids_ref = Some(ResultReference {
        result_of: "q0".to_owned(),
        name: "Principal/query".to_owned(),
        path: "/ids".to_owned(),
    });
    let req_success = Request::new([CAPABILITY_CORE, CAPABILITY_PRINCIPALS])
        .call("Principal/query", &json!({"accountId": account_id}), "q0")
        .unwrap()
        .call("Principal/get", &get_success, "g0")
        .unwrap();
    let resp_success = client
        .api_call(&req_success)
        .expect("api_call must succeed at HTTP level");
    assert_eq!(resp_success.method_responses.len(), 2);
    assert_eq!(resp_success.method_responses[0].name, "Principal/query");
    assert_eq!(resp_success.method_responses[1].name, "Principal/get");
    assert!(!resp_success.method_responses[1].is_error());
}

/// Incremental sync paging and state resumption (RFC 8620 §5.2) against a real server:
/// 1. Verifies that `ContactCard/changes` with `maxChanges: 1` returns `hasMoreChanges: true`
///    and an intermediate `newState` when more than 1 change has occurred.
/// 2. Verifies that following the intermediate `newState` with a second page fetches
///    the remaining changes and terminates with `hasMoreChanges: false` at the current state.
/// 3. Validates that high-level `Client::all_changes` automatically traverses multiple pages
///    across intermediate states and folds them into a complete `ChangeSet`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn changes_paging_and_resumption_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the changes paging test"
        );
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");

    let books = client
        .address_books(&account_id)
        .expect("AddressBook/get failed against the real server");
    let book_id = books
        .iter()
        .find(|b| b.is_default == Some(true))
        .or_else(|| books.first())
        .and_then(|b| b.id.clone())
        .expect("account has no address book for the test");

    let initial_state = client
        .contact_get(&account_id, &[])
        .expect("contact_get failed")
        .state;

    // Seed two contacts to create two changes.
    let suffix = unique_suffix();
    let card1 = ContactCard::simple(
        book_id.clone(),
        &format!("agent-page1-{suffix}"),
        "agent-page1@example.invalid",
    );
    let card2 = ContactCard::simple(
        book_id,
        &format!("agent-page2-{suffix}"),
        "agent-page2@example.invalid",
    );

    let created1 = client
        .contact_create(&account_id, &card1)
        .expect("contact_create card1 failed");
    let id1 = created1.id.expect("server must return id for card1");

    let created2 = client
        .contact_create(&account_id, &card2)
        .expect("contact_create card2 failed");
    let id2 = created2.id.expect("server must return id for card2");

    let final_state = client
        .contact_get(&account_id, &[])
        .expect("contact_get failed")
        .state;

    // Page 1: request maxChanges: 1
    let changes_req1 = ChangesRequest {
        account_id: account_id.clone(),
        since_state: initial_state.clone(),
        max_changes: Some(1),
    };
    let page1_val = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
            "ContactCard/changes",
            &changes_req1,
        )
        .expect("ContactCard/changes page 1 failed");
    let page1: ChangesResponse =
        serde_json::from_value(page1_val).expect("ChangesResponse parse failed");

    assert!(
        page1.has_more_changes,
        "page 1 must report hasMoreChanges: true"
    );
    assert_eq!(
        page1.created.len(),
        1,
        "page 1 must return exactly 1 created id"
    );
    assert_ne!(page1.new_state, initial_state);
    assert_ne!(page1.new_state, final_state);

    // Page 2: resume from page 1's newState
    let changes_req2 = ChangesRequest {
        account_id: account_id.clone(),
        since_state: page1.new_state.clone(),
        max_changes: Some(1),
    };
    let page2_val = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
            "ContactCard/changes",
            &changes_req2,
        )
        .expect("ContactCard/changes page 2 failed");
    let page2: ChangesResponse =
        serde_json::from_value(page2_val).expect("ChangesResponse parse failed");

    assert!(
        !page2.has_more_changes,
        "page 2 must report hasMoreChanges: false"
    );
    assert_eq!(
        page2.created.len(),
        1,
        "page 2 must return exactly 1 created id"
    );
    assert_eq!(page2.new_state, final_state);

    let mut all_paged_ids = vec![page1.created[0].clone(), page2.created[0].clone()];
    all_paged_ids.sort();
    let mut expected_ids = vec![id1.clone(), id2.clone()];
    expected_ids.sort();
    assert_eq!(all_paged_ids, expected_ids);

    // High-level Client::all_changes follows pages and aggregates all created ids.
    let full_change_set = client
        .all_changes(&account_id, "ContactCard", &initial_state)
        .expect("Client::all_changes must succeed across pages");
    assert!(full_change_set.created.contains(&id1));
    assert!(full_change_set.created.contains(&id2));
    assert_eq!(full_change_set.new_state, final_state);

    // Clean up created cards.
    client
        .contact_destroy(&account_id, &id1)
        .expect("cleanup destroy id1 failed");
    client
        .contact_destroy(&account_id, &id2)
        .expect("cleanup destroy id2 failed");
}

/// Finding 12 (STALWART-RFC-FINDINGS.md): Unauthenticated GET to the session
/// resource on Stalwart v1.0.0 returns HTTP 200 OK with empty `accounts: {}`
/// and `username: ""` rather than HTTP 401 Unauthorized with a WWW-Authenticate
/// header. Verifies that `Client::connect` with `Credentials::none()` tolerates
/// the response, deserializes the session, and accurately reports `is_anonymous()`.
#[test]
#[ignore = "needs a running JMAP server (see docs/manual-test-live-server.md)"]
fn unauthenticated_session_discovery_reports_anonymous_through_the_real_api() {
    let origin = env::var("JMAP_LIVE_SERVER_URL").expect(
        "set JMAP_LIVE_SERVER_URL to the server's origin, e.g. https://jmap.example.com \
         (see docs/manual-test-live-server.md)",
    );
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::none())
        .expect("unauthenticated session discovery must succeed on live server");

    assert!(
        client.is_anonymous(),
        "unauthenticated session on Stalwart must report is_anonymous() == true"
    );
    let session = client.session();
    assert_eq!(session.username, "");
    assert!(session.accounts.is_empty());
    assert!(session.primary_accounts.is_empty());
    assert!(session.capabilities.contains_key(CAPABILITY_CORE));
    assert!(client.primary_account(CAPABILITY_MAIL).is_err());
}

/// Finding 16 (STALWART-RFC-FINDINGS.md): Mailbox/set create with whitespace-padded
/// name trims the stored name on Stalwart v1.0.0, but omits the name property from
/// the `created` map, returning only `{"id": "<id>"}`. Verifies that `Client::mailbox_create`
/// tolerates the omitted name, falls back to the requested name trimmed of whitespace,
/// and returns the expected name matching what is stored on the server.
#[test]
#[ignore = "needs a running JMAP server (see docs/manual-test-live-server.md)"]
fn mailbox_create_tolerates_server_omitted_trimmed_name_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping write test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("write account has a mail capability");

    let raw_name = format!("  agent-livewrite-pad-{}  ", unique_suffix());
    let mailbox = Mailbox {
        name: raw_name.clone(),
        ..Mailbox::default()
    };

    let created = client
        .mailbox_create(&account_id, &mailbox)
        .expect("Mailbox/set create failed against the real server");
    let id = created
        .id
        .clone()
        .expect("the server named the new mailbox");

    // Assert that the client layer returned the trimmed name:
    assert_eq!(
        created.name,
        raw_name.trim(),
        "created.name must report the trimmed name when the server omits it"
    );

    // Verify against the real server storage via Mailbox/get:
    let fetched = client
        .mailbox_get(&account_id)
        .expect("Mailbox/get failed")
        .list
        .into_iter()
        .find(|m| m.id.as_ref() == Some(&id))
        .expect("created mailbox found in Mailbox/get");
    assert_eq!(fetched.name, raw_name.trim());

    // Clean up created mailbox:
    client
        .mailbox_destroy(&account_id, &id)
        .expect("Mailbox/set destroy failed");
}

/// Finding 20 (STALWART-RFC-FINDINGS.md): CalendarEventNotification/set destroy
/// with an unformatted ID silently drops it from the response map on Stalwart v1.0.0
/// rather than returning a notFound SetError in notDestroyed (RFC 8620 Section 5.3).
/// Verifies that Client::calendar_event_notification_destroy tolerates the omitted ID
/// and reports notFound instead of failing with a protocol error.
#[test]
#[ignore = "needs a running JMAP server (see docs/manual-test-live-server.md)"]
fn calendar_event_notification_destroy_tolerates_server_dropped_id_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping write test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("write account has a calendars capability");

    // Formatted unknown ID returns notFound via notDestroyed:
    match client.calendar_event_notification_destroy(&account_id, &Id::new("n000000000000")) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
        }
        other => {
            panic!(
                "expected notFound SetError for formatted unknown notification destroy, got {other:?}"
            )
        }
    }

    // Unformatted ID dropped by Stalwart also returns notFound:
    match client.calendar_event_notification_destroy(&account_id, &Id::new("unformatted_id")) {
        Err(jmap_client::Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
        }
        other => {
            panic!("expected notFound SetError for unformatted notification destroy, got {other:?}")
        }
    }
}

/// Finding 19 (STALWART-RFC-FINDINGS.md): ParticipantIdentity/get response omits
/// mandatory state property required by RFC 8620 Section 5.1 and
/// draft-ietf-jmap-calendars Section 3.1.
/// Verifies that Client::participant_identities tolerates the omitted state field,
/// while confirming that raw wire responses omit state and fail strict GetResponse
/// deserialization.
#[test]
#[ignore = "needs a running JMAP server (see docs/manual-test-live-server.md)"]
fn participant_identity_get_tolerates_server_omitted_state_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping participant identity test"
        );
        return;
    };
    let Ok(account_id) = client.primary_account(CAPABILITY_CALENDARS) else {
        eprintln!("server names no primary account for {CAPABILITY_CALENDARS}; skipping");
        return;
    };

    // 1. Client::participant_identities succeeds against real Stalwart:
    let _identities = client
        .participant_identities(&account_id)
        .expect("Client::participant_identities must succeed against live Stalwart");

    // 2. Make raw ParticipantIdentity/get call to inspect the wire response directly:
    let raw_args = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CALENDARS],
            "ParticipantIdentity/get",
            &GetRequest::all(account_id.clone()),
        )
        .expect("raw ParticipantIdentity/get succeeds");

    // 3. Confirm that live Stalwart indeed omits the mandatory state field (Finding 19):
    assert!(
        raw_args.get("state").is_none(),
        "live Stalwart ParticipantIdentity/get response must omit the state field (Finding 19), got: {raw_args:?}"
    );
    assert!(
        raw_args.get("list").is_some(),
        "live Stalwart ParticipantIdentity/get response must contain list"
    );

    // 4. Demonstrate that standard RFC 8620 GetResponse<ParticipantIdentity> fails
    // deserialization due to the missing state field, proving the necessity of
    // the client-level tolerance:
    let strict_result: Result<GetResponse<ParticipantIdentity>, _> =
        serde_json::from_value(raw_args);
    match strict_result {
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("missing field") && msg.contains("state"),
                "expected missing field `state` error, got: {msg}"
            );
        }
        Ok(_) => panic!("expected strict GetResponse deserialization to fail on omitted state"),
    }
}

/// Finding 18 (STALWART-RFC-FINDINGS.md): Session capability currentUserPrincipalId
/// advertises the account ID instead of the principal ID, causing Principal/get to
/// report notFound.
/// Verifies that Client::current_user_principal and Client::current_user_principal_id
/// resolve the true principal object ("b") and principal identifier, and that
/// Client::principal_get tolerates the advertised account ID.
#[test]
#[ignore = "needs a running JMAP server (see docs/manual-test-live-server.md)"]
fn current_user_principal_resolves_past_account_id_mismatch_through_the_real_api() {
    let client = connect();
    let Ok(account_id) = client.primary_account(CAPABILITY_PRINCIPALS) else {
        eprintln!("server names no primary account for {CAPABILITY_PRINCIPALS}; skipping");
        return;
    };

    // 1. Session advertises currentUserPrincipalId as the account id:
    let advertised = client
        .session()
        .accounts
        .get(&account_id)
        .and_then(|acct| acct.account_capabilities.get(CAPABILITY_PRINCIPALS))
        .and_then(|cap| cap.get("currentUserPrincipalId"))
        .and_then(|v| v.as_str());
    assert_eq!(
        advertised,
        Some(account_id.as_str()),
        "Stalwart advertises account ID as currentUserPrincipalId (Finding 18)"
    );

    // 2. Direct raw Principal/get with the advertised ID reproduces the notFound response:
    let raw_args = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS],
            "Principal/get",
            &json!({ "accountId": account_id, "ids": [account_id.as_str()] }),
        )
        .expect("raw Principal/get call succeeds");
    let not_found = raw_args
        .get("notFound")
        .and_then(|v| v.as_array())
        .expect("notFound array present in response");
    assert!(
        not_found
            .iter()
            .any(|v| v.as_str() == Some(account_id.as_str())),
        "raw Principal/get with account ID must return notFound on live Stalwart"
    );

    // 3. Client::current_user_principal tolerates the mismatch and resolves the true principal:
    let principal = client
        .current_user_principal(&account_id)
        .expect("current_user_principal must succeed")
        .expect("owner principal must be resolved");
    assert!(principal.id.is_some(), "principal must have an ID");
    assert_ne!(
        principal.id.as_ref(),
        Some(&account_id),
        "resolved principal ID must be distinct from the account ID"
    );
    assert_eq!(principal.id.as_ref(), Some(&Id::new("b")));
    assert_eq!(principal.name, "admin@example.internal");
    assert_eq!(principal.email.as_deref(), Some("admin@example.internal"));

    // 4. Client::current_user_principal_id resolves the true principal ID:
    let principal_id = client
        .current_user_principal_id(&account_id)
        .expect("current_user_principal_id must succeed")
        .expect("owner principal ID must be resolved");
    assert_eq!(principal_id, Id::new("b"));

    // 5. Client::principal_get with the advertised account ID tolerates the mismatch:
    let list = client
        .principal_get(&account_id, std::slice::from_ref(&account_id))
        .expect("principal_get with advertised ID must succeed via tolerance fallback");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id.as_ref(), Some(&Id::new("b")));

    // 6. Client::principal_get with the real principal ID succeeds directly:
    let list_b = client
        .principal_get(&account_id, &[Id::new("b")])
        .expect("principal_get with real ID must succeed");
    assert_eq!(list_b.len(), 1);
    assert_eq!(list_b[0].id.as_ref(), Some(&Id::new("b")));
}

/// Finding 13 (STALWART-RFC-FINDINGS.md): CalendarEvent/set rejects the standard
/// JSCalendar timeZones property (RFC 8984 Section 4.7.2) with invalidProperties,
/// and CalendarEvent/parse converts custom VTIMEZONE definitions into iCalendar
/// convertedProperties rather than RFC 8984 timeZones.
/// Verifies that Client::event_create surfaces the invalidProperties SetError cleanly,
/// and that CalendarEvent/parse output is passed through intact to the client.
#[test]
#[ignore = "needs a running JMAP server (see docs/manual-test-live-server.md)"]
fn calendar_event_set_rejects_timezones_property_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("no live server configured or write credentials missing; skipping");
        return;
    };
    let Ok(account_id) = client.primary_account(CAPABILITY_CALENDARS) else {
        eprintln!("server names no primary account for {CAPABILITY_CALENDARS}; skipping");
        return;
    };

    // 1. Resolve a calendar to target:
    let calendar_id = client
        .calendars(&account_id)
        .expect("calendars call succeeds")
        .into_iter()
        .next()
        .expect("must find a calendar")
        .id
        .expect("calendar must have an ID");

    // 2. Direct raw CalendarEvent/set create with empty "timeZones": {} reproduces the
    // invalidProperties rejection on live Stalwart (Finding 13):
    let raw_args = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CALENDARS],
            "CalendarEvent/set",
            &json!({
                "accountId": account_id,
                "create": {
                    "probe_raw_tz": {
                        "calendarIds": { calendar_id.as_str(): true },
                        "title": "Probe Event Raw TZ",
                        "start": "2026-10-05T12:00:00",
                        "timeZones": {}
                    }
                }
            }),
        )
        .expect("raw CalendarEvent/set call succeeds on wire");
    let not_created = raw_args
        .get("notCreated")
        .and_then(|v| v.as_object())
        .expect("notCreated map present in response");
    let error_obj = not_created
        .get("probe_raw_tz")
        .expect("probe_raw_tz present in notCreated");
    assert_eq!(
        error_obj.get("type").and_then(|v| v.as_str()),
        Some("invalidProperties")
    );
    let props = error_obj
        .get("properties")
        .and_then(|v| v.as_array())
        .expect("properties array present");
    assert!(
        props.iter().any(|p| p.as_str() == Some("timeZones")),
        "properties array must name timeZones"
    );

    // 3. Client::event_create with time_zones set returns Error::Set cleanly:
    let mut event_with_tz = CalendarEvent::simple(
        calendar_id.clone(),
        "Probe Event With TimeZones",
        "2026-10-05T12:00:00",
        "PT1H",
    );
    let mut tz_map = BTreeMap::new();
    tz_map.insert(
        "/custom/zone1".to_string(),
        json!({
            "@type": "TimeZone",
            "timeZoneId": "/custom/zone1"
        }),
    );
    event_with_tz.time_zones = Some(tz_map);

    let set_err = client
        .event_create(&account_id, &event_with_tz)
        .expect_err("event_create with time_zones must fail on Stalwart");
    match set_err {
        Error::Set(err) => {
            assert_eq!(err.error_type, "invalidProperties");
            assert_eq!(err.description.as_deref(), Some("Invalid property."));
            assert_eq!(
                err.properties.as_ref(),
                Some(&vec!["timeZones".to_string()])
            );
        }
        other => panic!("expected Error::Set with invalidProperties, got {other:?}"),
    }

    // 4. Client::event_create without time_zones succeeds cleanly:
    let event_clean = CalendarEvent::simple(
        calendar_id,
        "Probe Event Without TimeZones",
        "2026-10-05T12:00:00",
        "PT1H",
    );
    let created = client
        .event_create(&account_id, &event_clean)
        .expect("event_create without time_zones must succeed on Stalwart");
    let created_id = created.id.expect("created event must have an ID");

    // Clean up created event:
    client
        .event_destroy(&account_id, &created_id)
        .expect("event_destroy succeeds");

    // 5. CalendarEvent/parse on an .ics blob with custom VTIMEZONE passes the server's
    // representation through intact to the client:
    let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:/custom/zone1\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19700101T000000\r\n\
TZOFFSETFROM:+0000\r\n\
TZOFFSETTO:+0000\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:test-tz-probe\r\n\
DTSTAMP:20261005T000000Z\r\n\
DTSTART;TZID=/custom/zone1:20261005T120000\r\n\
SUMMARY:Test Event Custom TZ\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let upload_res = client
        .blob_upload(
            &BlobUploadRequest::new(account_id.clone())
                .create_blob("b0", UploadBlob::from_text(ics, "text/calendar")),
        )
        .expect("blob_upload succeeds")
        .created
        .expect("blob created map present");
    let blob_id = upload_res.get("b0").expect("b0 created").id.clone();

    let parse_req = CalendarEventParseRequest::new(account_id, vec![blob_id.clone()]);
    let parse_res = client
        .event_parse(&parse_req)
        .expect("event_parse succeeds on Stalwart");
    let parsed_map = parse_res
        .parsed
        .expect("parsed map must be present in response");
    let parsed_ev = parsed_map
        .get(&blob_id)
        .expect("parsed event must be present for blob ID");
    assert_eq!(parsed_ev.title.as_deref(), Some("Test Event Custom TZ"));
    assert_eq!(parsed_ev.start.as_deref(), Some("2026-10-05T12:00:00"));
    assert_eq!(parsed_ev.uid.as_deref(), Some("test-tz-probe"));
    assert!(parsed_ev.time_zones.is_none());
    assert!(parsed_ev.extra.contains_key("iCalendar"));
}

/// `ContactCard/set` create with `onlineServices` lacking a `uri`, and subsequent
/// `ContactCard/get` against a real server (Finding 14).
/// Stalwart accepts the creation, but silently discards the `onlineServices` entry on storage
/// because it has no `uri` (RFC 9553 Section 2.3.2).
/// Verifies that `Client::contact_create` and `Client::contact_get` pass the server's reply
/// through intact without error.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn contact_card_online_service_without_uri_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping online_service test");
        return;
    };
    let Ok(account_id) = client.primary_account(CAPABILITY_CONTACTS) else {
        eprintln!("server names no primary account for {CAPABILITY_CONTACTS}; skipping");
        return;
    };

    let books = client
        .address_books(&account_id)
        .expect("AddressBook/get failed against the real server");
    let book_id = books
        .iter()
        .find(|b| b.is_default == Some(true))
        .or_else(|| books.first())
        .and_then(|b| b.id.clone())
        .expect("no address book available for contact card create");

    let suffix = unique_suffix();
    let full_name = format!("Alice Probe-{}", suffix);
    let mut card = ContactCard::simple(book_id, &full_name, "alice@example.org");
    let mut online_services = BTreeMap::new();
    online_services.insert(
        "s1".to_string(),
        OnlineService {
            service: Some("Jabber".to_string()),
            user: Some(format!("alice-{}@example.org", suffix)),
            uri: None,
            ..Default::default()
        },
    );
    card.online_services = Some(online_services);

    // 1. Create succeeds on Stalwart:
    let created = client
        .contact_create(&account_id, &card)
        .expect("contact_create with onlineServices lacking uri must succeed on Stalwart");
    let card_id = created.id.expect("created card must have an id");

    // 2. Fetch the card back: Stalwart accepted the create, but discarded onlineServices on storage (Finding 14):
    let fetched = client
        .contact_get(&account_id, std::slice::from_ref(&card_id))
        .expect("contact_get must succeed on Stalwart");
    assert_eq!(fetched.list.len(), 1);
    let fetched_card = &fetched.list[0];
    assert_eq!(
        fetched_card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some(full_name.as_str())
    );
    assert!(
        fetched_card.online_services.is_none(),
        "Stalwart v1.0.0 discards onlineServices entries lacking a uri upon storage (Finding 14)"
    );

    // 3. Clean up created contact:
    client
        .contact_destroy(&account_id, &card_id)
        .expect("contact_destroy must succeed on Stalwart");
}

/// `ContactCard/parse` with `EMAIL;TYPE=home,internet:...` against a real server (Finding 15).
/// Stalwart promotes `TYPE=internet` to an unrecognized context (`"internet": true`) in `contexts`
/// (RFC 9553 Section 1.7.3 / Section 2.2.3).
/// Verifies that `Client::contact_card_parse` tolerates the non-standard context and passes
/// the parsed card through intact to the client.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn contact_card_parse_promotes_type_internet_to_context_through_the_real_api() {
    let Some(client) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping contact_card_parse test"
        );
        return;
    };
    let Ok(account_id) = client.primary_account(CAPABILITY_CONTACTS) else {
        eprintln!("server names no primary account for {CAPABILITY_CONTACTS}; skipping");
        return;
    };
    if !client
        .session()
        .capabilities
        .contains_key(CAPABILITY_CONTACTS_PARSE)
    {
        eprintln!("server does not advertise {CAPABILITY_CONTACTS_PARSE}; skipping");
        return;
    }

    let suffix = unique_suffix();
    let full_name = format!("Probe Card-{}", suffix);
    let email_addr = format!("user-{}@example.com", suffix);
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:4.0\r\n\
         FN:{full_name}\r\n\
         EMAIL;TYPE=home,internet:{email_addr}\r\n\
         END:VCARD\r\n"
    );

    let upload = client
        .upload_blob(&account_id, "text/vcard", vcard.into_bytes())
        .expect("blob upload failed against the real server");

    let response = client
        .contact_card_parse(&ContactCardParseRequest::new(
            account_id.clone(),
            [upload.blob_id.clone()],
        ))
        .expect("ContactCard/parse failed against the real server");

    let parsed_map = response
        .parsed
        .expect("ContactCard/parse response missing parsed map");
    let parsed_card = parsed_map
        .get(&upload.blob_id)
        .expect("uploaded blobId missing from parsed map");

    assert_eq!(
        parsed_card.name.as_ref().and_then(|n| n.full.as_deref()),
        Some(full_name.as_str())
    );

    let emails = parsed_card
        .emails
        .as_ref()
        .expect("parsed card missing emails map");
    let email = emails
        .values()
        .find(|e| e.address == email_addr)
        .expect("expected email address in parsed card");

    // Finding 15: Stalwart promotes TYPE=internet to an unrecognized context key:
    let contexts = email
        .contexts
        .as_ref()
        .expect("contexts map must be present");
    assert_eq!(
        contexts.get("private"),
        Some(&serde_json::Value::Bool(true)),
        "private context should be present"
    );
    assert_eq!(
        contexts.get("internet"),
        Some(&serde_json::Value::Bool(true)),
        "Stalwart v1.0.0 promotes TYPE=internet to context key 'internet' (Finding 15)"
    );
}
