// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::{AddressBook, AddressBookRights};
use jmap_proto::methods::{Comparator, QueryRequest, QueryResponse, SetRequest};
use jmap_proto::principals::{
    Principal, PrincipalQueryFilter, ShareNotification, ShareNotificationQueryFilter,
};
use jmap_proto::state::UtcDate;
use serde_json::json;

#[test]
fn principal_set_allows_self_update_and_rejects_create_destroy() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let principal_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let acct = state.account_mut(&account_id).unwrap();
        let mut p = Principal::new("Owner");
        p.email = Some("owner@example.com".to_string());
        acct.seed_current_user_principal(p)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // Self-update name, description, and timeZone
    let mut update_req = SetRequest::<Principal>::new(account_id.clone());
    update_req.update = Some(
        [(
            principal_id.clone(),
            json!({
                "name": "Updated Name",
                "description": "Updated Description",
                "timeZone": "Europe/Paris"
            }),
        )]
        .into_iter()
        .collect(),
    );

    let res = client.principal_set(&update_req).unwrap();
    assert!(res.updated.is_some_and(|u| u.contains_key(&principal_id)));

    // Verify updated principal
    let fetched = client
        .principal_get(&account_id, std::slice::from_ref(&principal_id))
        .unwrap();
    assert_eq!(fetched[0].name, "Updated Name");
    assert_eq!(
        fetched[0].description.as_deref(),
        Some("Updated Description")
    );
    assert_eq!(fetched[0].time_zone.as_deref(), Some("Europe/Paris"));

    // Create is rejected
    let mut create_req = SetRequest::<Principal>::new(account_id.clone());
    create_req.create = Some(
        [("c1".to_string(), Principal::new("New Principal"))]
            .into_iter()
            .collect(),
    );
    let create_res = client.principal_set(&create_req).unwrap();
    assert!(
        create_res
            .not_created
            .is_some_and(|nc| nc.contains_key("c1"))
    );

    // Destroy is rejected
    let mut destroy_req = SetRequest::<Principal>::new(account_id.clone());
    destroy_req.destroy = Some(vec![principal_id]);
    let destroy_res = client.principal_set(&destroy_req).unwrap();
    assert!(destroy_res.not_destroyed.is_some());
}

#[test]
fn principal_query_matches_text_description_email_substring_and_fields() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let seeded_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let acct = state.account_mut(&account_id).unwrap();
        let mut p = Principal::new("Support Team");
        p.email = Some("support@example.com".to_string());
        p.description = Some("Helps with conference room booking".to_string());
        p.principal_type = Some("resource".to_string());
        p.time_zone = Some("Europe/Berlin".to_string());
        p.accounts = Some([(Id::new("acc-help"), json!({}))].into_iter().collect());
        acct.seed_principal(p)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // Text matching in description
    let ids_text = client
        .principal_query(
            &account_id,
            PrincipalQueryFilter::default().text("conference room"),
        )
        .unwrap();
    assert!(ids_text.contains(&seeded_id));

    // Email substring matching
    let ids_email = client
        .principal_query(&account_id, PrincipalQueryFilter::email("example.com"))
        .unwrap();
    assert!(ids_email.contains(&seeded_id));

    // Type matching
    let ids_type = client
        .principal_query(
            &account_id,
            PrincipalQueryFilter::default().principal_type("resource"),
        )
        .unwrap();
    assert!(ids_type.contains(&seeded_id));

    // TimeZone matching
    let ids_tz = client
        .principal_query(
            &account_id,
            PrincipalQueryFilter::default().time_zone("Europe/Berlin"),
        )
        .unwrap();
    assert!(ids_tz.contains(&seeded_id));

    // AccountIds matching
    let ids_acc = client
        .principal_query(
            &account_id,
            PrincipalQueryFilter::default().account_ids(vec!["acc-help"]),
        )
        .unwrap();
    assert!(ids_acc.contains(&seeded_id));
}

#[test]
fn share_notification_set_dismisses_notification_and_rejects_create_update() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let notif_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let acct = state.account_mut(&account_id).unwrap();
        let current_principal = acct
            .current_user_principal_id
            .clone()
            .unwrap_or_else(|| Id::new("p-owner"));
        acct.current_user_principal_id = Some(current_principal.clone());
        let notif = ShareNotification::new(
            UtcDate::new("2026-06-01T12:00:00Z"),
            "Calendar",
            Id::new("cal-1"),
            account_id.clone(),
        );
        let id = acct.share_notifications.alloc_id();
        acct.share_notifications
            .seed_with_id(id.clone(), (current_principal, notif.with_id(id.clone())));
        id
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // Verify notification is visible before destroy
    let notifs = client.share_notifications(&account_id).unwrap();
    assert!(notifs.iter().any(|n| n.id.as_ref() == Some(&notif_id)));

    // Destroy notification
    let destroy_res = client.share_notification_destroy(&account_id, &notif_id);
    assert!(destroy_res.is_ok());

    // Verify notification is no longer returned
    let notifs_after = client.share_notifications(&account_id).unwrap();
    assert!(
        !notifs_after
            .iter()
            .any(|n| n.id.as_ref() == Some(&notif_id))
    );

    // Create and update are rejected
    let mut invalid_req = SetRequest::<ShareNotification>::new(account_id.clone());
    invalid_req.create = Some(
        [(
            "c1".to_string(),
            ShareNotification::new(
                UtcDate::new("2026-06-01T12:00:00Z"),
                "Calendar",
                Id::new("cal-2"),
                account_id.clone(),
            ),
        )]
        .into_iter()
        .collect(),
    );
    invalid_req.update = Some(
        [("sn-bogus".into(), json!({"objectType": "Mailbox"}))]
            .into_iter()
            .collect(),
    );
    let res = client.share_notification_set(&invalid_req).unwrap();
    assert!(res.not_created.is_some_and(|nc| nc.contains_key("c1")));
    assert!(
        res.not_updated
            .is_some_and(|nu| nu.contains_key(&Id::new("sn-bogus")))
    );
}

#[test]
fn share_notification_query_after_exact_timestamp_and_object_account_id() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let created_date = UtcDate::new("2026-06-01T12:00:00Z");
    let (n1_id, n2_id) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let acct = state.account_mut(&account_id).unwrap();
        let current_principal = acct
            .current_user_principal_id
            .clone()
            .unwrap_or_else(|| Id::new("p-owner"));
        acct.current_user_principal_id = Some(current_principal.clone());

        let n1 = ShareNotification::new(
            created_date.clone(),
            "Calendar",
            Id::new("cal-1"),
            Id::new("acc-remote-1"),
        );
        let id1 = acct.share_notifications.alloc_id();
        acct.share_notifications.seed_with_id(
            id1.clone(),
            (current_principal.clone(), n1.with_id(id1.clone())),
        );

        let n2 = ShareNotification::new(
            UtcDate::new("2026-06-02T12:00:00Z"),
            "AddressBook",
            Id::new("ab-1"),
            Id::new("acc-remote-2"),
        );
        let id2 = acct.share_notifications.alloc_id();
        acct.share_notifications
            .seed_with_id(id2.clone(), (current_principal, n2.with_id(id2.clone())));

        (id1, id2)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // after condition: created >= after must match n1 (exact match) and n2 (later)
    let ids_after = client
        .share_notification_query(
            &account_id,
            ShareNotificationQueryFilter::new().with_after(created_date),
        )
        .unwrap();
    assert_eq!(ids_after.len(), 2);
    assert!(ids_after.contains(&n1_id));
    assert!(ids_after.contains(&n2_id));

    // objectAccountId filter: acc-remote-2 matches only n2
    let ids_obj_acc = client
        .share_notification_query(
            &account_id,
            ShareNotificationQueryFilter::new().with_object_account_id("acc-remote-2"),
        )
        .unwrap();
    assert_eq!(ids_obj_acc, vec![n2_id]);
}

#[test]
fn share_notification_query_sorts_by_created() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let (id1, id2) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let acct = state.account_mut(&account_id).unwrap();
        let current_principal = acct
            .current_user_principal_id
            .clone()
            .unwrap_or_else(|| Id::new("p-owner"));
        acct.current_user_principal_id = Some(current_principal.clone());

        let n1 = ShareNotification::new(
            UtcDate::new("2026-06-01T10:00:00Z"),
            "Calendar",
            Id::new("cal-1"),
            account_id.clone(),
        );
        let id1 = acct.share_notifications.alloc_id();
        acct.share_notifications.seed_with_id(
            id1.clone(),
            (current_principal.clone(), n1.with_id(id1.clone())),
        );

        let n2 = ShareNotification::new(
            UtcDate::new("2026-06-01T15:00:00Z"),
            "Calendar",
            Id::new("cal-2"),
            account_id.clone(),
        );
        let id2 = acct.share_notifications.alloc_id();
        acct.share_notifications
            .seed_with_id(id2.clone(), (current_principal, n2.with_id(id2.clone())));

        (id1, id2)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // Ascending sort by created: id1, then id2
    let req_asc = QueryRequest::<ShareNotificationQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::ascending("created")]);
    let res_asc: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:principals",
                ],
                "ShareNotification/query",
                &req_asc,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res_asc.ids, vec![id1.clone(), id2.clone()]);

    // Descending sort by created: id2, then id1
    let req_desc = QueryRequest::<ShareNotificationQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::descending("created")]);
    let res_desc: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:principals",
                ],
                "ShareNotification/query",
                &req_desc,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res_desc.ids, vec![id2, id1]);
}

#[test]
fn share_with_cannot_include_account_owner_principal() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let owner_principal_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let acct = state.account_mut(&account_id).unwrap();
        let mut p = Principal::new("Owner");
        p.email = Some("owner@example.com".to_string());
        acct.seed_current_user_principal(p)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // Attempting to create an address book with owner in shareWith must fail
    let mut book = AddressBook::new("SharedBook");
    let mut share_with = BTreeMap::new();
    share_with.insert(owner_principal_id.clone(), AddressBookRights::all());
    book.share_with = Some(share_with);

    let create_res = client.address_book_create(&account_id, &book);
    assert!(
        create_res.is_err(),
        "shareWith containing owner principal must be rejected on create"
    );

    // Creating without owner succeeds
    let mut valid_book = AddressBook::new("ValidBook");
    valid_book.share_with = Some(BTreeMap::new());
    let created = client
        .address_book_create(&account_id, &valid_book)
        .unwrap();
    let book_id = created.id.unwrap();

    // Updating to include owner in shareWith must fail
    let mut update_share = BTreeMap::new();
    update_share.insert(owner_principal_id, AddressBookRights::all());
    let mut update_req = SetRequest::<AddressBook>::new(account_id.clone());
    update_req.update = Some(
        [(book_id, json!({"shareWith": update_share}))]
            .into_iter()
            .collect(),
    );
    let update_res = client.address_book_set(&update_req).unwrap();
    assert!(
        update_res.not_updated.is_some(),
        "updating shareWith with owner principal must be rejected"
    );
}
