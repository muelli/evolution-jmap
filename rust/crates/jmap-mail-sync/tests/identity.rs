// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which of the account's identities a message goes out through, against a
//! live mock server.
//!
//! [`Outgoing`](jmap_mail_sync::Outgoing) takes an identity id and nothing in
//! the provider picked one: a transport is handed an address by Camel and the
//! server takes an id, and this is the lookup between them. It is its own step
//! rather than part of sending because getting it wrong is not a failed send —
//! RFC 8621 §7 has the server check the message's `From` against the identity,
//! so the worst case is a refusal, and the case worth testing is the account
//! that has several addresses and the message that has to leave through the
//! right one.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, SyncError};
use jmap_mock::MockServer;
use jmap_proto::Id;

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    fn seed_identity(&self, name: &str, email: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        account.seed_identity(name, email)
    }
}

/// The failure, or a panic naming what came back instead.
fn refusal(result: Result<Id, SyncError>) -> String {
    match result {
        Err(SyncError::NoIdentity(address)) => address,
        Err(other) => panic!("expected no identity, got {other:?}"),
        Ok(id) => panic!("expected no identity, got {id}"),
    }
}

#[test]
fn the_address_the_message_is_sent_from_picks_the_identity_that_has_it() {
    let fixture = Fixture::start();
    let work = fixture.seed_identity("Alice at work", "alice@example.com");
    let home = fixture.seed_identity("Alice at home", "alice@example.net");

    // An account with two addresses is the ordinary case this exists for: the
    // user chose one in the composer, Camel hands it to the transport as the
    // envelope sender, and the submission has to name the identity that owns it
    // — not simply the account's first.
    assert_eq!(
        fixture.sync().identity_for("alice@example.net").unwrap(),
        home
    );
    assert_eq!(
        fixture.sync().identity_for("alice@example.com").unwrap(),
        work
    );
}

#[test]
fn an_address_the_account_has_no_identity_for_is_refused() {
    let fixture = Fixture::start();
    fixture.seed_identity("Alice", "alice@example.com");

    // Its own failure rather than a client error: nothing is wrong with the
    // account or the connection, and the sentence the user needs names the
    // address they tried to send as.
    assert_eq!(
        refusal(fixture.sync().identity_for("bob@example.com")),
        "bob@example.com"
    );
}

#[test]
fn an_account_with_no_identities_at_all_is_refused() {
    let fixture = Fixture::start();

    assert_eq!(
        refusal(fixture.sync().identity_for("alice@example.com")),
        "alice@example.com"
    );
}

#[test]
fn a_wildcard_identity_covers_any_address_in_its_domain() {
    let fixture = Fixture::start();
    let any = fixture.seed_identity("Example", "*@example.com");

    // RFC 8621 §6: a local part of the single character `*` means the identity
    // may be used with any address in that domain. Servers that host a domain
    // publish exactly one identity like this, and a client that compared the
    // whole string would tell such an account it cannot send at all.
    assert_eq!(
        fixture.sync().identity_for("alice@example.com").unwrap(),
        any
    );
    assert_eq!(fixture.sync().identity_for("bob@example.com").unwrap(), any);
}

#[test]
fn a_wildcard_identity_covers_only_its_own_domain() {
    let fixture = Fixture::start();
    fixture.seed_identity("Example", "*@example.com");

    assert_eq!(
        refusal(fixture.sync().identity_for("alice@example.net")),
        "alice@example.net"
    );
    // And a domain that merely ends with it is a different domain — the whole
    // label has to match, or an account that may send as `example.com` would be
    // told it may send as `notexample.com` too.
    assert_eq!(
        refusal(fixture.sync().identity_for("alice@notexample.com")),
        "alice@notexample.com"
    );
}

#[test]
fn an_identity_whose_local_part_only_begins_with_a_star_is_not_a_wildcard() {
    let fixture = Fixture::start();
    fixture.seed_identity("Starred", "*alice@example.com");

    // The RFC's wildcard is the local part being the single character `*`, and
    // nothing else. `*alice@example.com` is an address with an unusual name in
    // it; reading it as a wildcard would send Bob's mail through Alice's
    // identity.
    assert_eq!(
        refusal(fixture.sync().identity_for("bob@example.com")),
        "bob@example.com"
    );
}

#[test]
fn an_identity_that_has_the_address_outright_beats_one_that_matches_by_domain() {
    let fixture = Fixture::start();
    // Seeded wildcard first, so a lookup that took the first match would take
    // the wrong one.
    fixture.seed_identity("Example", "*@example.com");
    let alice = fixture.seed_identity("Alice", "alice@example.com");

    // The exact identity is the one the user configured — it carries their name
    // and their signature, and the server writes its `From` from it. The
    // wildcard is the account's fallback for everything else.
    assert_eq!(
        fixture.sync().identity_for("alice@example.com").unwrap(),
        alice
    );
}

#[test]
fn the_first_of_two_identities_with_the_same_address_wins() {
    let fixture = Fixture::start();
    let first = fixture.seed_identity("Alice", "alice@example.com");
    fixture.seed_identity("Alice, again", "alice@example.com");

    // Nothing distinguishes them from here, and the answer still has to be the
    // same one every time: a send that picked a different identity on each
    // attempt would put a different signature on retries of one message.
    assert_eq!(
        fixture.sync().identity_for("alice@example.com").unwrap(),
        first
    );
}

#[test]
fn an_address_that_differs_only_in_case_is_the_same_address() {
    let fixture = Fixture::start();
    let alice = fixture.seed_identity("Alice", "Alice@Example.COM");
    let any = fixture.seed_identity("Example", "*@EXAMPLE.com");

    // The domain is case-insensitive by DNS, and the local part is compared
    // that way too: both spellings are the user's own address, on their own
    // account, and refusing to send because the server wrote one of them with a
    // capital would be a failure with nothing behind it. The server checks the
    // `From` against the identity either way, so a permissive match here can
    // only ever produce a refusal — never mail sent as somebody else.
    assert_eq!(
        fixture.sync().identity_for("alice@example.com").unwrap(),
        alice
    );
    assert_eq!(fixture.sync().identity_for("BOB@example.com").unwrap(), any);
}
