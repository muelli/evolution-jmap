// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Quota (RFC 9425): capability detection and `Quota/get`. No Evolution
//! UI or EDS surface consumes this yet; the wiring is a separate increment.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::quota::{
    Quota, QuotaQueryFilter, quota_data_type, quota_resource_type, quota_scope,
};
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
    assert!(quota.hard_limit > 0);
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

/// `Quota/query` with no filter returns every quota, the same as `Quota/get`
/// with `ids: null` (RFC 9425 §4.4: zero conditions match everything).
#[test]
fn quota_query_with_no_filter_returns_every_quota() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let ids = client
        .quota_query(&account_id, QuotaQueryFilter::new())
        .unwrap();
    assert_eq!(ids, vec![jmap_proto::Id::from("Q1")]);
}

/// A `scope` condition is an exact match (RFC 9425 §4.4): a query for
/// `domain` scope finds nothing among account-scoped fixtures.
#[test]
fn quota_query_filters_by_exact_scope() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let ids = client
        .quota_query(
            &account_id,
            QuotaQueryFilter::new().with_scope(quota_scope::DOMAIN),
        )
        .unwrap();
    assert!(ids.is_empty());

    let ids = client
        .quota_query(
            &account_id,
            QuotaQueryFilter::new().with_scope(quota_scope::ACCOUNT),
        )
        .unwrap();
    assert_eq!(ids, vec![jmap_proto::Id::from("Q1")]);
}

/// A `type` condition tests membership in the Quota's own `types` list
/// (RFC 9425 §4.4), not exact equality against the whole list.
#[test]
fn quota_query_filters_by_type_membership() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.quotas.seed_with_id(
            jmap_proto::Id::from("Q2"),
            Quota::new(
                "Q2",
                "Contacts",
                quota_resource_type::OCTETS,
                0,
                1_000,
                quota_scope::ACCOUNT,
                [quota_data_type::CONTACTS],
            ),
        );
    }
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let ids = client
        .quota_query(
            &account_id,
            QuotaQueryFilter::new().with_type(quota_data_type::CONTACTS),
        )
        .unwrap();
    assert_eq!(ids, vec![jmap_proto::Id::from("Q2")]);
}

/// Same capability gate as `Quota/get`: a server that never advertises quota
/// is not sent a `Quota/query` either.
#[test]
fn quota_query_is_not_sent_when_the_account_does_not_advertise_it() {
    let server = MockServer::builder()
        .without_capability(CAPABILITY_QUOTA)
        .start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    assert_eq!(
        client
            .quota_query(&account_id, QuotaQueryFilter::new())
            .unwrap(),
        Vec::new()
    );
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
