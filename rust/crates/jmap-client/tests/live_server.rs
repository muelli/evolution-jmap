// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Driving a real JMAP server end to end — the recipe `docs/ROADMAP.md`'s
//! "Integration testing (parallel track)" asks for, and the real-server half
//! of the current priority's "real-server readiness" item.
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
//! (`infra/gcp/create-stalwart.sh`).
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

use std::env;

use jmap_client::{Client, Credentials};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::contacts::ContactCard;
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailImport, EmailQueryFilter, Mailbox,
    keyword, role,
};
use jmap_proto::session::{
    CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
};
use serde_json::json;

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

/// Credentials for the one test in this file that writes, kept deliberately
/// separate from [`connect`]'s: those are handed to every read-only test, so
/// whatever account someone points them at (their own mailbox, an
/// operator's shared test login) must never be the one a `Mailbox/set`
/// lands on. This account exists only if `JMAP_LIVE_SERVER_WRITE_USER`/
/// `_PASSWORD` are set to a login seeded for exactly this purpose — see
/// `docs/manual-test-live-server.md`'s "write-path test" section for the
/// `infra/stalwart/stw seed` recipe. Absent, not just empty, so the base
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
/// seed it (`infra/stalwart/stw seed`, same domain, a different local part).
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
/// (`lib.rs:153`), and a real server's state tokens and
/// created/updated/destroyed classification for this type had no
/// live-server coverage until now.
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
    let card = ContactCard::simple(book_id, &full_name, "agent-livewrite@example.invalid");

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
    let event = CalendarEvent::simple(calendar_id, &title, "2026-08-18T13:00:00", "PT1H");

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
        .email_import(&account_id, &EmailImport::new(upload.blob_id, inbox_id))
        .expect("Email/import failed against the real server");
    let id = imported.id.clone().expect("the server named the new email");

    let changes_after_import = client
        .all_changes(&account_id, "Email", &state_before_import)
        .expect("Email/changes failed against the real server");
    assert!(
        changes_after_import.created.contains(&id),
        "Email/changes since before the import does not list the new message as created"
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
