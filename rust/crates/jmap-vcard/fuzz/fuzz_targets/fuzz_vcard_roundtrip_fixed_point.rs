// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(vcard) = std::str::from_utf8(data) {
        if let Ok(first_card) = jmap_vcard::vcard_to_card(vcard) {
            let second_vcard = jmap_vcard::card_to_vcard(&first_card);
            let second_card = jmap_vcard::vcard_to_card(&second_vcard)
                .expect("second parse of emitted vCard must succeed");

            let third_vcard = jmap_vcard::card_to_vcard(&second_card);
            let third_card = jmap_vcard::vcard_to_card(&third_vcard)
                .expect("third parse of emitted vCard must succeed");

            let fourth_vcard = jmap_vcard::card_to_vcard(&third_card);
            let fourth_card = jmap_vcard::vcard_to_card(&fourth_vcard)
                .expect("fourth parse of emitted vCard must succeed");

            if third_vcard == fourth_vcard {
                assert_eq!(third_card, fourth_card);
            }
        }
    }
});
