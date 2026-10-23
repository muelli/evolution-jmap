// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail: Camel finds `libcameljmap.so` through the `.urls` file next to it,
//! opens a store from a `.source` keyfile, and gets the folder tree and the
//! inbox from the mock JMAP server.
//!
//! The mail test is different from the address book and calendar tests. An
//! EDS factory daemon loads those backends from a directory. The *mail client
//! process* loads a Camel provider, and only when a request names a protocol
//! that a `.urls` file in the Camel provider directory claims. The tests of
//! `jmap-mail` link the provider, so they cannot show that Camel finds it.
//! Here, the client is a plain libcamel consumer and does not link the
//! provider.
//!
//! The test examines only the two ends. The client program reports what
//! Camel gave it, and the mock reports the requests of the provider.
//!
//! [`the_live_client_fails_the_connect_on_a_rejected_password`] tests the
//! session of `mail-live-client.c`, not the provider (see its doc).

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_mock::EmailSeed;

/// The two messages in the inbox of the mock. The client only reports what
/// it read.
///
/// The client sorts each list, so these constants are in sorted order. In a
/// different order, the set comparisons fail for a reason not related to the
/// provider.
const FIRST_SUBJECT: &str = "Lunch on Tuesday";
const SECOND_SUBJECT: &str = "Re: the quarterly numbers";
const FIRST_BODY: &str = "One o'clock at the usual place.";
const SECOND_BODY: &str = "They are fine.";

/// The keyfile from `docs/examples/jmap-mock-standalone-mail.source`, with
/// the ephemeral port of the mock. Only the *account*, not the identity or
/// the transport, because this test sends nothing.
///
/// It is a literal, so a change to the documented recipe makes this test
/// fail. `recipe.rs` of `jmap-mail` examines the documented files.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::MailAccount, port).to_string()
}

#[test]
fn camel_opens_the_store_and_serves_the_inbox() {
    let client = required_path("JMAP_FUNCTIONAL_MAIL_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // Five mailboxes with their JMAP roles and two messages in the inbox.
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");

        let inbox = account.seed_mailbox("Inbox", Some("inbox"));
        account.seed_mailbox("Sent", Some("sent"));
        // Drafts starts unsubscribed, as the only mailbox. The
        // `CamelSubscribable` block subscribes it, so the write must change
        // the value.
        let drafts = account.seed_mailbox("Drafts", Some("drafts"));
        account
            .mailboxes
            .get_mut(&drafts)
            .expect("the mailbox just seeded")
            .is_subscribed = Some(false);
        // For `get_trash_folder_sync` and `get_junk_folder_sync`, which find
        // a folder from its role, as `get_inbox_folder_sync` does.
        account.seed_mailbox("Trash", Some("trash"));
        account.seed_mailbox("Junk", Some("junk"));

        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            FIRST_SUBJECT,
            FIRST_BODY,
            "2026-01-14T09:00:00Z",
        ));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Carol", "carol@example.com"),
            SECOND_SUBJECT,
            SECOND_BODY,
            "2026-01-14T10:00:00Z",
        ));
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/mail"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_camel_provider(&module, &urls);

    let output = session.run(&client, &["jmap-functional"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Assert this before the exit status, because it tells *why* a failure
    // occurred. Camel finds providers by protocol. The account `BackendName`,
    // the line in `libcameljmap.urls` and the `camel_provider_module_init`
    // string must be the same. If not, each step fails with "no provider
    // available for protocol", which hides a typo in one of the three files.
    assert_eq!(
        seen.get("protocol"),
        Some(&"jmap"),
        "the source names a protocol the provider does not register\n{report}"
    );
    assert_eq!(
        seen.get("store-connected"),
        Some(&"1"),
        "Camel never opened the store — the provider was not found, or its \
         connect failed\n{report}"
    );

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // Only the five mailboxes of the mock: the provider builds the tree from
    // one `Mailbox/get`.
    assert_eq!(
        seen.get("folders"),
        Some(&"Drafts,Inbox,Junk,Sent,Trash"),
        "the folder tree is not the mock's five mailboxes\n{report}"
    );

    // `CamelSubscribable`: the check mark of a folder in the subscription
    // editor, through the real vtable. `tests/subscriptions.rs` of
    // `jmap-mail` calls the plain decision function. First, the local state.
    assert_eq!(
        seen.get("sent-subscribed-initially"),
        Some(&"1"),
        "Sent did not start subscribed, the way every mock-seeded mailbox does\n{report}"
    );
    assert_eq!(
        seen.get("sent-subscribed-after-unsubscribe"),
        Some(&"0"),
        "unsubscribe_folder_sync did not flip the store's own held answer for Sent\n{report}"
    );
    assert_eq!(
        seen.get("drafts-subscribed-initially"),
        Some(&"0"),
        "Drafts did not start unsubscribed, the way this test seeded it\n{report}"
    );
    assert_eq!(
        seen.get("drafts-subscribed-after-subscribe"),
        Some(&"1"),
        "subscribe_folder_sync did not flip the store's own held answer for Drafts\n{report}"
    );

    // Then the copy of the mock. If the write does not get to the server,
    // the in-memory state of Camel above is still correct.
    {
        let state = server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&account_id)
            .expect("the mock's default account");
        let subscribed = |name: &str| {
            account
                .mailboxes
                .iter()
                .find(|(_, mailbox)| mailbox.name == name)
                .and_then(|(_, mailbox)| mailbox.is_subscribed)
        };
        assert_eq!(
            subscribed("Sent"),
            Some(false),
            "unsubscribe_folder_sync never reached the server: the mock's own copy of \
             Sent still carries isSubscribed=true\n{report}"
        );
        assert_eq!(
            subscribed("Drafts"),
            Some(true),
            "subscribe_folder_sync never reached the server: the mock's own copy of \
             Drafts still carries isSubscribed=false\n{report}"
        );
    }

    // By role, not by name. The provider identifies the inbox from the JMAP
    // role of the mailbox. A provider that ignores roles returns the first
    // folder that it opened.
    assert_eq!(
        seen.get("inbox-full-name"),
        Some(&"Inbox"),
        "the store's inbox is not the mailbox with the inbox role\n{report}"
    );

    // `get_trash_folder_sync` and `get_junk_folder_sync` use the role, not
    // the inherited vTrash/vJunk virtual folders. The module doc of
    // `folders.rs` tells why those are incorrect for a JMAP account.
    assert_eq!(
        seen.get("trash-full-name"),
        Some(&"Trash"),
        "the store's trash folder is not the mailbox with the trash role\n{report}"
    );
    assert_eq!(
        seen.get("junk-full-name"),
        Some(&"Junk"),
        "the store's junk folder is not the mailbox with the junk role\n{report}"
    );

    assert_eq!(
        seen.get("inbox-count"),
        Some(&"2"),
        "the inbox does not hold the two seeded messages\n{report}"
    );
    assert_eq!(
        seen.get("inbox-subjects"),
        Some(&format!("{FIRST_SUBJECT},{SECOND_SUBJECT}").as_str()),
        "the summaries Camel built are not the seeded messages\n{report}"
    );

    // The two full messages use a different request. The summaries come
    // from `Email/query` and `Email/get`, a body from a blob download (a plain
    // HTTP GET). The client sorts the bodies, so this compares a set.
    assert_eq!(
        seen.get("message-bodies"),
        Some(&format!("{FIRST_BODY},{SECOND_BODY}").as_str()),
        "the message bodies did not survive the download\n{report}"
    );

    // `synchronize_sync`, the write path. A flag stays local in Camel until a
    // synchronization sends it to the server as a keyword. The test examines
    // the two ends. If the synchronization hides a write failure, the
    // in-memory row of Camel still shows the flag.
    let flagged_uid = match seen.get("flagged-uid") {
        Some(uid) => (*uid).to_string(),
        None => panic!("the client never named a message to flag\n{report}"),
    };
    assert_eq!(
        seen.get("flagged-after-sync"),
        Some(&"1"),
        "the flag did not survive a fresh read of the row after synchronize_sync\n{report}"
    );

    // The other end: the copy of the message in the mock.
    {
        let state = server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&account_id)
            .expect("the mock's default account");
        let email = account
            .emails
            .get(&jmap_proto::Id::new(flagged_uid.clone()))
            .unwrap_or_else(|| panic!("the flagged message is not on the server at all\n{report}"));
        assert_eq!(
            email
                .keywords
                .as_ref()
                .and_then(|keywords| keywords.get(jmap_proto::mail::keyword::FLAGGED))
                .copied(),
            Some(true),
            "synchronize_sync never reached the server: the mock's own copy of \
             message {flagged_uid} carries no {} keyword\n{report}",
            jmap_proto::mail::keyword::FLAGGED
        );
    }

    // The other end: the requests that the server got. All calls above are
    // synchronous, so there is no race.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "Email/set"),
        "synchronize_sync never asked the server for Email/set; it asked for {calls:?}\n{report}"
    );
    for method in ["Mailbox/get", "Email/query", "Email/get"] {
        assert!(
            calls.iter().any(|call| call == method),
            "the provider never asked for {method}; it asked for {calls:?}\n{report}"
        );
    }

    // `create_folder_sync` through the real vtable. The unit tests of
    // `jmap-mail` call the plain decision function.
    assert_eq!(
        seen.get("create-folder-name"),
        Some(&"Receipts"),
        "the created folder's full name is not the mailbox name asked for\n{report}"
    );

    // The listing of the store contains the new folder.
    assert_eq!(
        seen.get("folders-after-create"),
        Some(&"Drafts,Inbox,Junk,Receipts,Sent,Trash"),
        "the store's own listing did not gain the new folder\n{report}"
    );

    // `append_message_sync`: a message from outside the account goes into
    // the inbox. It uses `Email/import` of an uploaded blob, not the
    // `Email/set` transfer. The test examines the round trip: the listing
    // grows, and a fetch by the new uid gives the same subject.
    assert!(
        seen.get("append-uid").is_some_and(|uid| !uid.is_empty()),
        "append_message_sync did not mint a uid for the appended message\n{report}"
    );
    assert_eq!(
        seen.get("inbox-count-after-append"),
        Some(&"3"),
        "the inbox listing did not grow by the appended message\n{report}"
    );
    assert_eq!(
        seen.get("appended-subject"),
        Some(&"Dropped in"),
        "re-fetching the appended message by its minted uid did not return the message that went up\n{report}"
    );
    assert!(
        calls.iter().any(|call| call == "Email/import"),
        "append_message_sync never asked the server for Email/import; it asked for {calls:?}\n{report}"
    );

    // `transfer_messages_to_sync` moves the appended message from the inbox
    // to "Receipts". RFC 8621 gives an `Email` one immutable id per account.
    // Thus the transfer reports the uid of the append (see the doc of
    // `Reported` in `transfer.rs`).
    assert_eq!(
        seen.get("transfer-uid"),
        seen.get("append-uid"),
        "transfer_messages_to_sync reported a different uid than the message it moved was appended under\n{report}"
    );

    // The row leaves the inbox (again two messages) and goes into the
    // destination.
    assert_eq!(
        seen.get("inbox-count-after-transfer"),
        Some(&"2"),
        "the inbox still holds the message after it was moved out\n{report}"
    );
    assert_eq!(
        seen.get("receipts-count-after-transfer"),
        Some(&"1"),
        "the destination folder does not hold the moved message\n{report}"
    );

    // The message moves back before the rename and delete of "Receipts". A
    // JMAP server refuses to destroy a mailbox that contains a message.
    assert_eq!(
        seen.get("receipts-count-after-transfer-back"),
        Some(&"0"),
        "the destination folder still holds the message after it was moved back out\n{report}"
    );
    assert_eq!(
        seen.get("inbox-count-after-transfer-back"),
        Some(&"3"),
        "the inbox does not hold the message again after it was moved back\n{report}"
    );

    // The other end: the transfer is one `Email/set` that changes
    // `mailboxIds`, not a second `Email/import`.
    assert!(
        calls.iter().any(|call| call == "Email/set"),
        "transfer_messages_to_sync never asked the server for Email/set; it asked for {calls:?}\n{report}"
    );

    // `rename_folder_sync` through the real vtable, not
    // `manage::rename_folder`. Only the last component changes, under the
    // same (root) parent. This is the "name the user typed" case of the
    // `manage.rs` doc comment, not a drag-and-drop.
    assert_eq!(
        seen.get("folders-after-rename"),
        Some(&"Drafts,Inbox,Invoices,Junk,Sent,Trash"),
        "the store's own listing did not reflect the rename\n{report}"
    );

    // The other end for create, rename and delete. The client completes all
    // three before the assertions read the mock state, so the test cannot
    // examine the mock between the calls. The method log proves that the
    // requests got to the server, not to a local cache. There are three
    // `Mailbox/set` calls for create, rename and delete. `set_subscribed`
    // also uses `Mailbox/set`, one call each for Sent and Drafts. The total
    // is five.
    let mailbox_set_calls = calls.iter().filter(|call| *call == "Mailbox/set").count();
    assert_eq!(
        mailbox_set_calls, 5,
        "expected one Mailbox/set each for the create, the rename, the delete, the unsubscribe and the subscribe; saw {mailbox_set_calls} in {calls:?}\n{report}"
    );

    // `delete_folder_sync` through the real vtable, not
    // `manage::delete_folder`. The client deletes the folder by its new name.
    assert_eq!(
        seen.get("folders-after-delete"),
        Some(&"Drafts,Inbox,Junk,Sent,Trash"),
        "the store's own listing did not lose the deleted folder\n{report}"
    );

    // The other end: the mock no longer has the folder. The test examines
    // the two names, because a rename can leave a copy with the old name.
    {
        let state = server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&account_id)
            .expect("the mock's default account");
        let names: std::collections::BTreeSet<&str> = account
            .mailboxes
            .iter()
            .map(|(_, mailbox)| mailbox.name.as_str())
            .collect();
        assert!(
            !names.contains("Receipts") && !names.contains("Invoices"),
            "the mock's own mailbox store still holds the renamed/deleted folder after delete; it holds {names:?}\n{report}"
        );
    }

    // `expunge_sync` is "Expunge" or "Empty Trash". `synchronize_sync` calls
    // it when its `expunge` argument is TRUE (FALSE above). The message is
    // not the message that `synchronize_sync` flagged. Until this point the
    // inbox has three messages: the two seeds and the appended message.
    let deleted_uid = match seen.get("deleted-uid") {
        Some(uid) => (*uid).to_string(),
        None => panic!("the client never named a message to delete\n{report}"),
    };
    assert_eq!(
        seen.get("inbox-count-after-expunge"),
        Some(&"2"),
        "the inbox listing did not shrink by the expunged message\n{report}"
    );
    assert_eq!(
        seen.get("expunged-uid-still-listed"),
        Some(&"0"),
        "the expunged message's row is still in the folder's own uid list\n{report}"
    );

    // The other end: the message is now in the trash of the mock, and only
    // there. The account has a trash, so `expunge_sync` outside the trash is a
    // move, not a destroy (IMAPX with a real trash). A hidden write failure
    // still shows a smaller local listing, but the server keeps the message
    // in the inbox.
    {
        let state = server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&account_id)
            .expect("the mock's default account");
        let trash = account
            .mailboxes
            .iter()
            .find(|(_, mailbox)| mailbox.role.as_deref() == Some("trash"))
            .map(|(id, _)| id.clone())
            .expect("the trash this test seeded");
        let mailboxes = account
            .emails
            .get(&jmap_proto::Id::new(deleted_uid.clone()))
            .and_then(|email| email.mailbox_ids.clone())
            .unwrap_or_default();
        assert_eq!(
            mailboxes.keys().collect::<Vec<_>>(),
            [&trash],
            "expunge_sync did not move message {deleted_uid} into the trash only\n{report}"
        );
    }
}

/// The user and password that the Basic-auth mock of
/// [`the_live_client_fails_the_connect_on_a_rejected_password`] accepts.
const BASIC_USER: &str = "alice";
const BASIC_PASSWORD: &str = "correct horse";

/// `mail-live-client.c` runs against a real server, so a wrong password there
/// must fail the connect with an error. Its session retries a `REJECTED`
/// attempt once. When the retry is `REJECTED` too, the session must set an
/// error. Without it, the connect returns TRUE with the store not connected.
#[test]
fn the_live_client_fails_the_connect_on_a_rejected_password() {
    let client = required_path("JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT");
    let server = jmap_mock::MockServer::builder()
        .basic_auth(BASIC_USER, BASIC_PASSWORD)
        .start();
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/mail-live-rejected"));
    session.write_source(
        "jmap-functional",
        &keyfile(port).replace(
            "[Authentication]\n",
            &format!("[Authentication]\nUser={BASIC_USER}\n"),
        ),
    );
    session.stage_camel_provider(
        &required_path("JMAP_FUNCTIONAL_MAIL_MODULE"),
        &required_path("JMAP_FUNCTIONAL_MAIL_URLS"),
    );
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", "wrong password");

    // `timeout` exits with 124 when it stops the client.
    let client = client.to_str().expect("the client path is UTF-8");
    let output = session.run(
        std::path::Path::new("/usr/bin/timeout"),
        &[
            "--kill-after=5",
            "30",
            client,
            "jmap-functional",
            "unused subject",
            "receive",
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let unauthorized = server.unauthorized_responses();

    assert_ne!(
        output.status.code(),
        Some(124),
        "the client was still connecting after 30 s ({unauthorized} 401 responses)\n{report}"
    );
    assert!(
        !output.status.success(),
        "a wrong password connected the store\n{report}"
    );
    assert!(
        stderr.contains("connect: ") && !stderr.contains("(no error set)"),
        "the connect did not fail with an error\n{report}"
    );
    // One attempt with the seeded password and one retry with the value from
    // `get_password`. Each attempt can make a few requests before its 401.
    assert!(
        (1..=4).contains(&unauthorized),
        "the client made {unauthorized} attempts that got a 401\n{report}"
    );
}

/// A server that answers each `Email/query` with the same limited, non-empty
/// page is out of spec (RFC 8620 §5.5). The provider lists the messages when
/// Camel opens the inbox. That step must fail with an error. It must not hang
/// the Camel thread or crash.
/// `messages.rs` of `jmap-mail-sync` tests the same limit without Camel.
#[test]
fn a_query_that_never_stops_paging_fails_the_inbox_with_an_error() {
    let client = required_path("JMAP_FUNCTIONAL_MAIL_CLIENT");
    let server = jmap_mock::MockServer::builder()
        .query_page_size(1)
        .query_never_terminates()
        .start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        // The client works on all five folders before it refreshes the inbox.
        let inbox = account.seed_mailbox("Inbox", Some("inbox"));
        for (name, role) in [
            ("Sent", "sent"),
            ("Drafts", "drafts"),
            ("Trash", "trash"),
            ("Junk", "junk"),
        ] {
            account.seed_mailbox(name, Some(role));
        }
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            FIRST_SUBJECT,
            FIRST_BODY,
            "2026-01-14T09:00:00Z",
        ));
    }
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/mail-query-paging"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_camel_provider(
        &required_path("JMAP_FUNCTIONAL_MAIL_MODULE"),
        &required_path("JMAP_FUNCTIONAL_MAIL_URLS"),
    );

    // `timeout` exits with 124 when it stops the client.
    let client = client.to_str().expect("the client path is UTF-8");
    let output = session.run(
        std::path::Path::new("/usr/bin/timeout"),
        &["--kill-after=5", "60", client, "jmap-functional"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let queries = server
        .method_calls()
        .iter()
        .filter(|name| name.as_str() == "Email/query")
        .count();

    assert_ne!(
        output.status.code(),
        Some(124),
        "the listing was still paging after 60 s ({queries} Email/query calls)\n{report}"
    );
    assert_eq!(
        observations(&stdout).get("store-connected"),
        Some(&"1"),
        "the store did not connect, so the listing never ran\n{report}"
    );
    assert!(
        !output.status.success(),
        "the client accepted a query that never stopped paging\n{report}"
    );
    assert!(
        stderr.contains("inbox: protocol error: Email/query never stopped"),
        "opening the inbox did not fail with the paging error\n{report}"
    );
    // The page limit of `jmap-mail-sync` is 1024 pages for one listing.
    assert!(
        (1024..=2048).contains(&queries),
        "the provider made {queries} Email/query calls\n{report}"
    );
}
