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
//! $ cargo test -p evolution-jmap-client --features live-server -- --ignored
//! ```
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
//! Write anything. A real account may be someone's actual mailbox — even the
//! disposable Stalwart VM is meant to be reused across runs rather than
//! reseeded — so every test here is read-only: session discovery, `Core/echo`,
//! and listing what already exists. `Mailbox/set` round-trips (create, rename,
//! destroy) are covered against the mock, where they cost nothing.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_proto::session::{
    CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
};
use serde_json::json;

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

    Client::connect(&origin, credentials)
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
/// does not offer is as much a part of it as reading what it does.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn mail_capable_accounts_list_a_non_empty_mailbox_set() {
    let client = connect();
    let Some(account_id) = client.session().primary_account(CAPABILITY_MAIL).cloned() else {
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
    let Some(account_id) = client
        .session()
        .primary_account(CAPABILITY_CONTACTS)
        .cloned()
    else {
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
    let Some(account_id) = client
        .session()
        .primary_account(CAPABILITY_CALENDARS)
        .cloned()
    else {
        eprintln!("server names no primary account for {CAPABILITY_CALENDARS}; skipping");
        return;
    };

    client.calendars(&account_id).unwrap();
}
