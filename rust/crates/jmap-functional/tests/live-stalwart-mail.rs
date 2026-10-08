// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Item 82 batches 1 to 4: the mail leg driven against a real Stalwart
//! instead of the in-process mock -- its receive half, the flag and expunge
//! writes, the transfer between folders, and the `EmailSubmission` send.
//!
//! The fourth live-server leg, after the address book, the calendar and the
//! collection account, and the one the other three cannot stand in for: a
//! Camel provider is dlopened by the mail client's own process rather than by
//! a factory daemon EDS ships, so everything the other legs prove about
//! `.source` keyfiles and EDS factories says nothing about whether Camel
//! would ever have found this provider against a real server.
//!
//! Deliberately narrower than `mail.rs`'s own mock-based leg, for the reason
//! `live-stalwart-calendar.rs` gives about `calendar.rs`: that one run also
//! subscribes and unsubscribes, flags a message, creates, renames and deletes
//! a folder, transfers a message out and back, and expunges, and a real
//! server genuinely differing on any one of those would fail the whole client
//! rather than just mismeasure a field. So the legs here are split one batch
//! at a time instead: batch 1 opens the store, lists the folder tree,
//! resolves the three purpose folders by their JMAP role, puts one message in
//! and reads it back; batch 2 writes to a message's flags and then expunges
//! it; batch 3 creates a folder of its own, moves the message into it, copies
//! it back, and deletes the folder again; batch 4 sends through the
//! `CamelTransport` to a second seeded account and watches the message
//! actually arrive there -- intra-server delivery, the one claim the mock
//! structurally cannot back, because its outbox is a list it never delivers
//! from. The send leg has a client of its own,
//! `tests/functional/transport-live-client.c`: what it walks (account to
//! identity to transport, through two uids) is the mock leg's
//! `transport-client.c` chain, not `mail-live-client.c`'s store preamble.
//!
//! ## Item 90: the password prompt, against real Camel
//!
//! `jmap-mail/src/connect.rs`'s `StoreError::Unauthenticated` (finding 12 in
//! `STALWART-RFC-FINDINGS.md`: real Stalwart answers an unauthenticated
//! session request 200 OK instead of RFC 8620's own 401) exists so that a
//! fresh account with no stored password still ends up asking the user for
//! one, instead of silently resolving no primary account. Every test proving
//! that so far uses the mock, which never answers with a real `REJECTED`/
//! `get_password` round trip the way the base `CamelService` class's own
//! retry loop does. `camel_prompts_for_a_password_before_a_fresh_account_connects`
//! drives that loop for real: `mail-live-client.c`'s `password-prompt` phase
//! opens the account's own store first with no password anywhere (the state
//! a brand new account is in) and then again with the real one.
//!
//! The mechanism is unchanged from the other three legs:
//! `spawn_loopback_proxy` satisfies `jmap-backend-core::connect_target`'s
//! plaintext-stays-loopback rule honestly while the traffic actually
//! continues to Stalwart, and `JMAP_LIVE_SERVER_REBASE_URLS` rebases the
//! server's stated `apiUrl` onto the address actually connected through.
//!
//! The one piece that does *not* carry over is the password seed. The
//! address-book, calendar and collection legs store it on the `ESource`,
//! because an EDS backend asks EDS for credentials; a Camel provider takes
//! its password off the `CamelService` its session put it on, and never
//! consults the secret store at all. So the password reaches this leg through
//! `JMAP_FUNCTIONAL_STORE_PASSWORD` as before, but the client seeds it
//! somewhere else -- see `tests/functional/mail-live-client.c`.
//!
//! ## Running it
//!
//! Same environment as the `jmap-*-sync` crates' own `live_server_*.rs`
//! tests -- see `docs/manual-test-live-server.md`: a throwaway `stw seed`
//! account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-mail -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;
use std::sync::{Mutex, MutexGuard};

use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};

/// The body of the message this test puts into the inbox. A single line with
/// no MIME structure to it: what is being measured is whether a blob
/// downloaded off a real server decodes back to what went up, not whether
/// Camel can walk a multipart.
const BODY: &str = "Found on the floor.";

/// The three purpose folders, and what a real Stalwart calls them.
///
/// This is the assertion this whole leg exists for. The mock seeds mailboxes
/// named exactly "Inbox", "Trash" and "Junk", so `mail.rs` cannot tell a
/// store that answers `get_trash_folder_sync` from the mailbox's JMAP `role`
/// apart from one that just went looking for a folder called "Trash".
/// Stalwart names the same two roles "Deleted Items" and "Junk Mail", so here
/// the two answers differ and only the by-role one is right.
const INBOX_NAME: &str = "Inbox";
const TRASH_NAME: &str = "Deleted Items";
const JUNK_NAME: &str = "Junk Mail";

/// The five mailboxes a freshly seeded Stalwart account carries, confirmed by
/// hand against a `stw seed`-ed principal before this test was written.
const DEFAULT_FOLDERS: [&str; 5] = [
    "Deleted Items",
    "Drafts",
    "Inbox",
    "Junk Mail",
    "Sent Items",
];

/// Where a real Stalwart's sent and drafts roles live, by name. The send leg
/// has to open the sent folder to find the copy the transport left, and a
/// `CamelStore` has no by-role getter for either (inbox, trash and junk are
/// the three it has) -- so these two names are harness constants handed to
/// the client, pinned here the way `TRASH_NAME` and `JUNK_NAME` are.
const SENT_NAME: &str = "Sent Items";
const DRAFTS_NAME: &str = "Drafts";

/// The body of the message the send leg posts. Its own string rather than
/// [`BODY`] so a delivery assertion can never be satisfied by a message some
/// other leg left behind.
const SEND_BODY: &str = "Posted from the corner box.";

/// The three sources of the send leg's chain, which are also their file
/// names -- `transport.rs`'s own convention: `IdentityUid` and `TransportUid`
/// are these strings, and the client is handed only the first of the three.
const SENDER_UID: &str = "jmap-functional";
const IDENTITY_UID: &str = "jmap-functional-identity";
const TRANSPORT_UID: &str = "jmap-functional-transport";

/// The recipient's account source: a second real account, not a second file
/// of the sender's.
const RECIPIENT_UID: &str = "jmap-functional-recipient";

/// Who the sender claims to be. Only the address matters to the server --
/// `Identity/get` is matched on it -- but the name rides the `From` header,
/// so it is pinned too.
const SENDER_NAME: &str = "Agent Sender";

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never mistakes a previous run's message for this
/// one's. Mirrors every `jmap-*-sync` live-server test's own copy.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)`, or `None` to skip -- mirrors
/// `live-stalwart-calendar.rs`'s own `live_server_params`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// `mail.rs`'s own keyfile, with a real `User=` added -- the one field that
/// turns `jmap-mail::connect::password_credentials`'s no-credentials branch
/// into the Basic one.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart mail test\n\
         Enabled=true\n\
         \n\
         [Mail Account]\n\
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

/// `(user, password)` for the second account the send leg delivers to, or
/// `None` to skip -- the same two variables, and the same skip-not-fail
/// shape, as `jmap-mail-sync/tests/live_server_send.rs` established.
fn recipient_params() -> Option<(String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    Some((user, password))
}

/// The sender's account source for the send leg: [`keyfile`] plus the
/// `IdentityUid` line the chain walk starts from.
fn sender_account(port: u16, user: &str) -> String {
    keyfile(port, user).replace(
        "BackendName=jmap\n",
        &format!("BackendName=jmap\nIdentityUid={IDENTITY_UID}\n"),
    )
}

/// The identity: who the mail is from, and where the chain turns towards the
/// transport. It names no server of its own -- an identity is not a service.
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

/// The transport: a second `CamelService` with a server of its own. The
/// `[Authentication]` group is a second copy of the account's and has to be,
/// for the reason `transport.rs` gives: nothing copies a server between two
/// standalone sources.
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

/// Held for the length of every test in this file.
///
/// The legs share one throwaway account, and every one of them counts the
/// inbox: batch 1 claims the listing grew by exactly its own append, batch 2
/// that it shrank by exactly its own expunge, batch 3 that a move emptied it
/// of exactly its own message, and batch 4 writes to the same account's Sent
/// folder. Cargo runs the tests in one binary on
/// threads, so without this the appends interleave and any count can be off
/// by one for a reason that has nothing to do with the provider. Serialising
/// them is cheaper and more honest than making each claim weak enough to
/// survive the others.
static ONE_ACCOUNT_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The guard, with a poisoned lock treated as an ordinary one: the poison
/// only means the other leg already failed, which this one still wants to be
/// measured independently of.
fn exclusive_account() -> MutexGuard<'static, ()> {
    ONE_ACCOUNT_AT_A_TIME
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// The scratch session both legs run their client in, pointed at the real
/// server through a loopback proxy of its own.
///
/// `root` differs per leg so the two scratch trees -- the XDG directories,
/// the `.source` keyfile, the private D-Bus socket -- never overlap even
/// though the account behind them is shared.
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
    // Stalwart's session document states its own configured `apiUrl`, not the
    // loopback proxy address it was actually asked through -- see
    // `live-stalwart-book.rs`'s own comment on this line for the mechanism.
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

    // Before the exit status, for the reason `mail.rs` gives: the account's
    // `BackendName`, the one line in `libcameljmap.urls` and the string
    // `camel_provider_module_init` registers are three spellings that have to
    // agree, and when they do not every later step fails with "no provider
    // available for protocol" -- a message about the connect that is really
    // about a typo in one of three files.
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

    // Not an equality: unlike the mock, which `mail.rs` starts fresh every
    // run, a throwaway account outlives the run that seeded it, and a later
    // batch of item 82 will leave folders of its own behind. The claim is
    // that a real account's five defaults are all there -- same reasoning as
    // `live-stalwart-calendar.rs`'s own `events-after` check.
    let folders = seen
        .get("folders")
        .unwrap_or_else(|| panic!("the client reported no folder listing\n{report}"));
    for expected in DEFAULT_FOLDERS {
        assert!(
            folders.split(',').any(|folder| folder == expected),
            "the folder tree is missing the real server's {expected:?}; it holds {folders:?}\n{report}"
        );
    }

    // The three by-role lookups, and the reason this leg is worth running at
    // all: see `TRASH_NAME`'s own comment. A provider that answered any of
    // these by name rather than by the mailbox's JMAP role passes against the
    // mock and fails right here.
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

    // One message in, over `Email/import` of an uploaded blob, and read back
    // out -- summary and body, which are two different requests: a subject
    // comes from `Email/query` plus `Email/get`, a body from a blob download
    // that is a plain HTTP GET rather than a method call.
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

    // `Email/import` of a blob with no keywords on it. Asserted rather than
    // merely reported because the two writes below are both measured as
    // differences from here: a server that handed its own keywords to an
    // imported message would make "the flag the user set arrived" and "the
    // flag was already there" indistinguishable.
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

    // What `synchronize_sync` left in Camel's own summary row. Weak on its
    // own -- the row is what the client just wrote to -- but it separates a
    // write that never reached the server from one Camel never attempted.
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

    // The claim this leg exists for. A second `CamelStore` on a scratch tree
    // of its own has no summary database to answer from, so these two come
    // off the real server's `Email/get` -- which is the only way to tell a
    // keyword Stalwart stored from one Camel only ever remembered locally.
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

    // And the other direction, which is a different `Email/set` patch
    // (a keyword set to null rather than to true) and the half a provider
    // that wrote keywords additively would still pass the first check on.
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

    // Item 82 names the folder-summary counts after an expunge as one of the
    // divergences worth hunting, so both counters are read: the uid listing
    // the message list is drawn from, and `camel_folder_summary_count`, which
    // is what the folder tree's unread/total column asks. A provider that
    // dropped the row from one and not the other leaves a folder claiming a
    // message nobody can open.
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

    // The rows go when the expunge does rather than at the next listing
    // (`expunge.rs`'s own "the rows go now" note), so the two checks above
    // would both pass against a provider that only forgot the message
    // locally. This one reopens and asks the server.
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

    // One string names both the message's subject and the scratch folder:
    // each only has to be unique per run, and two differently-derived values
    // would just be two ways to misspell the same nonce.
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

    // The scratch folder is this run's own: created at the account root under
    // the unique subject, so it has to come back empty -- unlike the inbox,
    // whose counts are differences because the throwaway account outlives any
    // one run.
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

    // The move. RFC 8621 gives an `Email` one immutable id per account and a
    // move only patches its `mailboxIds`, so the uid the vfunc reports is the
    // uid that went in -- the claim `mail.rs` makes against the mock, now
    // against a server that actually stores the thing.
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

    // The same filing question put to a store that has never seen this
    // account: its answers come off the real server's `Email/query`, not out
    // of any summary database the move itself wrote to.
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

    // The copy back, `delete_originals=FALSE`: a different `Email/set` patch
    // (the destination's `mailboxIds` member is added and nothing is taken
    // away), after which one message is in two folders under one uid.
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

    // The cleanup move files the message into a folder that already holds it
    // -- the one `mailboxIds` patch shape the move and the copy above cannot
    // produce -- so the inbox must end up listing it once, not twice.
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

    // And the account ends where this run found it: the emptied folder
    // destroyed (a JMAP server refuses `mailboxHasEmail` otherwise, which is
    // exactly why the cleanup move comes first), the message expunged.
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

    // The chain's three sources plus the recipient's own account, all behind
    // one proxy: four `[Authentication]` groups, one server. `live_session`
    // writes only the plain one-account keyfile, so the tree is built here.
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

    // The chain before anything about the send, `transport.rs`'s own order:
    // the client was handed the sender's account uid and nothing else, so a
    // wrong link here explains every later failure.
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
    // auth against a server that actually checks the password -- the half of
    // the connect the mock leg never exercises.
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
    // A seeded Stalwart account has both a drafts and a sent role, which is
    // the ordinary account and the one whose answer is TRUE: told 0 here,
    // Evolution would append a second copy of its own to Sent.
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

    // The sent copy, read back through a store that has never seen this
    // account -- so the filing is the real server's, not a summary row the
    // send wrote. In Sent and no longer in Drafts, because the staging and
    // the move are RFC 8621 section 7.5's `onSuccessUpdateEmail`, applied by
    // the server as part of accepting the submission: a copy still sitting
    // in Drafts is a submission that was posted but never accepted.
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

    // Delivery, the claim this batch exists for: the message in the SECOND
    // account's inbox, found by a `CamelStore` authenticated as that account,
    // with the headers and body it was composed with. The mock's outbox is a
    // list; this is a letter that actually crossed.
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

    // Both accounts end where the run found them, and the expunges that get
    // them there are themselves measured against a relisting.
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

    // The claim this leg exists for: a fresh account with no stored password
    // never connects, and real Camel's own retry loop is what decides that --
    // not a result this test classifies after the fact, but the loop's own
    // get_password call, counted by the client.
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

    // The same account, now with the password a real credentials-required
    // prompt would have stored, must connect.
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

/// One reported observation as a number, or a panic naming which one was
/// missing. Two counters are read this way and a parse spelled out twice
/// reads as if the two were different.
fn parsed(seen: &std::collections::BTreeMap<&str, &str>, key: &str, report: &str) -> u32 {
    seen.get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("the client reported no parseable {key}\n{report}"))
}
