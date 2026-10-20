// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! This test compiles a catalogue and finds it through the binding that the
//! modules make.
//!
//! The other i18n test checks that the domain is bound to the directory that
//! the build chose. That half is easy. It stays true with a wrong directory
//! layout, because `bindtextdomain` records a string and reads no files. This
//! test checks the half that only a real lookup can show. Gettext must open
//! the `.mo` at `<dir>/<language>/LC_MESSAGES/<domain>.mo` (the path that an
//! install rule must make). [`translate`] must return its contents.
//!
//! ## Why this is a test binary of its own
//!
//! The test touches three process-global items, none of them per-thread: the
//! binding of the domain, the `LANGUAGE` environment variable, and the locale.
//! A sibling test that binds the domain to the *installed* directory would
//! race this test for the same glibc state. Cargo gives each file in `tests/`
//! its own process, so this file alone gives the isolation.
//!
//! ## Why there is no `msgfmt`
//!
//! `.mo` is a fixed binary layout: a header, two sorted tables of
//! (length, offset), and a string blob. The code to write it here is
//! approximately twenty lines. A call to `msgfmt` would make the test need the
//! installed gettext tools. Such a test passes on a developer machine and
//! disappears in a container.
//!
//! ## What it cannot always check
//!
//! glibc reads no catalogue while `LC_MESSAGES` is the `C` (or `POSIX`, or
//! `C.UTF-8`) locale, and returns the message unchanged. On a machine with no
//! other locale, the lookup has nothing to find, so the test asserts *that*
//! contract. The test prints which of the two ran, because the difference is
//! important in a log.
//!
//! [`translate`]: jmap_backend_core::i18n::translate

use std::ffi::{CStr, CString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use jmap_backend_core::i18n::{DOMAIN, bind_to, translate, translate_with};

/// The message the catalogue translates, and what it translates it to.
///
/// The translation contains non-ASCII text intentionally. Gettext returns the
/// bytes of the `.mo`, and the binding configures the codeset conversion. Thus
/// valid UTF-8 in the result shows that the conversion did not corrupt it.
const MSGID: &CStr = c"For reading and storing mail on JMAP servers.";
const MSGSTR: &str = "Zum Lesen und Speichern von E-Mail auf JMAP-Servern.";

/// A message whose arguments the translation puts in the other order.
///
/// This is why the placeholders have numbers and are not each a bare `%s`.
/// Word order changes between languages. A translator who cannot move the
/// arguments must choose between a natural sentence and a correct one.
///
/// This entry sorts before [`MSGID`] (`%` is below `F`). The `.mo` format
/// needs this order, because there is no hash table to find an entry.
const ORDERED_MSGID: &CStr = c"%1$s could not be stated in %2$s";
const ORDERED_MSGSTR: &str = "In %2$s ließ sich %1$s nicht ausdrücken";

/// The language the catalogue is filed under.
///
/// This is intentionally not a real language. The test selects it through
/// `LANGUAGE`, which glibc reads before the locale. Thus no locale of this
/// name is necessary. None must exist, or the test would depend on the
/// locales that the machine generated.
const LANGUAGE: &str = "xx";

fn main_test_dir() -> PathBuf {
    std::env::temp_dir().join(format!("jmap-catalogue-{}", std::process::id()))
}

/// Writes a catalogue where gettext looks for one.
///
/// Returns the directory for the domain binding. This is the root of the
/// tree, not the directory of the file, because `bindtextdomain` takes the
/// root.
fn install_catalogue(root: &Path) -> PathBuf {
    let dir = root.join(LANGUAGE).join("LC_MESSAGES");
    fs::create_dir_all(&dir).expect("a directory for the catalogue");
    let name = DOMAIN.to_str().expect("an ASCII domain");
    fs::write(dir.join(format!("{name}.mo")), compile_mo()).expect("a catalogue on disk");
    root.to_path_buf()
}

/// The entries of the catalogue, as a `.mo` file.
///
/// The header entry (msgid `""`) is necessary. Its msgstr declares the
/// charset. Without it, gettext does not know the encoding of the
/// translations and does not convert them to the encoding of the binding.
///
/// The header also sorts first, as the format needs. With no hash table (`0`
/// in both hash fields), gettext does a binary search of the original table.
/// Thus the entries must be in ascending msgid order.
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
    // Little-endian magic. All other fields use the byte order of the magic,
    // and gettext reads both byte orders.
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

/// Writes one table entry (length and offset of its string) and adds the
/// string to the blob that the offsets point into.
fn record(table: &mut Vec<u8>, blob: &mut Vec<u8>, strings_at: u32, s: &str) {
    table.extend_from_slice(&(s.len() as u32).to_le_bytes());
    table.extend_from_slice(&(strings_at + blob.len() as u32).to_le_bytes());
    blob.extend_from_slice(s.as_bytes());
    // The recorded length excludes the NUL, but gettext gives the pointer
    // as a C string.
    blob.push(0);
}

/// Sets `LC_MESSAGES` to a locale that can translate, if this machine has one.
///
/// The locale of the environment is first, so a configured machine is tested
/// as it is. Then some usual names follow. `C`, `POSIX` and `C.UTF-8` never
/// count. glibc treats all three as "this program is not localised" and
/// returns each msgid unchanged.
fn a_locale_that_can_translate() -> Option<String> {
    let candidates = [c"", c"en_US.UTF-8", c"en_US.utf8", c"C.UTF-8"];
    for candidate in candidates {
        // SAFETY: a C string. This is the only thread that touches the
        // locale, because the module is alone in its test binary.
        let set = unsafe { libc::setlocale(libc::LC_MESSAGES, candidate.as_ptr()) };
        if set.is_null() {
            continue;
        }
        // SAFETY: a non-NULL return from setlocale is a C string that glibc
        // owns. The code copies it before anything can rebind it.
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

    // SAFETY: single-threaded. This is the only test in the binary, and it
    // runs before anything else can look up a message.
    unsafe { std::env::set_var("LANGUAGE", LANGUAGE) };

    let dir = CString::new(bound.as_os_str().as_bytes()).expect("a path without a NUL");
    let reported = bind_to(&dir);
    assert_eq!(reported.as_c_str(), dir.as_c_str());

    match translating {
        Some(locale) => {
            println!("catalogue lookup exercised under locale {locale}");
            assert_eq!(translate(MSGID), MSGSTR);
            // The translation moved the arguments, and the arguments went to
            // their new positions. Numbered placeholders exist for this. Only
            // a real catalogue can show it: with no `.mo`, the template is the
            // msgid and the order never changes.
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

    // Best effort: a panic in the teardown does not help a failed test.
    let _ = fs::remove_dir_all(&root);
}
