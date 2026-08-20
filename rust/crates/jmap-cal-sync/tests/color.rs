// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::set_color` against the mock server: the whole of what a
//! calendar-colour write-back means, minus the vfunc/diff bookkeeping.

mod common;

use common::Fixture;

#[test]
fn set_color_reaches_the_server() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    sync.set_color(Some("#00ff00")).unwrap();

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color.as_deref(), Some("#00ff00"));
}

#[test]
fn set_color_of_none_clears_it() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    sync.set_color(Some("#00ff00")).unwrap();

    sync.set_color(None).unwrap();

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color, None);
}
