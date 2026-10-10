// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(vcard) = std::str::from_utf8(data) {
        if let Ok(first_card) = jmap_vcard::vcard_to_card(vcard) {
            let second_vcard = jmap_vcard::card_to_vcard(&first_card);
            if let Ok(second_card) = jmap_vcard::vcard_to_card(&second_vcard) {
                assert_eq!(first_card, second_card);
            }
        }
    }
});
