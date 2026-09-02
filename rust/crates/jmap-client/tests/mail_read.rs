// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Receiving email: query + fetch, attachments.

use jmap_client::{Client, Credentials, limits};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::{EmailQueryFilter, role};
use jmap_proto::methods::Comparator;

#[test]
fn receive_email_query_then_fetch() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let inbox = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Hello Alice",
            "Hi Alice, how are you?",
            "2026-08-01T10:00:00Z",
        ));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Carol", "carol@example.com"),
            "Meeting tomorrow",
            "Can we meet at 10?",
            "2026-08-02T09:30:00Z",
        ));
        inbox
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // The client finds the inbox by role, like a real mail client would.
    let mailboxes = client.mailbox_get(&account_id).unwrap().list;
    let inbox_from_server = mailboxes
        .iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::INBOX))
        .expect("inbox exists");
    assert_eq!(inbox_from_server.id.as_ref(), Some(&inbox));
    assert_eq!(inbox_from_server.total_emails, Some(2));
    assert_eq!(inbox_from_server.unread_emails, Some(2));

    // Query newest-first and fetch in the same request via back-reference.
    let emails = client
        .email_query_then_get(
            &account_id,
            EmailQueryFilter::in_mailbox(inbox.clone()),
            Some(vec![Comparator::descending("receivedAt")]),
            None,
        )
        .unwrap();

    assert_eq!(emails.len(), 2);
    assert_eq!(emails[0].subject.as_deref(), Some("Meeting tomorrow"));
    assert_eq!(emails[1].subject.as_deref(), Some("Hello Alice"));
    assert_eq!(emails[1].from.as_ref().unwrap()[0].email, "bob@example.com");

    // Body text is reachable via textBody partId → bodyValues.
    let email = &emails[1];
    let part_id = email.text_body.as_ref().unwrap()[0]
        .part_id
        .clone()
        .unwrap();
    assert_eq!(
        email.body_values.as_ref().unwrap()[&part_id].value,
        "Hi Alice, how are you?"
    );
}

#[test]
fn receive_email_attachment_blob_download() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    let attachment_bytes = b"%PDF-1.7 fake report".to_vec();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        let blob_id = account.add_blob("application/pdf", attachment_bytes.clone());
        account.seed_email(
            EmailSeed::new(
                inbox,
                ("Bob", "bob@example.com"),
                "Report attached",
                "See attachment.",
                "2026-08-03T08:00:00Z",
            )
            .attachment(blob_id, "report.pdf", "application/pdf"),
        );
    }

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let emails = client
        .email_query_then_get(
            &account_id,
            EmailQueryFilter::default(),
            None,
            Some(&["subject", "hasAttachment", "attachments"]),
        )
        .unwrap();

    assert_eq!(emails.len(), 1);
    let email = &emails[0];
    assert_eq!(email.has_attachment, Some(true));
    // Property projection: unrequested properties must be absent.
    assert!(email.from.is_none());

    let attachment = &email.attachments.as_ref().unwrap()[0];
    assert_eq!(attachment.name.as_deref(), Some("report.pdf"));
    assert_eq!(attachment.content_type.as_deref(), Some("application/pdf"));

    let downloaded = client
        .download_blob(
            &account_id,
            attachment.blob_id.as_ref().unwrap(),
            "report.pdf",
            limits::MAX_BLOB_BYTES,
        )
        .unwrap();
    assert_eq!(downloaded, attachment_bytes);
}

/// `Email/query` filter fields the mock did not apply until now: `cc`,
/// `bcc`, `body`, `text`, `header`, `minSize`/`maxSize`, `hasAttachment`.
/// `from`/`to`/`subject`/`inMailbox`/keyword/date filters are covered above
/// and elsewhere; this test is only about the fields item 46(b) added.
#[test]
fn email_query_filters_the_fields_the_mock_used_to_ignore() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();

    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(
            EmailSeed::new(
                inbox.clone(),
                ("Bob", "bob@example.com"),
                "Short note",
                "hi",
                "2026-08-01T10:00:00Z",
            )
            .cc("Carol", "carol@example.com")
            .header("X-Priority", "1"),
        );
        let blob_id = account.add_blob("application/pdf", b"fake pdf".to_vec());
        account.seed_email(
            EmailSeed::new(
                inbox,
                ("Dave", "dave@example.com"),
                "Longer report",
                "quarterly numbers attached below",
                "2026-08-02T09:00:00Z",
            )
            .bcc("Eve", "eve@example.com")
            .attachment(blob_id, "report.pdf", "application/pdf"),
        );
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let subjects = |filter: EmailQueryFilter| -> Vec<String> {
        client
            .email_query_then_get(&account_id, filter, None, Some(&["subject"]))
            .unwrap()
            .into_iter()
            .map(|email| email.subject.unwrap_or_default())
            .collect()
    };

    assert_eq!(
        subjects(EmailQueryFilter::default().cc("carol@example.com")),
        vec!["Short note"]
    );
    assert_eq!(
        subjects(EmailQueryFilter::default().bcc("eve@example.com")),
        vec!["Longer report"]
    );
    assert_eq!(
        subjects(EmailQueryFilter::default().body("quarterly")),
        vec!["Longer report"]
    );
    // `text` reaches into addresses and the subject too, not just the body.
    assert_eq!(
        subjects(EmailQueryFilter::default().text("carol@example.com")),
        vec!["Short note"]
    );
    assert_eq!(
        subjects(EmailQueryFilter::default().header("X-Priority", "1")),
        vec!["Short note"]
    );
    assert_eq!(
        subjects(EmailQueryFilter::default().has_attachment(true)),
        vec!["Longer report"]
    );
    assert_eq!(
        subjects(EmailQueryFilter::default().has_attachment(false)),
        vec!["Short note"]
    );

    // minSize/maxSize bracket the shorter message's raw size between the two.
    let sizes = client
        .email_query_then_get(
            &account_id,
            EmailQueryFilter::default(),
            Some(vec![Comparator::ascending("receivedAt")]),
            Some(&["size"]),
        )
        .unwrap();
    let short_size = sizes[0].size.unwrap();
    let long_size = sizes[1].size.unwrap();
    assert!(short_size < long_size);
    let threshold = (short_size + long_size) / 2;
    assert_eq!(
        subjects(EmailQueryFilter::default().min_size(threshold)),
        vec!["Longer report"]
    );
    assert_eq!(
        subjects(EmailQueryFilter::default().max_size(threshold)),
        vec!["Short note"]
    );
}

/// The state probe: `Email/get` naming no ids at all, which is how a client
/// learns what to ask `Email/changes` from without downloading a mailbox first.
#[test]
fn the_email_state_can_be_read_without_reading_any_email() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let held = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "Hello",
            "text",
            "2026-08-01T10:00:00Z",
        ));
        account.emails.state()
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    assert_eq!(client.email_state(&account_id).unwrap(), held);

    // And it moves with the account, which is the whole reason to read it.
    let delivered = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Later", None);
        account.deliver_email(EmailSeed::new(
            inbox,
            ("Carol", "carol@example.com"),
            "Newer",
            "text",
            "2026-08-02T10:00:00Z",
        ));
        account.emails.state()
    };
    assert_ne!(delivered, held);
    assert_eq!(client.email_state(&account_id).unwrap(), delivered);
}
