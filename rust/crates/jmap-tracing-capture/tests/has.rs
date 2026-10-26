// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Proves `has` finds an exact level/name/value triple and rejects a
//! near-miss on any one of the three fields.

use jmap_tracing_capture::{capture, has};
use tracing::Level;

#[test]
fn has_finds_an_exact_triple_and_rejects_a_near_miss() {
    let ((), captured) = capture(|| tracing::warn!(account = "acc-1", folder = "Inbox"));

    assert!(has(&captured, Level::WARN, "account", "acc-1"));
    assert!(!has(&captured, Level::ERROR, "account", "acc-1"));
    assert!(!has(&captured, Level::WARN, "folder", "acc-1"));
    assert!(!has(&captured, Level::WARN, "account", "acc-2"));
}
