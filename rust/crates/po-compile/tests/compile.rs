// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a translator's `.po` turns into, and what it refuses to turn into.
//!
//! The catalogue is read back here with a reader written for this test rather
//! than with the crate's own code: a compiler checked against its own idea of
//! the format agrees with itself no matter what the format is. The layout the
//! reader assumes is the one in the GNU gettext manual, and the same layout
//! `msgunfmt` reads — `tests/gettext.rs` closes that loop by handing the
//! output to glibc.

use std::collections::BTreeMap;

use po_compile::{Error, compile};

/// A header entry, which every `.po` this project compiles has to carry.
const HEADER: &str = "msgid \"\"\nmsgstr \"\"\n\"Content-Type: text/plain; charset=UTF-8\\n\"\n";

/// The entries of a compiled catalogue, in the order the file stores them.
///
/// Order is not decoration: with no hash table gettext binary-searches the
/// table of originals, so a catalogue whose entries are out of order is one
/// where some lookups silently miss. Returning a `Vec` rather than a map is
/// what lets the test say so.
fn read_mo(mo: &[u8]) -> Vec<(String, String)> {
    let word = |at: usize| -> u32 {
        u32::from_le_bytes(mo[at..at + 4].try_into().expect("four bytes for a word"))
    };

    assert_eq!(word(0), 0x9504_12de, "magic, little-endian");
    assert_eq!(word(4), 0, "format revision");
    let count = word(8) as usize;
    let originals_at = word(12) as usize;
    let translations_at = word(16) as usize;

    let string_at = |table: usize, index: usize| -> String {
        let length = word(table + 8 * index) as usize;
        let offset = word(table + 8 * index + 4) as usize;
        assert_eq!(
            mo[offset + length],
            0,
            "the blob NUL-terminates its strings"
        );
        String::from_utf8(mo[offset..offset + length].to_vec()).expect("UTF-8 in the blob")
    };

    (0..count)
        .map(|index| {
            (
                string_at(originals_at, index),
                string_at(translations_at, index),
            )
        })
        .collect()
}

/// The compiled entries as a map, for the assertions that do not care about
/// order.
fn entries(po: &str) -> BTreeMap<String, String> {
    read_mo(&compile(po).expect("a catalogue"))
        .into_iter()
        .collect()
}

#[test]
fn a_translated_message_reaches_the_catalogue() {
    let mo = entries(&format!(
        "{HEADER}\nmsgid \"For reading and storing mail on JMAP servers.\"\n\
         msgstr \"Zum Lesen und Speichern von E-Mail auf JMAP-Servern.\"\n"
    ));

    assert_eq!(
        mo.get("For reading and storing mail on JMAP servers."),
        Some(&"Zum Lesen und Speichern von E-Mail auf JMAP-Servern.".to_owned())
    );
}

#[test]
fn the_header_is_an_entry_of_its_own() {
    let mo = entries(HEADER);

    assert_eq!(
        mo.get(""),
        Some(&"Content-Type: text/plain; charset=UTF-8\n".to_owned()),
        "the header carries the charset gettext converts from"
    );
}

#[test]
fn an_untranslated_message_is_left_out() {
    let mo = entries(&format!(
        "{HEADER}\nmsgid \"not translated yet\"\nmsgstr \"\"\n"
    ));

    assert!(
        !mo.contains_key("not translated yet"),
        "an empty msgstr must not become the translation: gettext would hand \
         back an empty string where falling through to the English is right"
    );
}

#[test]
fn a_fuzzy_message_is_left_out_but_a_fuzzy_header_is_not() {
    let po = format!("#, fuzzy\n{HEADER}\n#, fuzzy\nmsgid \"guessed at\"\nmsgstr \"geraten\"\n");
    let mo = entries(&po);

    assert!(
        !mo.contains_key("guessed at"),
        "a fuzzy entry is a translator's unfinished work, not a translation"
    );
    assert!(
        mo.contains_key(""),
        "the header is fuzzy in every freshly initialised catalogue, and \
         dropping it would take the charset with it"
    );
}

#[test]
fn an_obsolete_entry_is_left_out() {
    let mo = entries(&format!(
        "{HEADER}\n#~ msgid \"was in an older version\"\n#~ msgstr \"veraltet\"\n"
    ));

    assert!(!mo.contains_key("was in an older version"));
}

#[test]
fn entries_are_written_in_ascending_order() {
    let po = format!(
        "{HEADER}\n\
         msgid \"second\"\nmsgstr \"zweite\"\n\n\
         msgid \"first\"\nmsgstr \"erste\"\n"
    );

    let written: Vec<String> = read_mo(&compile(&po).expect("a catalogue"))
        .into_iter()
        .map(|(original, _)| original)
        .collect();

    assert_eq!(
        written,
        ["", "first", "second"],
        "gettext binary-searches this table; out of order is a silent miss"
    );
}

#[test]
fn a_message_may_be_written_across_several_lines() {
    let mo = entries(&format!(
        "{HEADER}\n\
         msgid \"\"\n\"This event repeats until %1$s, and the time zone \"\n\"it is in is not known.\"\n\
         msgstr \"\"\n\"Der Termin wiederholt sich bis %1$s, und die Zeitzone \"\n\"ist unbekannt.\"\n"
    ));

    assert_eq!(
        mo.get("This event repeats until %1$s, and the time zone it is in is not known."),
        Some(&"Der Termin wiederholt sich bis %1$s, und die Zeitzone ist unbekannt.".to_owned()),
        "the pieces are concatenated with nothing between them"
    );
}

#[test]
fn the_escapes_a_catalogue_uses_are_decoded() {
    let mo = entries(&format!(
        "{HEADER}\nmsgid \"a \\\"quoted\\\" word\"\nmsgstr \"ein \\\"zitiertes\\\" Wort\\n\"\n"
    ));

    assert_eq!(
        mo.get("a \"quoted\" word"),
        Some(&"ein \"zitiertes\" Wort\n".to_owned())
    );
}

#[test]
fn an_escape_this_compiler_does_not_know_is_refused() {
    let refusal = compile(&format!("{HEADER}\nmsgid \"x\"\nmsgstr \"\\q\"\n"))
        .expect_err("an unknown escape is not a translation");

    assert!(
        matches!(refusal, Error::UnknownEscape { line: 6, .. }),
        "the refusal names the line: {refusal}"
    );
}

#[test]
fn a_catalogue_without_a_header_is_refused() {
    let refusal = compile("msgid \"x\"\nmsgstr \"y\"\n").expect_err("no header, no charset");

    assert!(matches!(refusal, Error::NoHeader), "{refusal}");
}

#[test]
fn a_catalogue_in_another_charset_is_refused() {
    let refusal =
        compile("msgid \"\"\nmsgstr \"\"\n\"Content-Type: text/plain; charset=ISO-8859-1\\n\"\n")
            .expect_err("this compiler reads and writes UTF-8 and nothing else");

    assert!(matches!(refusal, Error::Charset { .. }), "{refusal}");
}

#[test]
fn a_catalogue_that_declares_no_charset_is_refused() {
    let refusal = compile("msgid \"\"\nmsgstr \"Project-Id-Version: evolution-jmap\\n\"\n")
        .expect_err("without a charset gettext cannot convert the translations");

    assert!(matches!(refusal, Error::Charset { .. }), "{refusal}");
}

#[test]
fn a_string_that_never_ends_is_refused() {
    let refusal = compile(&format!(
        "{HEADER}\nmsgid \"x\"\nmsgstr \"unterminated\\\"\n"
    ))
    .expect_err("the closing quote was escaped, so the string ran off the line");

    assert!(
        matches!(refusal, Error::Unparsed { line: 6, .. }),
        "{refusal}"
    );
}

#[test]
fn the_same_message_translated_twice_is_refused() {
    let refusal = compile(&format!(
        "{HEADER}\nmsgid \"x\"\nmsgstr \"eins\"\n\nmsgid \"x\"\nmsgstr \"zwei\"\n"
    ))
    .expect_err("which of the two would be the translation?");

    assert!(matches!(refusal, Error::Duplicate { .. }), "{refusal}");
}

#[test]
fn a_construct_this_compiler_does_not_implement_is_refused_by_name() {
    for construct in [
        "msgctxt \"menu\"\n",
        "msgid_plural \"xs\"\n",
        "msgstr[0] \"x\"\n",
    ] {
        let refusal = compile(&format!("{HEADER}\n{construct}"))
            .expect_err("silently dropping it would lose a translation");

        assert!(
            matches!(refusal, Error::Unsupported { .. }),
            "{construct} was accepted, or refused as something else: {refusal}"
        );
    }
}

#[test]
fn a_line_that_is_not_part_of_an_entry_is_refused() {
    let refusal = compile(&format!("{HEADER}\nmsgid \"x\"\nmsgstr \"y\"\nnonsense\n"))
        .expect_err("a typo in a catalogue must not be skipped over");

    assert!(
        matches!(refusal, Error::Unparsed { line: 7, .. }),
        "{refusal}"
    );
}

#[test]
fn a_message_with_no_translation_line_at_all_is_refused() {
    let refusal = compile(&format!("{HEADER}\nmsgid \"x\"\n"))
        .expect_err("half an entry is a broken catalogue, not an untranslated one");

    assert!(matches!(refusal, Error::NoMsgstr { .. }), "{refusal}");
}
