// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A card with no explicit file-as gets one derived from its name.
//!
//! Evolution's contact editor refuses to enable Save after an edit to any
//! field but the name itself (and Company) unless File under is non-empty
//! (`sensitize_ok` in `e-contact-editor.c`). `jmap_vcard::card_to_vcard`
//! only ever states `X-EVOLUTION-FILE-AS` for a group card's own name or an
//! explicit `fileAs`, so a plain JMAP-sourced contact loads with it blank,
//! and a user who edits, say, its categories without first touching the
//! name can never save that edit. Defaulting it here, before rendering,
//! gives such a card the same starting point a contact created inside
//! Evolution itself would have had. The derived value must differ from the
//! card's `full` name string, never equal it: a GUI scenario that finds the
//! Contact Editor's Full Name field by its current content (`text_field` in
//! `ci/gui-scenario/contact_photo.py`) would otherwise find the file-as
//! combo's own entry just as well, and silently edit the wrong one.

mod common;

use common::Fixture;

#[test]
fn a_card_with_no_file_as_gets_one_derived_from_its_full_name() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Cara Categories", "cara@example.com");

    let (_, contacts) = fixture.sync().list_existing().unwrap();

    assert!(
        contacts[0]
            .vcard
            .contains("X-EVOLUTION-FILE-AS:Categories\\, Cara"),
        "vcard: {}",
        contacts[0].vcard
    );
}
