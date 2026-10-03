// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Item 82 batch 1: the mail leg's receive half, driven against a real
//! Stalwart instead of the in-process mock.
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
//! rather than just mismeasure a field. This is the receive half only -- open
//! the store, list the folder tree, resolve the three purpose folders by
//! their JMAP role, put one message in and read it back. The write-side
//! vfuncs are item 82's later batches.
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

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn camel_opens_the_store_and_serves_a_real_inbox() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server mail leg"
        );
        return;
    };
    let client = required_path("JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/live-stalwart-mail"));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    // Stalwart's session document states its own configured `apiUrl`, not the
    // loopback proxy address it was actually asked through -- see
    // `live-stalwart-book.rs`'s own comment on this line for the mechanism.
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_camel_provider(&module, &urls);

    let subject = format!("agent-fnmail-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(&client, &["jmap-functional", &subject]);
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

/// One reported observation as a number, or a panic naming which one was
/// missing. Two counters are read this way and a parse spelled out twice
/// reads as if the two were different.
fn parsed(seen: &std::collections::BTreeMap<&str, &str>, key: &str, report: &str) -> u32 {
    seen.get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("the client reported no parseable {key}\n{report}"))
}
