// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verifies glibc gettext can read this crate's compiled `.mo` output.
//!
//! Locale and domain bindings are process-global, so this runs in its own
//! binary. If only C or POSIX locales exist, gettext should return `msgid`.

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

/// glibc's `LC_MESSAGES` value.
const LC_MESSAGES: c_int = 5;

/// A test-only gettext domain.
const DOMAIN: &CStr = c"po-compile-test";

/// Synthetic language tag selected through `LANGUAGE`.
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

/// Returns a locale where gettext consults message catalogues.
fn a_locale_that_can_translate() -> Option<String> {
    for candidate in [c"", c"en_US.UTF-8", c"en_US.utf8", c"C.UTF-8"] {
        // SAFETY: this test binary is single-threaded when it runs.
        let set = unsafe { setlocale(LC_MESSAGES, candidate.as_ptr()) };
        if set.is_null() {
            continue;
        }
        // SAFETY: non-NULL points to a glibc-owned C string that is copied.
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

    // SAFETY: this binary contains one test and sets locale-related state once.
    unsafe { std::env::set_var("LANGUAGE", LANGUAGE) };

    let bound = CString::new(root.as_os_str().as_bytes()).expect("a path without a NUL");
    // SAFETY: two C strings valid for the call, both of which glibc copies.
    let reported = unsafe {
        let reported = bindtextdomain(DOMAIN.as_ptr(), bound.as_ptr());
        bind_textdomain_codeset(DOMAIN.as_ptr(), c"UTF-8".as_ptr());
        reported
    };
    assert!(!reported.is_null(), "gettext did not record the binding");

    // SAFETY: `dgettext` sees valid C strings and returns a non-NULL gettext-owned pointer.
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
