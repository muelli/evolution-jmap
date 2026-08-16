// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The compiler's output, read by the program that will actually read it.
//!
//! `tests/compile.rs` checks the bytes against the documented layout, which is
//! a statement about this repository's understanding of the format. This test
//! makes the other statement: glibc's gettext — the exact implementation the
//! installed modules call — opens the file, finds the message, and hands back
//! the translation. Nothing here goes through this project's own `i18n`
//! module, which lives in a crate needing the EDS headers; the three libc
//! functions are declared where they are used, so the check runs in a plain
//! `cargo test`.
//!
//! ## Why it is alone in its file
//!
//! The domain binding, `LANGUAGE`, and the locale are all process-global.
//! Cargo gives each file in `tests/` its own process, and that is the
//! isolation.
//!
//! ## What it cannot always check
//!
//! glibc consults no catalogue at all while `LC_MESSAGES` is `C`, `POSIX` or
//! `C.UTF-8`: it returns the msgid unchanged, by design. On a machine with no
//! other locale generated there is therefore nothing to look up, and the test
//! asserts *that* contract instead — and prints which of the two it ran,
//! because the difference matters when reading a log.

use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::{c_char, c_int};
use std::os::unix::ffi::OsStrExt;

unsafe extern "C" {
    fn bindtextdomain(domainname: *const c_char, dirname: *const c_char) -> *mut c_char;
    fn bind_textdomain_codeset(domainname: *const c_char, codeset: *const c_char) -> *mut c_char;
    fn dgettext(domainname: *const c_char, msgid: *const c_char) -> *mut c_char;
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

/// glibc's `LC_MESSAGES`. Hard-coded rather than pulled in with a dependency
/// on `libc` for one integer; it is fixed by the ABI on the platforms this
/// project builds for.
const LC_MESSAGES: c_int = 5;

/// A domain of this test's own. Not `evolution-jmap`: binding that one here
/// would state where the real catalogues are on the strength of a temporary
/// directory.
const DOMAIN: &CStr = c"po-compile-test";

/// Deliberately not a language anyone speaks — it is selected through
/// `LANGUAGE`, which glibc consults ahead of the locale, so no locale of this
/// name has to exist on the machine.
const LANGUAGE: &str = "xx";

const MSGID: &CStr = c"For reading and storing mail on JMAP servers.";
const MSGSTR: &str = "Zum Lesen und Speichern von E-Mail auf JMAP-Servern.";

const PO: &str = concat!(
    "# German translation of evolution-jmap.\n",
    "#, fuzzy\n",
    "msgid \"\"\n",
    "msgstr \"\"\n",
    "\"Project-Id-Version: evolution-jmap\\n\"\n",
    "\"MIME-Version: 1.0\\n\"\n",
    "\"Content-Type: text/plain; charset=UTF-8\\n\"\n",
    "\"Content-Transfer-Encoding: 8bit\\n\"\n",
    "\n",
    "#: rust/crates/jmap-mail/src/provider.rs:65\n",
    "msgid \"For reading and storing mail on JMAP servers.\"\n",
    "msgstr \"Zum Lesen und Speichern von E-Mail auf JMAP-Servern.\"\n",
);

/// Whether this machine has a locale under which gettext will consult a
/// catalogue at all.
fn a_locale_that_can_translate() -> Option<String> {
    for candidate in [c"", c"en_US.UTF-8", c"en_US.utf8", c"C.UTF-8"] {
        // SAFETY: a C string, and this is the only thread that touches the
        // locale — the test is alone in its binary.
        let set = unsafe { setlocale(LC_MESSAGES, candidate.as_ptr()) };
        if set.is_null() {
            continue;
        }
        // SAFETY: a non-NULL return is a C string glibc owns; it is copied out
        // before anything can change the locale again.
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
fn glibc_reads_what_this_compiler_wrote() {
    let root = std::env::temp_dir().join(format!("po-compile-{}", std::process::id()));
    let dir = root.join(LANGUAGE).join("LC_MESSAGES");
    fs::create_dir_all(&dir).expect("a directory for the catalogue");
    let name = DOMAIN.to_str().expect("an ASCII domain");
    fs::write(
        dir.join(format!("{name}.mo")),
        po_compile::compile(PO).expect("a catalogue"),
    )
    .expect("a catalogue on disk");

    let translating = a_locale_that_can_translate();

    // SAFETY: single-threaded — this is the only test in the binary, and it
    // runs before any lookup.
    unsafe { std::env::set_var("LANGUAGE", LANGUAGE) };

    let bound = CString::new(root.as_os_str().as_bytes()).expect("a path without a NUL");
    // SAFETY: two C strings valid for the call, both of which glibc copies.
    let reported = unsafe {
        let reported = bindtextdomain(DOMAIN.as_ptr(), bound.as_ptr());
        bind_textdomain_codeset(DOMAIN.as_ptr(), c"UTF-8".as_ptr());
        reported
    };
    assert!(!reported.is_null(), "gettext did not record the binding");

    // SAFETY: a NUL-terminated domain and msgid; `dgettext` never returns
    // NULL, and what it returns is gettext's to keep, so it is copied out.
    let translated = unsafe { CStr::from_ptr(dgettext(DOMAIN.as_ptr(), MSGID.as_ptr())) }
        .to_string_lossy()
        .into_owned();

    match translating {
        Some(locale) => {
            println!("catalogue lookup exercised under locale {locale}");
            assert_eq!(translated, MSGSTR);
        }
        None => {
            println!(
                "no non-C locale on this machine: checking that gettext \
                 leaves the message alone instead"
            );
            assert_eq!(translated, MSGID.to_str().expect("an ASCII msgid"));
        }
    }

    // Best effort: a failed test is not improved by a panic in its teardown.
    let _ = fs::remove_dir_all(&root);
}
