// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(changes) = serde_json::from_slice::<jmap_proto::methods::ChangesResponse>(data) {
        if let Ok(encoded) = serde_json::to_vec(&changes) {
            let _ = serde_json::from_slice::<jmap_proto::methods::ChangesResponse>(&encoded);
        }
    }
    if let Ok(query_changes) =
        serde_json::from_slice::<jmap_proto::methods::QueryChangesResponse>(data)
    {
        if let Ok(encoded) = serde_json::to_vec(&query_changes) {
            let _ = serde_json::from_slice::<jmap_proto::methods::QueryChangesResponse>(&encoded);
        }
    }
});
