// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, mail: Camel finding `libcameljmap.so` by the `.urls` file
//! beside it, opening a store from a `.source` keyfile, and serving the
//! folder tree and the inbox out of the mock JMAP server.
//!
//! The third leg, and the one that is not a mirror of the other two. An
//! address book or calendar backend is dlopened by a factory daemon EDS
//! ships, found by being a file in a directory; a Camel provider is dlopened
//! by the *mail client's own process*, and only when something asks for a
//! protocol that a `.urls` file in Camel's provider directory claims. That
//! is a whole loading mechanism no other test in this repository exercises:
//! `jmap-mail`'s own tests link the provider in, so they cannot tell whether
//! Camel would ever have found it. Here nothing links it — the client is a
//! plain libcamel consumer, and the provider is a file in a directory.
//!
//! Everything is checked from the two ends and nothing in between: the
//! client program says what Camel gave it, the mock says what the provider
//! asked the server for.

use jmap_functional::{Session, observations, required_path};
use jmap_mock::EmailSeed;

/// The two messages the mock's inbox is seeded with. Named here rather than
/// in the client, which reports what it read and holds no opinion about it.
///
/// Every list the client reports is sorted, so these are written in the
/// order they sort in: the assertions compare sets, and a set spelled out of
/// order would fail for a reason that has nothing to do with the provider.
const FIRST_SUBJECT: &str = "Lunch on Tuesday";
const SECOND_SUBJECT: &str = "Re: the quarterly numbers";
const FIRST_BODY: &str = "One o'clock at the usual place.";
const SECOND_BODY: &str = "They are fine.";

/// The keyfile from `docs/examples/jmap-mock-standalone-mail.source`, with
/// the mock's ephemeral port filled in — the *account*, and not the identity
/// or the transport that recipe also writes, because nothing here sends.
///
/// Kept as a literal for the reason the other two tests give: a change to the
/// documented recipe should fail this test loudly rather than quietly
/// retargeting it. `jmap-mail`'s `recipe.rs` is what holds the documented
/// files to what they claim to mean.
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional test\n\
         Enabled=true\n\
         \n\
         [Mail Account]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

#[test]
fn camel_opens_the_store_and_serves_the_inbox() {
    let client = required_path("JMAP_FUNCTIONAL_MAIL_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // Three mailboxes with their JMAP roles and two messages in the inbox —
    // the same shape `jmap-mockd` seeds for the manual recipe, written out
    // here so the assertions below name what they are asserting.
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");

        let inbox = account.seed_mailbox("Inbox", Some("inbox"));
        account.seed_mailbox("Sent", Some("sent"));
        account.seed_mailbox("Drafts", Some("drafts"));

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

    // Before the exit status, because this is the observation that says
    // *why* a failure happened. Camel keys its provider table by protocol,
    // so the account's `BackendName`, the one line in `libcameljmap.urls`
    // and the string `camel_provider_module_init` registers are three
    // spellings that have to agree — and when they do not, every later step
    // fails with "no provider available for protocol", a message about the
    // connect that is really about a typo in one of three files.
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

    // The mock's three mailboxes, and only those: the provider builds the
    // tree from one `Mailbox/get`.
    assert_eq!(
        seen.get("folders"),
        Some(&"Drafts,Inbox,Sent"),
        "the folder tree is not the mock's three mailboxes\n{report}"
    );

    // By role rather than by name. Camel asks the store which folder is the
    // inbox and the provider answers from the mailbox's JMAP role, so a
    // provider that ignored roles would hand back whatever it opened first.
    assert_eq!(
        seen.get("inbox-full-name"),
        Some(&"Inbox"),
        "the store's inbox is not the mailbox with the inbox role\n{report}"
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

    // And both whole messages, which is a different request again: the
    // summaries come from `Email/query` and `Email/get`, a body from a blob
    // download that is a plain HTTP GET. Sorted on the client side, so this
    // is the set of bodies and not an order.
    assert_eq!(
        seen.get("message-bodies"),
        Some(&format!("{FIRST_BODY},{SECOND_BODY}").as_str()),
        "the message bodies did not survive the download\n{report}"
    );

    // The other end: what the server was actually asked for. Nothing here is
    // scheduled or refreshed in the background — every call above is
    // synchronous — so these are not a race.
    let calls = server.method_calls();
    for method in ["Mailbox/get", "Email/query", "Email/get"] {
        assert!(
            calls.iter().any(|call| call == method),
            "the provider never asked for {method}; it asked for {calls:?}\n{report}"
        );
    }

    // `create_folder_sync`, driven through the real vtable rather than the
    // plain decision function `jmap-mail`'s own unit tests call directly.
    assert_eq!(
        seen.get("create-folder-name"),
        Some(&"Receipts"),
        "the created folder's full name is not the mailbox name asked for\n{report}"
    );

    // The provider's own claim: Camel's `folder_created` announcement named
    // the same folder the return value did.
    assert_eq!(
        seen.get("folders-after-create"),
        Some(&"Drafts,Inbox,Receipts,Sent"),
        "the store's own listing did not gain the new folder\n{report}"
    );

    // `rename_folder_sync`, driven through the real vtable rather than the
    // plain decision function `manage::rename_folder`. The last component
    // changed under the same (root) parent — the "name the user typed" half
    // of `manage.rs`'s own doc comment, not a drag-and-drop of the folder's
    // existing path encoding.
    assert_eq!(
        seen.get("folders-after-rename"),
        Some(&"Drafts,Inbox,Invoices,Sent"),
        "the store's own listing did not reflect the rename\n{report}"
    );

    // The other end for create, rename and delete together: the client runs
    // to completion — create, then rename, then delete — before any
    // assertion here can look at the mock's state, so there is no point
    // between the calls to catch the mock mid-sequence the way the
    // address-book/calendar removal tests do. What the method log *can*
    // still prove is that all three requests actually reached the server
    // rather than the provider answering out of a purely local cache:
    // exactly three `Mailbox/set` calls, one per write.
    let mailbox_set_calls = calls.iter().filter(|call| *call == "Mailbox/set").count();
    assert_eq!(
        mailbox_set_calls, 3,
        "expected one Mailbox/set each for the create, the rename and the delete; saw {mailbox_set_calls} in {calls:?}\n{report}"
    );

    // `delete_folder_sync`, the mirror image: driven through the real
    // vtable, not the plain decision function `manage::delete_folder` —
    // deleting the folder by its post-rename name, since that is what the
    // client asked to delete.
    assert_eq!(
        seen.get("folders-after-delete"),
        Some(&"Drafts,Inbox,Sent"),
        "the store's own listing did not lose the deleted folder\n{report}"
    );

    // And the other end: the mock's mailbox store actually lost it too, not
    // merely that the provider claimed success — checked under both names,
    // since a rename that silently left a stale copy behind under the old
    // name would still pass a check that only looked for the new one.
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
}
