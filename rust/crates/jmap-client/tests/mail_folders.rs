// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Making, renaming and removing a folder: `Mailbox/set` (RFC 8621 §2.5).
//!
//! `Mailbox/get` answers what the account already has; this is the other
//! direction, and it is what a mail client's "New Folder", "Rename" and
//! "Delete" ask for. The refusals matter as much as the successes: a folder
//! that cannot be made because a sibling has the name, and one that cannot be
//! removed because it holds mail or holds another folder, are the two answers a
//! backend has to be able to tell its user about.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::{Mailbox, role};
use jmap_proto::{Id, State};
use serde_json::json;

/// A server with an inbox, and a client connected to it.
fn with_inbox() -> (MockServer, Client, Id, Id) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let inbox = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_mailbox("Inbox", Some(role::INBOX))
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    (server, client, account_id, inbox)
}

/// A mailbox named `name` under `parent`, as a client would ask for one.
fn asking_for(name: &str, parent: Option<&Id>) -> Mailbox {
    Mailbox {
        name: name.to_owned(),
        parent_id: parent.cloned(),
        ..Mailbox::default()
    }
}

/// The mailbox the account holds under `id`, or nothing.
fn listed(client: &Client, account_id: &Id, id: &Id) -> Option<Mailbox> {
    client
        .mailbox_get(account_id)
        .unwrap()
        .list
        .into_iter()
        .find(|mailbox| mailbox.id.as_ref() == Some(id))
}

/// The `Mailbox` state of the account.
fn mailbox_state(client: &Client, account_id: &Id) -> State {
    client.mailbox_get(account_id).unwrap().state
}

/// The set error type of a refusal, or a panic naming what came instead.
fn refusal(outcome: Result<(), Error>) -> String {
    match outcome {
        Err(Error::Set(set_error)) => set_error.error_type,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_created_mailbox_is_one_the_account_then_lists() {
    let (_server, client, account_id, _inbox) = with_inbox();

    let made = client
        .mailbox_create(&account_id, &asking_for("Invoices", None))
        .unwrap();

    // The id is the server's to hand out, and it is the whole reason a create
    // answers with an object rather than with nothing.
    let id = made.id.clone().expect("the server named the new mailbox");
    let listed = listed(&client, &account_id, &id).expect("the account lists it");
    assert_eq!(listed.name, "Invoices");
    assert_eq!(listed.parent_id, None);
    // A folder the user just asked for is one they are watching.
    assert_eq!(listed.is_subscribed, Some(true));
    // Nothing is filed in it yet, and the create says so rather than leaving
    // the caller to ask again.
    assert_eq!(made.total_emails, Some(0));
    assert_eq!(made.unread_emails, Some(0));
}

#[test]
fn a_created_mailbox_moves_the_state_a_delta_is_asked_from() {
    let (_server, client, account_id, _inbox) = with_inbox();
    let before = mailbox_state(&client, &account_id);

    let made = client
        .mailbox_create(&account_id, &asking_for("Invoices", None))
        .unwrap();

    let after = mailbox_state(&client, &account_id);
    assert_ne!(before, after);
    let delta = client
        .changes(&account_id, "Mailbox", &before)
        .unwrap_or_else(|error| panic!("Mailbox/changes after a create: {error:?}"));
    assert_eq!(delta.created, vec![made.id.clone().unwrap()]);
}

#[test]
fn a_mailbox_may_be_made_inside_another() {
    let (_server, client, account_id, inbox) = with_inbox();

    let made = client
        .mailbox_create(&account_id, &asking_for("2026", Some(&inbox)))
        .unwrap();

    let id = made.id.clone().unwrap();
    assert_eq!(
        listed(&client, &account_id, &id).unwrap().parent_id,
        Some(inbox)
    );
}

#[test]
fn two_folders_of_one_parent_may_not_share_a_name() {
    let (_server, client, account_id, inbox) = with_inbox();
    client
        .mailbox_create(&account_id, &asking_for("2026", Some(&inbox)))
        .unwrap();

    let again = client.mailbox_create(&account_id, &asking_for("2026", Some(&inbox)));

    assert_eq!(
        refusal(again.map(|_| ())),
        "invalidProperties",
        "a sibling already has the name"
    );
}

#[test]
fn one_name_under_two_parents_is_two_folders() {
    let (_server, client, account_id, inbox) = with_inbox();
    let under_inbox = client
        .mailbox_create(&account_id, &asking_for("2026", Some(&inbox)))
        .unwrap();

    // The name is taken only among siblings: the same name at the top level is
    // a different folder, and refusing it would be refusing an ordinary
    // arrangement of mail.
    let at_top = client
        .mailbox_create(&account_id, &asking_for("2026", None))
        .unwrap();

    assert_ne!(under_inbox.id, at_top.id);
}

#[test]
fn a_mailbox_under_a_parent_that_does_not_exist_is_refused() {
    let (_server, client, account_id, _inbox) = with_inbox();

    let made = client.mailbox_create(&account_id, &asking_for("Orphan", Some(&Id::new("M404"))));

    assert_eq!(refusal(made.map(|_| ())), "invalidProperties");
}

#[test]
fn a_mailbox_with_no_name_is_refused() {
    let (_server, client, account_id, _inbox) = with_inbox();

    let made = client.mailbox_create(&account_id, &asking_for("", None));

    assert_eq!(refusal(made.map(|_| ())), "invalidProperties");
}

#[test]
fn a_client_does_not_get_to_choose_the_id() {
    let (_server, client, account_id, _inbox) = with_inbox();
    let mut asked = asking_for("Invoices", None);
    asked.id = Some(Id::new("Mmine"));

    let made = client.mailbox_create(&account_id, &asked);

    assert_eq!(refusal(made.map(|_| ())), "invalidProperties");
}

#[test]
fn a_role_belongs_to_one_mailbox() {
    let (_server, client, account_id, _inbox) = with_inbox();
    let mut asked = asking_for("Second Inbox", None);
    asked.role = Some(role::INBOX.to_owned());

    let made = client.mailbox_create(&account_id, &asked);

    assert_eq!(
        refusal(made.map(|_| ())),
        "invalidProperties",
        "the account already has an inbox"
    );
}

#[test]
fn renaming_a_mailbox_changes_the_name_it_is_listed_under() {
    let (_server, client, account_id, _inbox) = with_inbox();
    let id = client
        .mailbox_create(&account_id, &asking_for("Invoces", None))
        .unwrap()
        .id
        .unwrap();

    client
        .mailbox_update(&account_id, &id, json!({"name": "Invoices"}))
        .unwrap();

    assert_eq!(listed(&client, &account_id, &id).unwrap().name, "Invoices");
}

#[test]
fn a_rename_onto_a_siblings_name_is_refused() {
    let (_server, client, account_id, inbox) = with_inbox();
    client
        .mailbox_create(&account_id, &asking_for("2025", Some(&inbox)))
        .unwrap();
    let id = client
        .mailbox_create(&account_id, &asking_for("2026", Some(&inbox)))
        .unwrap()
        .id
        .unwrap();

    let renamed = client.mailbox_update(&account_id, &id, json!({"name": "2025"}));

    assert_eq!(refusal(renamed), "invalidProperties");
    assert_eq!(listed(&client, &account_id, &id).unwrap().name, "2026");
}

#[test]
fn a_mailbox_may_be_moved_under_another() {
    let (_server, client, account_id, inbox) = with_inbox();
    let id = client
        .mailbox_create(&account_id, &asking_for("2026", None))
        .unwrap()
        .id
        .unwrap();

    client
        .mailbox_update(&account_id, &id, json!({"parentId": inbox.as_str()}))
        .unwrap();

    assert_eq!(
        listed(&client, &account_id, &id).unwrap().parent_id,
        Some(inbox)
    );
}

#[test]
fn a_mailbox_may_not_be_moved_inside_itself() {
    let (_server, client, account_id, _inbox) = with_inbox();
    let parent = client
        .mailbox_create(&account_id, &asking_for("Projects", None))
        .unwrap()
        .id
        .unwrap();
    let child = client
        .mailbox_create(&account_id, &asking_for("2026", Some(&parent)))
        .unwrap()
        .id
        .unwrap();

    // Both halves of the loop: under its own child, and under itself.
    let under_child =
        client.mailbox_update(&account_id, &parent, json!({"parentId": child.as_str()}));
    let under_itself =
        client.mailbox_update(&account_id, &parent, json!({"parentId": parent.as_str()}));

    assert_eq!(refusal(under_child), "invalidProperties");
    assert_eq!(refusal(under_itself), "invalidProperties");
    assert_eq!(
        listed(&client, &account_id, &parent).unwrap().parent_id,
        None
    );
}

#[test]
fn unsubscribing_is_an_update_like_any_other() {
    let (_server, client, account_id, inbox) = with_inbox();

    client
        .mailbox_update(&account_id, &inbox, json!({"isSubscribed": false}))
        .unwrap();

    assert_eq!(
        listed(&client, &account_id, &inbox).unwrap().is_subscribed,
        Some(false)
    );
}

#[test]
fn an_update_to_a_mailbox_that_is_not_there_is_not_found() {
    let (_server, client, account_id, _inbox) = with_inbox();

    let renamed = client.mailbox_update(&account_id, &Id::new("M404"), json!({"name": "Gone"}));

    assert_eq!(refusal(renamed), "notFound");
}

#[test]
fn a_destroyed_mailbox_stops_being_listed() {
    let (_server, client, account_id, _inbox) = with_inbox();
    let id = client
        .mailbox_create(&account_id, &asking_for("Invoices", None))
        .unwrap()
        .id
        .unwrap();

    client.mailbox_destroy(&account_id, &id).unwrap();

    assert!(listed(&client, &account_id, &id).is_none());
}

#[test]
fn a_mailbox_holding_another_is_not_destroyed() {
    let (_server, client, account_id, _inbox) = with_inbox();
    let parent = client
        .mailbox_create(&account_id, &asking_for("Projects", None))
        .unwrap()
        .id
        .unwrap();
    client
        .mailbox_create(&account_id, &asking_for("2026", Some(&parent)))
        .unwrap();

    let destroyed = client.mailbox_destroy(&account_id, &parent);

    // RFC 8621 §2.5's own error, and the reason it is its own error: the fix
    // is to remove the child first, which is something only the user can
    // decide.
    assert_eq!(refusal(destroyed), "mailboxHasChild");
    assert!(listed(&client, &account_id, &parent).is_some());
}

#[test]
fn a_mailbox_holding_mail_is_not_destroyed() {
    let (server, client, account_id, _inbox) = with_inbox();
    let id = client
        .mailbox_create(&account_id, &asking_for("Invoices", None))
        .unwrap()
        .id
        .unwrap();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_email(EmailSeed::new(
            id.clone(),
            ("Bob", "bob@example.com"),
            "An invoice",
            "Please find it attached.",
            "2026-08-01T10:00:00Z",
        ));
    }

    let destroyed = client.mailbox_destroy(&account_id, &id);

    assert_eq!(refusal(destroyed), "mailboxHasEmail");
    assert!(listed(&client, &account_id, &id).is_some());
}

#[test]
fn destroying_a_mailbox_that_is_not_there_is_not_found() {
    let (_server, client, account_id, _inbox) = with_inbox();

    let destroyed = client.mailbox_destroy(&account_id, &Id::new("M404"));

    assert_eq!(refusal(destroyed), "notFound");
}
