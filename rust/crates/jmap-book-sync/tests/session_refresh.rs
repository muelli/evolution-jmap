// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A sync pass refreshes a session the server already marked stale
//! (ROADMAP item 166, row C1-26).

mod common;
use common::Fixture;

#[test]
fn get_changes_refreshes_a_stale_session_before_it_syncs() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();

    {
        let state_lock = fixture.server.state();
        state_lock.lock().unwrap().session_state += 1;
    }
    // One call sees the new sessionState and marks the session stale.
    sync.client().echo(serde_json::json!({})).unwrap();
    assert!(sync.client().is_session_stale());

    sync.get_changes(&state).unwrap();

    assert!(
        !sync.client().is_session_stale(),
        "get_changes must refresh a stale session before it syncs"
    );
}
