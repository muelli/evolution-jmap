// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 179(a), finding F3: a real Evolution showed bob's shared
//! folders in `folder-tree.json` with the right `account_id`, but no children
//! under "Shared/bob@...". `folders.rs`'s `Request` filters the answer to the
//! subscribed folders when Camel asks with `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`
//! (the account's "Show only subscribed folders" setting, or the default
//! Evolution sends while first building a `CamelSubscribable` store's tree).
//!
//! This test checks the one real-server fact that decides whether that filter
//! hides a shared folder's children: what `isSubscribed` the owner's own
//! `Mailbox/get` gives for a share, seen from the grantee's side.
//! `notes/SHARED-MAILBOXES-2026-10-30.md` (157 (a)) already found a grantee
//! often cannot change it. If it comes back `false`, as it does on Stalwart
//! (confirmed live: grantee's own `Mailbox/get` returns the owner's own
//! subscription flag, not anything about the grantee's wish to see it), then
//! [`FolderTree::merge_shared`] must not let that flag decide whether the
//! folder is visible, the same "show what the server grants, regardless of
//! `isSubscribed`" rule ROADMAP item 163 already applies to address books and
//! calendars.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` (the owner) and
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` (the grantee), then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_shared_folder_tree_visibility -- --ignored
//! ```
//!
//! `JMAP_LIVE_SERVER_URL` is rewritten to a loopback proxy before either
//! account connects (see [`route_through_loopback`]). `jmap-client`'s
//! `execute_within` only attaches `Authorization` to a TLS or loopback URL;
//! the internal, non-loopback plain-http test server this runner reaches
//! would otherwise get every request unauthenticated and silently answered
//! as the server's anonymous session (confirmed live; no redirect is
//! involved, only a same-origin plain connection that the account settings
//! already accepted, same as `jmap-client/tests/plaintext_endpoints.rs`'s
//! own `a_plain_http_session_keeps_endpoints_on_its_own_origin`. That test
//! only checks `connect()` does not refuse the session; it does not check
//! that a later call actually carries `Authorization`, which is the gap
//! this test works around rather than fixes: `jmap-client` is a different
//! lane's own crate).

use std::net::{TcpListener, TcpStream};

use jmap_mail_sync::MailSync;
use jmap_proto::mail::Mailbox;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_MAIL, CAPABILITY_PRINCIPALS};
use jmap_test_support::{connect_owner, connect_recipient_with_user, unique_suffix};
use serde_json::json;

/// Forwards a loopback port to `target`, so a plain-http, non-loopback test
/// server looks like a loopback one to `jmap-client`'s credential check.
/// Returns the proxy's own `http://127.0.0.1:<port>` origin.
fn loopback_proxy_to(target: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback proxy port");
    let port = listener.local_addr().expect("a bound local address").port();
    let target = target.to_owned();
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(incoming) = incoming else { continue };
            let target = target.clone();
            std::thread::spawn(move || relay(incoming, &target));
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// Copies both directions of one proxied connection. Stops silently when
/// either side closes, the shape `jmap-functional::spawn_loopback_proxy`
/// already uses for the same purpose.
fn relay(mut incoming: TcpStream, target: &str) {
    let Ok(mut outgoing) = TcpStream::connect(target) else {
        return;
    };
    let (Ok(mut incoming_reader), Ok(mut outgoing_writer)) =
        (incoming.try_clone(), outgoing.try_clone())
    else {
        return;
    };
    let upstream =
        std::thread::spawn(move || std::io::copy(&mut incoming_reader, &mut outgoing_writer));
    let _ = std::io::copy(&mut outgoing, &mut incoming);
    let _ = upstream.join();
}

/// `target:port` from an `http://host:port` origin, as [`TcpStream::connect`] wants it.
fn host_port(origin: &str) -> String {
    origin
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_owned()
}

/// Points `JMAP_LIVE_SERVER_URL` at a loopback proxy of its current value,
/// once per process. A second call is a no-op: the env var already names
/// the proxy, and proxying a proxy would still work but serves nothing.
///
/// Also forces `JMAP_LIVE_SERVER_REBASE_URLS=1`: the session's `apiUrl` and
/// friends name Stalwart's own configured hostname, not the proxy, so every
/// call after the first would miss the proxy and fail to resolve unless the
/// client rewrites them back to the origin it actually connected through.
fn route_through_loopback() {
    let Ok(real) = std::env::var("JMAP_LIVE_SERVER_URL") else {
        return;
    };
    if real.starts_with("http://127.0.0.1:") || real.starts_with("http://[::1]:") {
        return;
    }
    let proxied = loopback_proxy_to(&host_port(&real));
    // SAFETY: this test runs single-threaded within its own process; no
    // other thread reads or writes these variables concurrently.
    unsafe {
        std::env::set_var("JMAP_LIVE_SERVER_URL", proxied);
        std::env::set_var("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    }
}

/// A shared mailbox must appear to the grantee's folder tree regardless of
/// the owner's own `isSubscribed` value for it: the same visibility rule
/// ROADMAP item 163 already applies to shared address books and calendars.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_mailbox_is_visible_in_the_folder_tree_regardless_of_the_owners_subscription() {
    route_through_loopback();
    let Some((owner, _owner_address)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared folder tree visibility test"
        );
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient_with_user() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the shared folder tree visibility test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the shared folder tree visibility test"
        );
        return;
    }

    let owner_account_id = owner
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_address),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner cannot resolve the recipient's principal id by email");

    let suffix = unique_suffix();
    let mailbox = owner
        .mailbox_create(
            &owner_account_id,
            // `isSubscribed` left at its server default: the point of this
            // test is that the grantee's view must not depend on it.
            &Mailbox::new(format!("agent-mailsync-shared-vis-{suffix}")),
        )
        .expect("Mailbox/set create failed against the real server");
    let mailbox_id = mailbox
        .id
        .clone()
        .expect("the server named the new mailbox");

    owner
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true}}}),
        )
        .expect("Mailbox/set shareWith failed against the real server");

    let recipient_account_id = recipient
        .primary_account(CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");
    let recipient_sync = MailSync::new(recipient, recipient_account_id);

    let (_, tree) = recipient_sync
        .folder_tree_with_shared()
        .expect("folder_tree_with_shared failed against the real server");
    let merged = tree
        .by_id(&mailbox_id)
        .expect("the shared mailbox should merge into the recipient's own folder tree");
    assert!(
        merged.subscribed,
        "a shared folder must stay visible to Evolution's \"only subscribed\" filter \
         regardless of the owner's own isSubscribed value for it"
    );

    owner
        .mailbox_destroy(&owner_account_id, &mailbox_id)
        .expect("Mailbox/set destroy failed against the real server (cleanup)");
}
