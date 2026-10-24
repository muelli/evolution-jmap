// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The backend name is the word in every `.source` file and in
//! `libcameljmap.urls`. A change breaks each existing account.

use jmap_backend_core::BACKEND_NAME;

#[test]
fn the_backend_name_is_jmap() {
    assert_eq!(BACKEND_NAME, c"jmap");
}
