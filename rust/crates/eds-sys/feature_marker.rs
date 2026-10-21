// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The marker test of build.rs, in its own file so that tests/feature_marker.rs
// can include it. A build script has no test harness of its own.

/// Tell if `bindings` contains `marker`. The test ignores all whitespace,
/// because rustfmt adds and removes it between tokens.
pub fn has_marker(bindings: &str, marker: &str) -> bool {
    compact(bindings).contains(&compact(marker))
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect()
}
