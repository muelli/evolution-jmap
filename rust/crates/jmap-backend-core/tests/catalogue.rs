// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A catalogue this test compiled itself, found through the binding the
//! modules make.
//!
//! The other i18n test says the domain is bound to the directory the build
//! chose. That is the easy half; it would still hold if the directory layout
//! underneath were wrong, because `bindtextdomain` records a string and reads
//! no files. What is checked here is the half that only a real lookup can
//! show: that a `.mo` filed at `<dir>/<language>/LC_MESSAGES/<domain>.mo` — the
//! path an install rule has to produce — is the file gettext opens, and that
//! [`translate`] returns what is in it.
//!
//! ## Why this is a test binary of its own
//!
//! Three of the things it touches are process-global and none of them are
//! per-thread: the domain's binding, the `LANGUAGE` environment variable, and
//! the locale. A sibling test that bound the domain to the *installed*
//! directory would race this one to the same glibc state. Cargo gives each
//! file in `tests/` its own process, so being alone in this file is the
//! isolation.
//!
//! ## Why there is no `msgfmt`
//!
//! `.mo` is a fixed binary layout — a header, two sorted tables of
//! (length, offset), and a string blob — and writing it here costs about
//! twenty lines. Shelling out to `msgfmt` would make the test depend on the
//! gettext tools being installed, which is exactly the kind of thing that
//! makes a test pass on a developer's machine and vanish in a container.
//!
//! ## What it cannot always check
//!
//! glibc refuses to consult any catalogue while `LC_MESSAGES` is the `C` (or
//! `POSIX`, or `C.UTF-8`) locale, and returns the message unchanged — so on a
//! machine with no other locale installed there is nothing for the lookup to
//! find and the test asserts *that* contract instead. Which of the two ran is
//! printed, because the difference matters when reading a log.
//!
//! [`translate`]: jmap_backend_core::i18n::translate

use std::ffi::{CStr, CString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use jmap_backend_core::i18n::{DOMAIN, bind_to, translate, translate_with};

/// The message the catalogue translates, and what it translates it to.
///
/// Non-ASCII on the translated side on purpose: gettext hands back the bytes
/// of the `.mo` and the codeset conversion is part of what the binding sets
/// up, so a translation that survives as UTF-8 says the conversion did not
/// mangle it.
const MSGID: &CStr = c"For reading and storing mail on JMAP servers.";
const MSGSTR: &str = "Zum Lesen und Speichern von E-Mail auf JMAP-Servern.";

/// A message whose arguments the translation puts in the other order.
///
/// This is the whole reason the placeholders are numbered rather than being a
/// bare `%s` each: word order is not a property a sentence keeps across
/// languages, and a translator who cannot move the arguments has to choose
/// between a natural sentence and a correct one. Sorts before [`MSGID`] — `%`
/// is below `F` — which the `.mo` format needs, there being no hash table to
/// find an entry by any other route.
const ORDERED_MSGID: &CStr = c"%1$s could not be stated in %2$s";
const ORDERED_MSGSTR: &str = "In %2$s ließ sich %1$s nicht ausdrücken";

/// The language the catalogue is filed under.
///
/// Deliberately not a language anyone speaks: it is selected through
/// `LANGUAGE`, which glibc consults ahead of the locale, so no locale of this
/// name needs to exist — and none must, or the test would depend on what the
/// machine happens to have generated.
const LANGUAGE: &str = "xx";

fn main_test_dir() -> PathBuf {
    std::env::temp_dir().join(format!("jmap-catalogue-{}", std::process::id()))
}

/// A catalogue, filed where gettext looks for one.
///
/// Returns the directory to bind the domain to — the root of the tree, not the
/// directory the file is in, which is the distinction `bindtextdomain` takes.
fn install_catalogue(root: &Path) -> PathBuf {
    let dir = root.join(LANGUAGE).join("LC_MESSAGES");
    fs::create_dir_all(&dir).expect("a directory for the catalogue");
    let name = DOMAIN.to_str().expect("an ASCII domain");
    fs::write(dir.join(format!("{name}.mo")), compile_mo()).expect("a catalogue on disk");
    root.to_path_buf()
}

/// The entries of the catalogue, as a `.mo` file.
///
/// The header entry — msgid `""` — is not decoration: its msgstr is where the
/// charset is declared, and without it gettext has no idea what encoding the
/// translations are in and will not convert them to the one the binding asked
/// for. It also sorts first, which the format needs: with no hash table
/// (`0` in both hash fields) gettext binary-searches the original table, so
/// the entries have to be in ascending msgid order.
fn compile_mo() -> Vec<u8> {
    let entries: [(&str, &str); 3] = [
        ("", "Content-Type: text/plain; charset=UTF-8\n"),
        (
            ORDERED_MSGID.to_str().expect("an ASCII msgid"),
            ORDERED_MSGSTR,
        ),
        (MSGID.to_str().expect("an ASCII msgid"), MSGSTR),
    ];

    let count = entries.len() as u32;
    // Header, then the two tables of (length, offset), then the strings.
    let strings_at = 28 + 16 * count;

    let mut blob = Vec::new();
    let mut originals = Vec::new();
    let mut translations = Vec::new();
    for (original, _) in &entries {
        record(&mut originals, &mut blob, strings_at, original);
    }
    for (_, translation) in &entries {
        record(&mut translations, &mut blob, strings_at, translation);
    }

    let mut mo = Vec::new();
    // Little-endian magic; the byte order of the magic is the byte order of
    // every other field, and gettext reads either.
    mo.extend_from_slice(&0x9504_12deu32.to_le_bytes());
    mo.extend_from_slice(&0u32.to_le_bytes()); // revision 0
    mo.extend_from_slice(&count.to_le_bytes());
    mo.extend_from_slice(&28u32.to_le_bytes()); // originals table
    mo.extend_from_slice(&(28 + 8 * count).to_le_bytes()); // translations table
    mo.extend_from_slice(&0u32.to_le_bytes()); // hash table size
    mo.extend_from_slice(&0u32.to_le_bytes()); // hash table offset
    mo.extend_from_slice(&originals);
    mo.extend_from_slice(&translations);
    mo.extend_from_slice(&blob);
    mo
}

/// One entry of a table: where its string is, and the string appended to the
/// blob the offsets are into.
fn record(table: &mut Vec<u8>, blob: &mut Vec<u8>, strings_at: u32, s: &str) {
    table.extend_from_slice(&(s.len() as u32).to_le_bytes());
    table.extend_from_slice(&(strings_at + blob.len() as u32).to_le_bytes());
    blob.extend_from_slice(s.as_bytes());
    // The recorded length excludes it, but gettext hands the pointer out as a
    // C string all the same.
    blob.push(0);
}

/// Puts `LC_MESSAGES` somewhere translations are possible at all, if this
/// machine has anywhere to put it.
///
/// The environment's own locale first, so a machine that is already set up is
/// tested as it stands; then a couple of the usual names. `C`, `POSIX` and
/// `C.UTF-8` do not count however they were arrived at — glibc treats all
/// three as "this program is not localised" and returns every msgid unchanged.
fn a_locale_that_can_translate() -> Option<String> {
    let candidates = [c"", c"en_US.UTF-8", c"en_US.utf8", c"C.UTF-8"];
    for candidate in candidates {
        // SAFETY: a C string, and this is the only thread that touches the
        // locale — the module is alone in its test binary.
        let set = unsafe { libc::setlocale(libc::LC_MESSAGES, candidate.as_ptr()) };
        if set.is_null() {
            continue;
        }
        // SAFETY: a non-NULL return from setlocale is a C string glibc owns;
        // it is copied out before anything can rebind it.
        let name = unsafe { CStr::from_ptr(set) }
            .to_string_lossy()
            .into_owned();
        let base = name.split('.').next().unwrap_or(&name);
        if base != "C" && base != "POSIX" {
            return Some(name);
        }
    }
    None
}

#[test]
fn a_catalogue_under_the_bound_directory_is_the_one_gettext_reads() {
    let root = main_test_dir();
    let bound = install_catalogue(&root);

    let translating = a_locale_that_can_translate();

    // SAFETY: single-threaded — this is the only test in the binary, and it
    // runs before anything else can look up a message.
    unsafe { std::env::set_var("LANGUAGE", LANGUAGE) };

    let dir = CString::new(bound.as_os_str().as_bytes()).expect("a path without a NUL");
    let reported = bind_to(&dir);
    assert_eq!(reported.as_c_str(), dir.as_c_str());

    match translating {
        Some(locale) => {
            println!("catalogue lookup exercised under locale {locale}");
            assert_eq!(translate(MSGID), MSGSTR);
            // The translation moved the arguments, and the arguments went
            // where it moved them to — the property numbered placeholders
            // exist for, and one only a real catalogue can show: with no `.mo`
            // to find, the template is the msgid and the order never changes.
            assert_eq!(
                translate_with(ORDERED_MSGID, &["an end date", "Europe/Berlin"]),
                "In Europe/Berlin ließ sich an end date nicht ausdrücken"
            );
        }
        None => {
            println!(
                "no non-C locale on this machine: checking that gettext \
                 leaves the message alone instead"
            );
            assert_eq!(translate(MSGID), MSGID.to_str().expect("an ASCII msgid"));
            assert_eq!(
                translate_with(ORDERED_MSGID, &["an end date", "Europe/Berlin"]),
                "an end date could not be stated in Europe/Berlin"
            );
        }
    }

    // Best effort: a failed test is not improved by a panic in its teardown.
    let _ = fs::remove_dir_all(&root);
}
