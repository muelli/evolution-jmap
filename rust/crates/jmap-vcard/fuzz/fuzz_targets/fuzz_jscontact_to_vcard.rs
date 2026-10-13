// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use jmap_proto::contacts::ContactCard;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(card) = serde_json::from_slice::<ContactCard>(data) {
        let vcard = jmap_vcard::card_to_vcard(&card);
        let _ = jmap_vcard::vcard_to_card(&vcard);
    }
});
