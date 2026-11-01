// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(response) = serde_json::from_slice::<jmap_proto::response::Response>(data) {
        if let Ok(encoded) = serde_json::to_vec(&response) {
            let _ = serde_json::from_slice::<jmap_proto::response::Response>(&encoded);
        }
    }
});
