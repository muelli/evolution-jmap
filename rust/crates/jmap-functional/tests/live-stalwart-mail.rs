// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mail tests, against a real Stalwart instead of the in-process mock:
//! receive, flag and expunge writes, transfer between folders, the
//! `EmailSubmission` send, and the password prompt.
//!
//! The book, calendar and collection tests cannot replace these tests. The
//! mail client process loads a Camel provider, not a factory daemon of EDS.
//! Thus those tests do not show that Camel finds this provider against a real
//! server.
//!
//! These tests test less than `mail.rs`, for the reason that
//! `live-stalwart-calendar.rs` gives about `calendar.rs`. On a real server, one
//! difference makes a full client run fail. Thus each test does one phase:
//!
//! - `receive` opens the store, lists the folder tree, finds the three
//!   purpose folders by their JMAP role, appends one message and reads it back.
//! - `flags` writes the flags of a message and then expunges it.
//! - `transfer` creates a folder, moves the message into it, copies it back,
//!   and deletes the folder.
//! - The send test sends through the `CamelTransport` to a second seeded
//!   account and waits until the message arrives there. This delivery on the
//!   server is not possible with the mock, because the mock never delivers
//!   from its outbox.
//!
//! The send test has its own client, `tests/functional/transport-live-client.c`.
//! It follows the chain from account to identity to transport, through two
//! uids, as `transport-client.c` does for the mock.
//!
//! ## The password prompt, against real Camel
//!
//! `StoreError::Unauthenticated` in `jmap-mail/src/connect.rs` exists because
//! real Stalwart answers an unauthenticated session request with 200 OK, not
//! with the 401 of RFC 8620. Thus a new account with no stored password asks
//! the user for a password, and does not quietly resolve no primary account.
//! The mock tests do not do the real `REJECTED`/`get_password` cycle of the
//! retry loop of the base `CamelService` class.
//! `camel_prompts_for_a_password_before_a_fresh_account_connects` does that
//! cycle. The `password-prompt` phase of `mail-live-client.c` opens the store
//! first with no password (as for a new account), then with the real password.
//!
//! The method is the same as in the other live tests: `spawn_loopback_proxy`
//! satisfies the loopback-only plaintext rule of `connect_target` in
//! `jmap-backend-core`, and sends the traffic to Stalwart.
//! `JMAP_LIVE_SERVER_REBASE_URLS` rebases the `apiUrl` of the server onto the
//! address of the connection.
//!
//! The password seed is different. The address-book, calendar and collection
//! tests store it on the `ESource`, because an EDS backend gets credentials
//! from EDS. A Camel provider gets its password from the `CamelService`, and
//! does not use the secret store. Thus the password comes in
//! `JMAP_FUNCTIONAL_STORE_PASSWORD` as before, but the client puts it in a
//! different location (see `tests/functional/mail-live-client.c`).
//!
//! ## How to run it
//!
//! Use the same environment as the `live_server_*.rs` tests of the
//! `jmap-*-sync` crates (see `docs/manual-test-live-server.md`): a throwaway
//! `stw seed` account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-mail -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the tests skip
//! and do not fail.

use std::env;
use std::sync::{Mutex, MutexGuard};

use jmap_functional::{
    Keyfile, Session, SourceKind, observations, required_path, spawn_loopback_proxy,
};
use jmap_test_support::unique_suffix;

/// The body of the message that this test puts into the inbox. One line with
/// no MIME structure: the test checks that a blob from a real server decodes
/// to the uploaded text, not that Camel can parse a multipart.
const BODY: &str = "Found on the floor.";

/// The three purpose folders, and their names on a real Stalwart.
///
/// This is the main assertion of these tests. The mock names its mailboxes
/// "Inbox", "Trash" and "Junk". Thus `mail.rs` cannot tell a store that finds
/// `get_trash_folder_sync` by the JMAP `role` from a store that finds a folder
/// named "Trash". Stalwart names the same roles "Deleted Items" and "Junk
/// Mail", thus only the lookup by role gives the correct answer.
const INBOX_NAME: &str = "Inbox";
const TRASH_NAME: &str = "Deleted Items";
const JUNK_NAME: &str = "Junk Mail";

/// The five mailboxes of a new `stw seed` Stalwart account.
const DEFAULT_FOLDERS: [&str; 5] = [
    "Deleted Items",
    "Drafts",
    "Inbox",
    "Junk Mail",
    "Sent Items",
];

/// The names of the sent and drafts folders on a real Stalwart. The send test
/// must open the sent folder to find the copy of the transport. A `CamelStore`
/// has no by-role getter for these two (it has one only for inbox, trash and
/// junk). Thus the test gives these names to the client.
const SENT_NAME: &str = "Sent Items";
const DRAFTS_NAME: &str = "Drafts";

/// The body of the message that the send test sends. It is different from
/// [`BODY`], so that a message from a different test cannot satisfy a delivery
/// assertion.
const SEND_BODY: &str = "Posted from the corner box.";

/// The three sources of the send chain. These are also their file names, as
/// in `transport.rs`: `IdentityUid` and `TransportUid` are these strings. The
/// client gets only the first of the three.
const SENDER_UID: &str = "jmap-functional";
const IDENTITY_UID: &str = "jmap-functional-identity";
const TRANSPORT_UID: &str = "jmap-functional-transport";

/// The account source of the recipient: a second real account, not a second
/// file of the sender.
const RECIPIENT_UID: &str = "jmap-functional-recipient";

/// The name of the sender. The server matches `Identity/get` only on the
/// address, but the name goes into the `From` header.
const SENDER_NAME: &str = "Agent Sender";

/// `(origin, user, password)`, or `None` to skip. The same as
/// `live_server_params` in `live-stalwart-calendar.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// The keyfile of `mail.rs`, with a real `User=`. This field causes
/// `jmap-mail::connect::password_credentials` to use Basic, not no credentials.
fn keyfile(port: u16, user: &str) -> String {
    Keyfile::new(SourceKind::MailAccount, port)
        .display_name("JMAP functional live-Stalwart mail test")
        .user(user)
        .to_string()
}

/// `(user, password)` for the second account that the send test delivers to,
/// or `None` to skip. The variables and the skip behavior are the same as in
/// `jmap-mail-sync/tests/live_server_send.rs`.
fn recipient_params() -> Option<(String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    Some((user, password))
}

/// The account source of the sender for the send test: [`keyfile`] plus the
/// `IdentityUid` line where the chain starts.
fn sender_account(port: u16, user: &str) -> String {
    keyfile(port, user).replace(
        "BackendName=jmap\n",
        &format!("BackendName=jmap\nIdentityUid={IDENTITY_UID}\n"),
    )
}

/// The identity: the sender of the mail, and the link to the transport. It
/// names no server, because an identity is not a service.
fn identity(address: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live identity\n\
         Enabled=true\n\
         \n\
         [Mail Identity]\n\
         Name={SENDER_NAME}\n\
         Address={address}\n\
         \n\
         [Mail Submission]\n\
         TransportUid={TRANSPORT_UID}\n"
    )
}

/// The transport: a second `CamelService` with its own server. The
/// `[Authentication]` group is a copy of the group of the account. This is
/// necessary for the reason in `transport.rs`: nothing copies a server between
/// two standalone sources.
fn transport(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live transport\n\
         Enabled=true\n\
         \n\
         [Mail Transport]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         User={user}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

/// Held for the full duration of each test in this file.
///
/// The tests use one throwaway account, and each test counts the inbox:
/// `receive` asserts that the listing increased by its append, `flags` that it
/// decreased by its expunge, and `transfer` that a move removed its message.
/// The send test writes to the Sent folder of the same account. Cargo runs
/// the tests of one binary on threads. Without this lock, the appends
/// interleave and a count can be incorrect by one for a reason not related to
/// the provider.
static ONE_ACCOUNT_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The guard. A poisoned lock is used as a normal lock, because the poison
/// only shows that a different test failed.
fn exclusive_account() -> MutexGuard<'static, ()> {
    ONE_ACCOUNT_AT_A_TIME
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// The scratch session of the client, with a loopback proxy to the real
/// server.
///
/// `root` is different for each test. Thus the scratch trees (the XDG
/// directories, the `.source` keyfile, the private D-Bus socket) do not
/// overlap, but the account is the same.
fn live_session(root: &str, origin: &str, user: &str, password: &str) -> Session {
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(root);
    session.write_source("jmap-functional", &keyfile(port, user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", password);
    // The session document gives the configured `apiUrl`, not the proxy
    // address. See the comment on this line in `live-stalwart-book.rs`.
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_camel_provider(&module, &urls);

    session
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn camel_opens_the_store_and_serves_a_real_inbox() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server mail leg"
        );
        return;
    };
    let _account = exclusive_account();
    let client = required_path("JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT");
    let session = live_session(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/live-stalwart-mail"),
        &origin,
        &user,
        &password,
    );

    let subject = format!("agent-fnmail-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(&client, &["jmap-functional", &subject, "receive"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check this before the exit status, for the reason in `mail.rs`. The
    // `BackendName` of the account, the line in `libcameljmap.urls` and the
    // string that `camel_provider_module_init` registers must be the same. If not,
    // each later step fails with "no provider available for protocol".
    assert_eq!(
        seen.get("protocol"),
        Some(&"jmap"),
        "the source names a protocol the provider does not register\n{report}"
    );
    assert_eq!(
        seen.get("store-connected"),
        Some(&"1"),
        "Camel never opened the store against the real server\n{report}"
    );

    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    // Not an equality. `mail.rs` starts a new mock for each run, but a
    // throwaway account keeps folders from earlier runs. Thus the test asserts
    // only that the five default folders of a real account are present.
    let folders = seen
        .get("folders")
        .unwrap_or_else(|| panic!("the client reported no folder listing\n{report}"));
    for expected in DEFAULT_FOLDERS {
        assert!(
            folders.split(',').any(|folder| folder == expected),
            "the folder tree is missing the real server's {expected:?}; it holds {folders:?}\n{report}"
        );
    }

    // The three by-role lookups (see the comment of `TRASH_NAME`). A provider
    // that finds these folders by name, not by JMAP role, passes against the mock
    // and fails here.
    assert_eq!(
        seen.get("inbox-full-name"),
        Some(&INBOX_NAME),
        "the store's inbox is not the real server's mailbox with the inbox role\n{report}"
    );
    assert_eq!(
        seen.get("trash-full-name"),
        Some(&TRASH_NAME),
        "the store's trash folder is not the real server's mailbox with the trash role\n{report}"
    );
    assert_eq!(
        seen.get("junk-full-name"),
        Some(&JUNK_NAME),
        "the store's junk folder is not the real server's mailbox with the junk role\n{report}"
    );

    // Append one message with `Email/import` of an uploaded blob, and read it
    // back. The summary and the body are two different requests: the subject
    // comes from `Email/query` plus `Email/get`, and the body from a blob
    // download, which is an HTTP GET, not a method call.
    assert!(
        seen.get("append-uid").is_some_and(|uid| !uid.is_empty()),
        "append_message_sync did not mint a uid for the appended message\n{report}"
    );

    let count_before = parsed(&seen, "inbox-count-before", &report);
    let count_after = parsed(&seen, "inbox-count-after", &report);
    assert_eq!(
        count_after,
        count_before + 1,
        "the real server's inbox listing did not grow by exactly the appended message\n{report}"
    );
    assert_eq!(
        seen.get("appended-in-listing"),
        Some(&"1"),
        "the appended message's uid is not in the folder's own uid list\n{report}"
    );
    assert_eq!(
        seen.get("appended-subject"),
        Some(&subject.as_str()),
        "re-fetching the appended message by its minted uid did not return the message that went up\n{report}"
    );
    assert_eq!(
        seen.get("appended-body"),
        Some(&BODY),
        "the appended message's body did not survive the round trip through the real server\n{report}"
    );
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn camel_writes_flags_and_expunges_through_the_real_server() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server mail flags leg"
        );
        return;
    };
    let _account = exclusive_account();
    let client = required_path("JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT");
    let session = live_session(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/live-stalwart-mail-flags"),
        &origin,
        &user,
        &password,
    );

    let subject = format!("agent-fnflag-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(&client, &["jmap-functional", &subject, "flags"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("store-connected"),
        Some(&"1"),
        "Camel never opened the store against the real server\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    // `Email/import` of a blob with no keywords. The test asserts this because
    // the two writes below are measured as differences from here. If the server
    // added its own keywords, the test could not tell a flag that the user set
    // from a flag that was already there.
    assert_eq!(
        seen.get("seen-after-append"),
        Some(&"0"),
        "the appended message arrived already marked read\n{report}"
    );
    assert_eq!(
        seen.get("flagged-after-append"),
        Some(&"0"),
        "the appended message arrived already marked important\n{report}"
    );

    // The Camel summary row after `synchronize_sync`. This check alone is weak,
    // because the client wrote this row. But it shows the difference between a
    // write that did not get to the server and a write that Camel did not try.
    assert_eq!(
        seen.get("seen-local-after-set"),
        Some(&"1"),
        "the summary row lost the read mark across the synchronise\n{report}"
    );
    assert_eq!(
        seen.get("flagged-local-after-set"),
        Some(&"1"),
        "the summary row lost the important mark across the synchronise\n{report}"
    );

    // The main check of this test. A second `CamelStore` in its own scratch tree
    // has no summary database. Thus these values come from `Email/get` on the
    // real server. Only this shows if Stalwart stored the keyword, or if only
    // Camel kept it locally.
    assert_eq!(
        seen.get("reopened-set-listed"),
        Some(&"1"),
        "a store opening this account afresh does not find the appended message at all\n{report}"
    );
    assert_eq!(
        seen.get("reopened-set-seen"),
        Some(&"1"),
        "the read mark never reached the real server\n{report}"
    );
    assert_eq!(
        seen.get("reopened-set-flagged"),
        Some(&"1"),
        "the important mark never reached the real server\n{report}"
    );

    // The other direction uses a different `Email/set` patch (the keyword is set
    // to null, not to true). A provider that only adds keywords passes the first
    // check but fails this check.
    assert_eq!(
        seen.get("reopened-cleared-seen"),
        Some(&"0"),
        "clearing the read mark never reached the real server\n{report}"
    );
    assert_eq!(
        seen.get("reopened-cleared-flagged"),
        Some(&"0"),
        "clearing the important mark never reached the real server\n{report}"
    );

    // Read the two counters after an expunge: the uid listing for the message
    // list, and `camel_folder_summary_count` for the unread/total column of the
    // folder tree. If a provider removes the row from only one, the folder shows
    // a message that nobody can open.
    let listed_before = parsed(&seen, "inbox-count-before-expunge", &report);
    let listed_after = parsed(&seen, "inbox-count-after-expunge", &report);
    let summary_before = parsed(&seen, "inbox-summary-before-expunge", &report);
    let summary_after = parsed(&seen, "inbox-summary-after-expunge", &report);
    assert_eq!(
        listed_after,
        listed_before - 1,
        "the inbox listing did not shrink by exactly the expunged message\n{report}"
    );
    assert_eq!(
        summary_after,
        summary_before - 1,
        "the folder summary count did not shrink by exactly the expunged message\n{report}"
    );
    assert_eq!(
        seen.get("expunged-still-listed"),
        Some(&"0"),
        "the expunged message is still in the folder's own uid list\n{report}"
    );

    // The rows go when the expunge occurs, not at the next listing (see
    // `expunge.rs`). Thus the two checks above also pass if a provider only
    // removed the message locally. This check opens the store again and asks the
    // server.
    assert_eq!(
        seen.get("reopened-expunged-listed"),
        Some(&"0"),
        "the expunged message is still on the real server\n{report}"
    );
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn camel_moves_and_copies_a_message_between_real_folders() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server mail transfer leg"
        );
        return;
    };
    let _account = exclusive_account();
    let client = required_path("JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT");
    let session = live_session(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/live-stalwart-mail-transfer"),
        &origin,
        &user,
        &password,
    );

    // One string is the subject of the message and the name of the scratch
    // folder. Each must only be unique for each run.
    let subject = format!("agent-fnxfer-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(&client, &["jmap-functional", &subject, "transfer"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("store-connected"),
        Some(&"1"),
        "Camel never opened the store against the real server\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    // The scratch folder belongs to this run: it is created at the account root
    // with the unique subject as its name. Thus it must be empty. The inbox
    // counts are differences, because the throwaway account keeps old messages.
    assert_eq!(
        seen.get("created-folder"),
        Some(&subject.as_str()),
        "create_folder_sync reported a different full name than the folder it was asked for\n{report}"
    );
    assert_eq!(
        parsed(&seen, "count-scratch-before", &report),
        0,
        "a folder created moments ago under a name nobody else uses is not empty\n{report}"
    );

    // The move. RFC 8621 gives an `Email` one immutable id for each account, and
    // a move only patches its `mailboxIds`. Thus the vfunc must report the same
    // uid that went in. `mail.rs` asserts the same against the mock.
    let append_uid = seen
        .get("append-uid")
        .filter(|uid| !uid.is_empty())
        .unwrap_or_else(|| panic!("append_message_sync did not mint a uid\n{report}"));
    assert_eq!(
        seen.get("moved-uid"),
        Some(append_uid),
        "the move reported a different uid than the message it moved was appended under\n{report}"
    );

    let count_before = parsed(&seen, "inbox-count-before", &report);
    assert_eq!(
        parsed(&seen, "count-inbox-after-move", &report),
        count_before,
        "the inbox listing did not shrink back by exactly the moved message\n{report}"
    );
    assert_eq!(
        seen.get("holds-inbox-after-move"),
        Some(&"0"),
        "the inbox still lists the message after it was moved out\n{report}"
    );
    assert_eq!(
        parsed(&seen, "count-scratch-after-move", &report),
        1,
        "the destination folder does not hold exactly the moved message\n{report}"
    );
    assert_eq!(
        seen.get("holds-scratch-after-move"),
        Some(&"1"),
        "the destination folder's listing does not name the moved message's uid\n{report}"
    );
    assert_eq!(
        seen.get("moved-subject"),
        Some(&subject.as_str()),
        "the destination folder's summary row is not the message that was moved\n{report}"
    );

    // The same checks, from a store that has not seen this account. Its answers
    // come from `Email/query` on the real server, not from a summary database
    // that the move wrote.
    assert_eq!(
        seen.get("reopened-moved-inbox-listed"),
        Some(&"0"),
        "a store opening this account afresh still finds the moved message in the inbox\n{report}"
    );
    assert_eq!(
        seen.get("reopened-moved-folder-listed"),
        Some(&"1"),
        "a store opening this account afresh does not find the message in the destination folder\n{report}"
    );
    assert_eq!(
        seen.get("reopened-moved-subject"),
        Some(&subject.as_str()),
        "the message a fresh store finds in the destination folder is not the one that was moved\n{report}"
    );

    // The copy back, with `delete_originals=FALSE`. This is a different
    // `Email/set` patch: it adds a member to `mailboxIds` and removes nothing.
    // After it, one message is in two folders with one uid.
    assert_eq!(
        seen.get("copied-uid"),
        Some(append_uid),
        "the copy reported a different uid than the message it copied\n{report}"
    );
    assert_eq!(
        parsed(&seen, "count-inbox-after-copy", &report),
        count_before + 1,
        "the inbox listing did not grow back by exactly the copied message\n{report}"
    );
    assert_eq!(
        seen.get("holds-inbox-after-copy"),
        Some(&"1"),
        "the inbox listing does not name the copied message's uid\n{report}"
    );
    assert_eq!(
        parsed(&seen, "count-scratch-after-copy", &report),
        1,
        "the copy did not leave the original in the folder it was copied from\n{report}"
    );
    assert_eq!(
        seen.get("reopened-copied-inbox-listed"),
        Some(&"1"),
        "the copy never reached the real server's inbox\n{report}"
    );
    assert_eq!(
        seen.get("reopened-copied-folder-listed"),
        Some(&"1"),
        "the copy took the original with it on the real server\n{report}"
    );

    // The cleanup move puts the message into a folder that already contains it.
    // The move and the copy above cannot make this `mailboxIds` patch. The inbox
    // must list the message one time, not two times.
    assert_eq!(
        parsed(&seen, "count-inbox-after-cleanup", &report),
        count_before + 1,
        "moving a message into a folder that already holds it duplicated its row\n{report}"
    );
    assert_eq!(
        parsed(&seen, "count-scratch-after-cleanup", &report),
        0,
        "the cleanup move did not empty the scratch folder\n{report}"
    );

    // At the end, the account is as this run found it: the empty folder is
    // destroyed and the message is expunged. The cleanup move must come first,
    // because a JMAP server refuses to destroy a mailbox with email in it
    // (`mailboxHasEmail`).
    assert_eq!(
        seen.get("deleted-folder-listed"),
        Some(&"0"),
        "the store's own listing still names the deleted folder\n{report}"
    );
    assert_eq!(
        parsed(&seen, "final-inbox-count", &report),
        count_before,
        "the run did not leave the inbox where it found it\n{report}"
    );
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn camel_sends_through_a_real_transport_and_a_second_account_receives_it() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server send leg"
        );
        return;
    };
    let Some((recipient_user, recipient_password)) = recipient_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the real-server send leg"
        );
        return;
    };
    let _account = exclusive_account();
    let client = required_path("JMAP_FUNCTIONAL_TRANSPORT_LIVE_CLIENT");

    // The three sources of the chain and the account of the recipient, all
    // behind one proxy: three `[Authentication]` groups, one server.
    // `live_session` writes only the one-account keyfile, thus this test makes
    // the tree here.
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");
    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-mail-send"
    ));
    session.write_source(SENDER_UID, &sender_account(port, &user));
    session.write_source(IDENTITY_UID, &identity(&user));
    session.write_source(TRANSPORT_UID, &transport(port, &user));
    session.write_source(RECIPIENT_UID, &keyfile(port, &recipient_user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_FUNCTIONAL_RECIPIENT_PASSWORD", &recipient_password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_camel_provider(&module, &urls);

    let subject = format!("agent-fnsend-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(
        &client,
        &[
            SENDER_UID,
            RECIPIENT_UID,
            &recipient_user,
            &subject,
            SENT_NAME,
            DRAFTS_NAME,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the chain first, in the same order as `transport.rs`. The client got
    // only the account uid of the sender for the chain. Thus an incorrect link
    // here causes each later failure.
    assert_eq!(
        seen.get("identity-uid"),
        Some(&IDENTITY_UID),
        "the account does not name the identity\n{report}"
    );
    assert_eq!(
        seen.get("identity-address"),
        Some(&user.as_str()),
        "the identity does not carry the address the account sends as\n{report}"
    );
    assert_eq!(
        seen.get("transport-uid"),
        Some(&TRANSPORT_UID),
        "the identity's submission extension does not name the transport\n{report}"
    );
    assert_eq!(
        seen.get("protocol"),
        Some(&"jmap"),
        "the transport source names a protocol the provider does not register\n{report}"
    );

    // The transport slot of the registered provider, connected with Basic
    // authentication to a server that checks the password. The mock test does
    // not do this part of the connect.
    assert_eq!(
        seen.get("transport-connected"),
        Some(&"1"),
        "Camel never connected the transport against the real server\n{report}"
    );

    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("sent"),
        Some(&"1"),
        "the send did not report success\n{report}"
    );
    // A seeded Stalwart account has a drafts role and a sent role. For such an
    // account, the answer is TRUE. If the answer is 0, Evolution appends a second
    // copy to Sent.
    assert_eq!(
        seen.get("sent-copy-saved"),
        Some(&"1"),
        "the transport did not claim the sent copy it left in Sent\n{report}"
    );
    assert_eq!(
        seen.get("transport-disconnected"),
        Some(&"1"),
        "the transport did not let go of its connection\n{report}"
    );

    // Read the sent copy through a store that has not seen this account. Thus
    // the filing comes from the real server, not from a summary row of the send.
    // The copy must be in Sent and not in Drafts. The server does the staging
    // and the move with `onSuccessUpdateEmail` (RFC 8621 section 7.5) when it
    // accepts the submission. A copy in Drafts shows that the server did not
    // accept the submission.
    assert_eq!(
        seen.get("sent-copy-listed"),
        Some(&"1"),
        "the sender's real Sent folder does not hold the sent copy\n{report}"
    );
    assert_eq!(
        seen.get("sent-copy-draft"),
        Some(&"0"),
        "the sent copy still carries the draft mark the staging gave it\n{report}"
    );
    assert_eq!(
        seen.get("staged-copy-listed"),
        Some(&"0"),
        "the sent copy is still sitting in the Drafts it was staged in\n{report}"
    );

    // Delivery, the main check of this test: the message is in the inbox of the
    // SECOND account, found by a `CamelStore` that authenticates as that account,
    // with its initial headers and body. The outbox of the mock is only a list.
    assert_eq!(
        seen.get("delivered-listed"),
        Some(&"1"),
        "the message never arrived in the recipient's real inbox\n{report}"
    );
    assert_eq!(
        seen.get("delivered-subject"),
        Some(&subject.as_str()),
        "the delivered message is not the one that was sent\n{report}"
    );
    let delivered_from = seen
        .get("delivered-from")
        .unwrap_or_else(|| panic!("the client reported no delivered-from\n{report}"));
    assert!(
        delivered_from.contains(&user),
        "the delivered message's From is not the identity's address: {delivered_from:?}\n{report}"
    );
    assert_eq!(
        seen.get("delivered-body"),
        Some(&SEND_BODY),
        "the delivered message's body did not survive the trip\n{report}"
    );

    // At the end, the two accounts are as the run found them. The client lists
    // the folders again to check the expunges.
    assert_eq!(
        seen.get("recipient-cleaned"),
        Some(&"1"),
        "the delivered message survived the recipient's expunge\n{report}"
    );
    assert_eq!(
        seen.get("sender-cleaned"),
        Some(&"1"),
        "the sent copy survived the sender's expunge\n{report}"
    );
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn camel_prompts_for_a_password_before_a_fresh_account_connects() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server password-prompt leg"
        );
        return;
    };
    let _account = exclusive_account();
    let client = required_path("JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT");
    let session = live_session(
        concat!(
            env!("CARGO_TARGET_TMPDIR"),
            "/live-stalwart-mail-password-prompt"
        ),
        &origin,
        &user,
        &password,
    );

    let subject = format!("agent-fnpwd-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(&client, &["jmap-functional", &subject, "password-prompt"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("protocol"),
        Some(&"jmap"),
        "the source names a protocol the provider does not register\n{report}"
    );

    // The main check of this test: a new account with no stored password does
    // not connect. The retry loop of real Camel decides this. The client counts
    // the `get_password` calls of the loop.
    assert_eq!(
        seen.get("unauthenticated-connect-succeeded"),
        Some(&"0"),
        "an account with no stored password connected to the real server anyway\n{report}"
    );
    assert!(
        seen.get("unauthenticated-password-prompts")
            .and_then(|count| count.parse::<u32>().ok())
            .is_some_and(|count| count >= 1),
        "Camel's own retry loop never asked for a password on the unauthenticated attempt\n{report}"
    );
    assert_eq!(
        seen.get("unauthenticated-error-domain"),
        Some(&"camel-service-error"),
        "the unauthenticated attempt failed outside CAMEL_SERVICE_ERROR, not through the ordinary authentication-required path\n{report}"
    );

    // The same account must connect with the password that a real
    // credentials-required prompt stores.
    assert_eq!(
        seen.get("authenticated-connect-succeeded"),
        Some(&"1"),
        "the same account did not connect once a password was stored for it\n{report}"
    );
    assert_eq!(
        seen.get("store-connected"),
        Some(&"1"),
        "the service did not report itself connected after the authenticated attempt\n{report}"
    );

    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );
}

/// One observation as a number. Panics with the name of the observation if it
/// is missing or not a number.
fn parsed(seen: &std::collections::BTreeMap<&str, &str>, key: &str, report: &str) -> u32 {
    seen.get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("the client reported no parseable {key}\n{report}"))
}
