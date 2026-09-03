// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Quota (RFC 9425): capability detection and `Quota/get`. No Evolution
//! UI or EDS surface consumes this yet; the wiring is a separate increment.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_QUOTA};

/// A server advertises `urn:ietf:params:jmap:quota` both at session level and
/// on the account, the same way every other capability does.
#[test]
fn quota_capability_is_advertised_and_resolves_to_the_account() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    assert!(client.session().quota_capability().is_some());
    assert_eq!(
        client.session().resolve_primary_account(CAPABILITY_QUOTA),
        Some(&account_id)
    );
}

/// A freshly started account already has a quota fixture (RFC 9425's `Quota`
/// objects are server-computed, never client-created), so `ids: null`
/// returns at least one object with sane RFC 8621 §2.1 fields.
#[test]
fn quota_get_returns_the_seeded_fixture() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let quotas = client.quotas(&account_id).unwrap();
    assert_eq!(quotas.len(), 1);
    let quota = &quotas[0];
    assert_eq!(quota.resource_type, "octets");
    assert_eq!(quota.scope, "account");
    assert!(quota.limit > 0);
    assert_eq!(quota.used, 0);
}

/// A server that never advertises `urn:ietf:params:jmap:quota` on the
/// account (Fastmail, in practice) must not be sent a `Quota/get` at all:
/// naming an unadvertised capability in `using` fails the whole request
/// (RFC 8620 §3.6.1's `unknownCapability`). `Client::quotas` answers an
/// empty list instead, which the `jmap-mail` caller already treats as "no
/// quota to report" for a folder that has none.
#[test]
fn quota_get_is_not_sent_when_the_account_does_not_advertise_it() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_QUOTA)
        .start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    assert!(client.session().quota_capability().is_none());
    assert_eq!(client.quotas(&account_id).unwrap(), Vec::new());
}

/// `Quota/get` with an id that names nothing answers `notFound`, the same as
/// every other `/get` method (RFC 8620 §5.1).
#[test]
fn quota_get_reports_unknown_ids_as_not_found() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let arguments = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_QUOTA],
            "Quota/get",
            &jmap_proto::methods::GetRequest::ids(account_id.clone(), ["nonexistent"]),
        )
        .unwrap();
    let response: jmap_proto::methods::GetResponse<jmap_proto::quota::Quota> =
        serde_json::from_value(arguments).unwrap();
    assert!(response.list.is_empty());
    assert_eq!(
        response.not_found,
        vec![jmap_proto::Id::from("nonexistent")]
    );
}
