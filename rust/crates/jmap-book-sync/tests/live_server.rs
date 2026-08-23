// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`/`remove_contact` against a real JMAP server —
//! the sync-layer functions `EBookMetaBackendSync::save_contact_sync`/
//! `remove_contact_sync` actually call, exercised end to end for the first
//! time.
//!
//! `jmap-client/tests/live_server.rs` already proves `ContactCard/set`
//! round-trips against real Stalwart directly through `Client`, but nothing
//! there drives `BookSync` itself — the vCard-to-`ContactCard` mapping
//! (`jmap_vcard::vcard_to_card`) and the create/update decision `save_contact`
//! makes. Only `jmap-mockd` has ever exercised this crate's own functions
//! (`jmap-book-sync/tests/save.rs`); this file is their live-server
//! counterpart, following the same recipe as `jmap-cal-sync/tests/
//! live_server.rs`.
//!
//! ## Running it
//!
//! Same environment as `jmap-cal-sync`'s live-server test — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up for
//! `jmap-client`'s write-path tests:
//!
//! ```console
//! $ cargo test -p evolution-jmap-book-sync -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like `jmap-cal-sync`,
//! this crate has no such feature, and `#[ignore]` alone already keeps it
//! out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment.

use std::env;

use jmap_book_sync::BookSync;
use jmap_client::{Client, Credentials};
use jmap_proto::session::CAPABILITY_CONTACTS;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover contact can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server.rs::connect_for_write` exactly.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// Saves a new vCard via `BookSync::save_contact`, confirms it via
/// `list_existing`, edits it (a name change, mirroring what Evolution sends
/// on a contact rename), confirms the edit via `load_contact`, then removes
/// it via `remove_contact` and confirms it is gone.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn saving_then_removing_a_contact_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let address_book_id = client
        .address_books(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the address book");

    let sync = BookSync::new(client, account_id, address_book_id);

    let name = format!("agent-booksync-{}", unique_suffix());
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");
    assert_ne!(
        saved.uid, "pas-id-not-a-server-id",
        "the locally invented UID must not be sent as the JMAP id"
    );
    assert!(
        saved.vcard.contains(&name),
        "the created card should carry the name we sent: {}",
        saved.vcard
    );

    let (_, existing) = sync.list_existing().expect("listing the book failed");
    assert!(
        existing.iter().any(|contact| contact.uid == saved.uid),
        "the newly created card should be listed in its address book"
    );

    let new_name = format!("{name}-renamed");
    let edited_vcard = vcard.replacen(&name, &new_name, 2);
    let updated = sync
        .save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");
    assert_eq!(updated.uid, saved.uid, "an edit must not change the id");
    let reloaded = sync
        .load_contact(&saved.uid)
        .expect("loading the edited card failed");
    assert!(
        reloaded.vcard.contains(&new_name),
        "the edit should be visible on reload: {}",
        reloaded.vcard
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");
    let (_, remaining) = sync
        .list_existing()
        .expect("listing the book failed after removal");
    assert!(
        !remaining.iter().any(|contact| contact.uid == saved.uid),
        "the removed card should no longer be listed"
    );
}
