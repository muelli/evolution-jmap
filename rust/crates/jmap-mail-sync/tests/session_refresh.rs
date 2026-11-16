// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A sync pass refreshes a session the server already marked stale
//! (ROADMAP item 166, row C1-26).

use jmap_proto::mail::role;

mod common;
use common::Fixture;

#[test]
fn folder_tree_since_refreshes_a_stale_session_before_it_syncs() {
    let fixture = Fixture::start();
    fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();
    let (state, _) = sync.folder_tree().unwrap();

    {
        let state_lock = fixture.server.state();
        state_lock.lock().unwrap().session_state += 1;
    }
    // One call sees the new sessionState and marks the session stale.
    sync.client().echo(serde_json::json!({})).unwrap();
    assert!(sync.client().is_session_stale());

    sync.folder_tree_since(&state).unwrap();

    assert!(
        !sync.client().is_session_stale(),
        "folder_tree_since must refresh a stale session before it syncs"
    );
}
