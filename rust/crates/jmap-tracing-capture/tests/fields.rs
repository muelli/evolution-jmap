// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Proves how a captured event records its level and its field values.

use jmap_tracing_capture::capture;
use tracing::Level;

#[test]
fn a_string_field_keeps_its_raw_text_and_other_fields_are_debug_formatted() {
    let ((), captured) = capture(|| {
        tracing::warn!(reason = "no \"quotes\" added", count = 3_u64, ok = true);
    });
    assert_eq!(
        captured,
        [
            (
                Level::WARN,
                "reason".to_owned(),
                "no \"quotes\" added".to_owned()
            ),
            (Level::WARN, "count".to_owned(), "3".to_owned()),
            (Level::WARN, "ok".to_owned(), "true".to_owned()),
        ]
    );
}

#[test]
fn a_message_is_captured_as_the_message_field() {
    let ((), captured) = capture(|| tracing::info!("plain {}", "text"));
    assert_eq!(
        captured,
        [(Level::INFO, "message".to_owned(), "plain text".to_owned())]
    );
}
