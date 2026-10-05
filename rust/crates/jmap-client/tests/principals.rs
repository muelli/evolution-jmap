// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal lookup against the mock server (RFC 9670) — the shared floor
//! for scheduling and per-source sharing. See docs/PRINCIPALS-DESIGN.md.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::principals::{Principal, PrincipalQueryFilter};
use serde_json::json;

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
fn get_availability_returns_busy_periods_in_the_window_sorted_by_start() {
    let (server, account_id, me, _attendee) = server_with_principals();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let calendar = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Personal", true)
    };
    let later = CalendarEvent::simple(calendar.clone(), "Later", "2026-09-01T15:00:00", "PT30M");
    let earlier = CalendarEvent::simple(calendar.clone(), "Earlier", "2026-09-01T09:00:00", "PT1H");
    let outside_window =
        CalendarEvent::simple(calendar.clone(), "Next week", "2026-09-08T09:00:00", "PT1H");
    let free = CalendarEvent {
        free_busy_status: Some("free".to_owned()),
        ..CalendarEvent::simple(calendar, "Optional", "2026-09-01T12:00:00", "PT1H")
    };
    for event in [&later, &earlier, &outside_window, &free] {
        client.event_create(&account_id, event).unwrap();
    }

    let busy = client
        .get_availability(
            &account_id,
            &me,
            "2026-09-01T00:00:00Z",
            "2026-09-02T00:00:00Z",
            false,
        )
        .unwrap();

    assert_eq!(busy.len(), 2, "expected only the two in-window busy events");
    assert_eq!(busy[0].utc_start.as_str(), "2026-09-01T09:00:00Z");
    assert_eq!(busy[0].utc_end.as_str(), "2026-09-01T10:00:00Z");
    assert_eq!(busy[0].busy_status, "confirmed");
    assert!(busy[0].event.is_none(), "showDetails was false");
    assert_eq!(busy[1].utc_start.as_str(), "2026-09-01T15:00:00Z");
    assert_eq!(busy[1].utc_end.as_str(), "2026-09-01T15:30:00Z");
}

#[test]
fn get_availability_includes_the_event_when_show_details_is_true() {
    let (server, account_id, me, _attendee) = server_with_principals();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let calendar = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Personal", true)
    };
    let event = CalendarEvent::simple(calendar, "Dentist", "2026-09-01T09:00:00", "PT1H");
    client.event_create(&account_id, &event).unwrap();

    let busy = client
        .get_availability(
            &account_id,
            &me,
            "2026-09-01T00:00:00Z",
            "2026-09-02T00:00:00Z",
            true,
        )
        .unwrap();

    assert_eq!(busy.len(), 1);
    let details = busy[0].event.as_ref().expect("showDetails was true");
    assert_eq!(details.title.as_deref(), Some("Dentist"));
}

#[test]
fn get_availability_is_not_found_when_the_principal_denies_it() {
    let (server, account_id, _me, _attendee) = server_with_principals();
    let denied = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_principal(Principal {
                principal_type: Some("individual".to_owned()),
                name: "Carol NoAvailability".to_owned(),
                capabilities: [(
                    "urn:ietf:params:jmap:calendars".to_owned(),
                    json!({"mayGetAvailability": false}),
                )]
                .into_iter()
                .collect(),
                ..Principal::default()
            })
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let error = client
        .get_availability(
            &account_id,
            &denied,
            "2026-09-01T00:00:00Z",
            "2026-09-02T00:00:00Z",
            false,
        )
        .unwrap_err();

    match error {
        jmap_client::Error::Method(method_error) => {
            assert_eq!(method_error.error_type, "notFound");
        }
        other => panic!("expected a notFound method error, got {other:?}"),
    }
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

#[test]
fn get_availability_reports_busy_period_for_event_with_participants() {
    let (server, account_id, me, _attendee) = server_with_principals();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let calendar = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Work", true)
    };

    let mut participant = jmap_proto::calendars::Participant::new("Bob Example", "bob@example.com");
    participant.participation_status = Some("needs-action".to_owned());
    participant.roles = Some([("attendee".to_owned(), true)].into());

    let mut event = CalendarEvent::simple(calendar, "Team Sync", "2026-09-01T10:00:00", "PT1H");
    event.participants = Some(
        [(
            "bob".to_owned(),
            serde_json::to_value(&participant).unwrap(),
        )]
        .into(),
    );
    let created = client.event_create(&account_id, &event).unwrap();
    let event_id = created.id.unwrap();

    let busy = client
        .get_availability(
            &account_id,
            &me,
            "2026-09-01T00:00:00Z",
            "2026-09-02T00:00:00Z",
            false,
        )
        .unwrap();

    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0].utc_start.as_str(), "2026-09-01T10:00:00Z");
    assert_eq!(busy[0].utc_end.as_str(), "2026-09-01T11:00:00Z");
    assert_eq!(busy[0].busy_status, "confirmed");

    client.event_destroy(&account_id, &event_id).unwrap();

    let busy_after = client
        .get_availability(
            &account_id,
            &me,
            "2026-09-01T00:00:00Z",
            "2026-09-02T00:00:00Z",
            false,
        )
        .unwrap();
    assert!(busy_after.is_empty());
}

#[test]
fn current_user_principal_and_principal_get_fetch_seeded_principal() {
    let (server, account_id, me, attendee) = server_with_principals();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let principal = client
        .current_user_principal(&account_id)
        .unwrap()
        .expect("current_user_principal must be found");
    assert_eq!(principal.id.as_ref(), Some(&me));
    assert_eq!(principal.name, "Alice Example");
    assert_eq!(principal.email.as_deref(), Some("alice@example.com"));

    let principal_id = client
        .current_user_principal_id(&account_id)
        .unwrap()
        .expect("current_user_principal_id must resolve");
    assert_eq!(principal_id, me);

    let get_list = client
        .principal_get(&account_id, &[me.clone(), attendee.clone()])
        .unwrap();
    assert_eq!(get_list.len(), 2);
    assert!(get_list.iter().any(|p| p.id.as_ref() == Some(&me)));
    assert!(get_list.iter().any(|p| p.id.as_ref() == Some(&attendee)));
}

/// `Principal/query`'s "text"/"email" filters match by substring
/// containment (RFC 9670 Section 2.4.1), so a server with two principals
/// that both happen to contain the account's own username as a substring
/// can answer with the wrong one first. `current_user_principal` (used when
/// the session omits `currentUserPrincipalId` entirely, the same fallback
/// Finding 18's account-id-mismatch shape also lands in) must not accept
/// whichever principal the query returns first; it must pick the one that
/// actually identifies the account owner.
#[test]
fn current_user_principal_does_not_accept_an_unverified_decoy_candidate() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let real_principal_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.name = "alice".to_owned();
        // Seeded first, so it sorts before the real principal below — a
        // "take whichever Principal/get answers first" bug would return
        // this one. Its email contains "alice", matching the substring
        // filter, but it is not the account owner.
        account.seed_principal(Principal {
            principal_type: Some("individual".to_owned()),
            name: "Alice Decoy Team".to_owned(),
            email: Some("alice.decoy@example.com".to_owned()),
            ..Principal::default()
        });
        account.seed_principal(Principal {
            principal_type: Some("individual".to_owned()),
            name: "Alice Example".to_owned(),
            email: Some("alice@example.com".to_owned()),
            ..Principal::default()
        })
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let principal = client
        .current_user_principal(&account_id)
        .unwrap()
        .expect("must resolve to the account owner's own principal");
    assert_eq!(
        principal.id.as_ref(),
        Some(&real_principal_id),
        "must not resolve to a principal that merely shares a substring with the account's own name"
    );
}
