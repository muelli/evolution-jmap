// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal lookup against the mock server (RFC 9670) — the shared floor
//! for scheduling and per-source sharing. See docs/PRINCIPALS-DESIGN.md.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::principals::{Principal, PrincipalQueryFilter};

fn server_with_principals() -> (MockServer, Id, Id, Id) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let (me, attendee) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let me = account.seed_current_user_principal(Principal {
            principal_type: Some("individual".to_owned()),
            name: "Alice Example".to_owned(),
            email: Some("alice@example.com".to_owned()),
            ..Principal::default()
        });
        let attendee = account.seed_principal(Principal {
            principal_type: Some("individual".to_owned()),
            name: "Bob Example".to_owned(),
            email: Some("bob@example.com".to_owned()),
            ..Principal::default()
        });
        (me, attendee)
    };
    (server, account_id, me, attendee)
}

#[test]
fn principals_lists_every_seeded_principal() {
    let (server, account_id, me, attendee) = server_with_principals();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let mut principals = client.principals(&account_id).unwrap();
    principals.sort_by(|a, b| a.id.cmp(&b.id));

    assert_eq!(principals.len(), 2);
    assert_eq!(principals[0].id.as_ref(), Some(&me));
    assert_eq!(principals[0].email.as_deref(), Some("alice@example.com"));
    assert_eq!(principals[1].id.as_ref(), Some(&attendee));
    assert_eq!(principals[1].email.as_deref(), Some("bob@example.com"));
}

#[test]
fn principal_query_resolves_an_attendee_by_email() {
    let (server, account_id, _me, attendee) = server_with_principals();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let ids = client
        .principal_query(&account_id, PrincipalQueryFilter::email("bob@example.com"))
        .unwrap();

    assert_eq!(ids, vec![attendee]);
}

#[test]
fn principal_query_finds_nothing_for_an_unknown_email() {
    let (server, account_id, ..) = server_with_principals();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let ids = client
        .principal_query(
            &account_id,
            PrincipalQueryFilter::email("nobody@example.com"),
        )
        .unwrap();

    assert!(ids.is_empty());
}

#[test]
fn session_names_the_current_user_principal() {
    let (server, account_id, me, _attendee) = server_with_principals();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let session = client.session();
    let capability = session
        .accounts
        .get(&account_id)
        .unwrap()
        .account_capabilities
        .get("urn:ietf:params:jmap:principals")
        .expect("server advertises the principals account capability");
    assert_eq!(capability["currentUserPrincipalId"], me.as_str());
}
