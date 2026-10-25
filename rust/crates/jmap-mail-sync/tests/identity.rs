// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The selection of the identity for an outgoing message, against a mock
//! server.
//!
//! [`Outgoing`](jmap_mail_sync::Outgoing) takes an identity id. Camel gives the
//! transport an address, and the server needs an id: this lookup connects the
//! two. RFC 8621 §7 makes the server check `From` against the identity, so a
//! wrong match causes a refusal. The important case is an account with
//! several addresses, where the message must use the correct one.

use jmap_mail_sync::SyncError;
use jmap_proto::Id;
use jmap_proto::mail::Identity;

mod common;
use common::Fixture;

impl Fixture {
    /// Seed an identity without an `id`, which violates RFC 8620 §5.1 in the
    /// `Identity/get` answer.
    fn seed_identity_without_id(&self, email: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        let id = account.identities.alloc_id();
        let mut identity = Identity::default();
        identity.email = email.to_owned();
        account.identities.seed_with_id(id.clone(), identity);
        id
    }
}

/// The refused address. Panics with the result if it is not a refusal.
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

    // The usual case: the user selects an address in the composer, and Camel
    // gives it to the transport as the envelope sender. The submission must
    // name the identity for that address, not the first identity.
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

    // A specific error, not a client error: the account and the connection are
    // correct. The message for the user must name the address.
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

    // RFC 8621 §6: a local part of only `*` permits any address in that domain.
    // Servers that host a domain often publish one such identity. A comparison
    // of the full string prevents all sends from such an account.
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
    // A domain that only ends with the same text is a different domain. The
    // full label must match, else `example.com` also permits `notexample.com`.
    assert_eq!(
        refusal(fixture.sync().identity_for("alice@notexample.com")),
        "alice@notexample.com"
    );
}

#[test]
fn an_identity_whose_local_part_only_begins_with_a_star_is_not_a_wildcard() {
    let fixture = Fixture::start();
    fixture.seed_identity("Starred", "*alice@example.com");

    // The RFC wildcard is a local part of only the character `*`.
    // `*alice@example.com` is an address with an unusual local part. As a
    // wildcard, it sends the mail of Bob through the identity of Alice.
    assert_eq!(
        refusal(fixture.sync().identity_for("bob@example.com")),
        "bob@example.com"
    );
}

#[test]
fn an_identity_that_has_the_address_outright_beats_one_that_matches_by_domain() {
    let fixture = Fixture::start();
    // Seed the wildcard first, so that a first-match lookup gets the wrong one.
    fixture.seed_identity("Example", "*@example.com");
    let alice = fixture.seed_identity("Alice", "alice@example.com");

    // The user configured the exact identity: it has the name and the
    // signature, and the server writes `From` from it. The wildcard is the
    // fallback for all other addresses.
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

    // The two are equal here, but the result must always be the same. Else,
    // each retry of one message can get a different signature.
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

    // DNS makes the domain case-insensitive, and the local part is compared the
    // same way. Both spellings are the address of the user, so a refusal
    // because of a capital letter has no cause. The server always checks
    // `From` against the identity. Thus a permissive match can only cause a
    // refusal, never mail sent as a different person.
    assert_eq!(
        fixture.sync().identity_for("alice@example.com").unwrap(),
        alice
    );
    assert_eq!(fixture.sync().identity_for("BOB@example.com").unwrap(), any);
}

#[test]
fn a_matched_identity_with_no_id_is_a_protocol_violation() {
    let fixture = Fixture::start();
    fixture.seed_identity_without_id("alice@example.com");

    let error = fixture
        .sync()
        .identity_for("alice@example.com")
        .expect_err("an identity with no id cannot be named in a submission");

    match error {
        SyncError::Client(jmap_client::Error::Protocol(message)) => {
            assert!(message.contains("without an id"), "{message}");
        }
        other => panic!("expected a protocol failure, got {other:?}"),
    }
}
