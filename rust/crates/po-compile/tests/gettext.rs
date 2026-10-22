// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify that glibc gettext reads `.mo` files that this crate writes.

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

const LC_MESSAGES: c_int = 5;

const DOMAIN: &CStr = c"po-compile-test";

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

/// Return one non-C locale. This lets gettext attempt translation.
fn a_locale_that_can_translate() -> Option<String> {
    for candidate in [c"", c"en_US.UTF-8", c"en_US.utf8", c"C.UTF-8"] {
        // SAFETY: This test process is single-threaded.
        let set = unsafe { setlocale(LC_MESSAGES, candidate.as_ptr()) };
        if set.is_null() {
            continue;
        }
        // SAFETY: `set` is non-null and points to glibc-owned C string data.
        // The copied Rust string owns its bytes.
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

    // SAFETY: This test process updates environment state one time.
    unsafe { std::env::set_var("LANGUAGE", LANGUAGE) };

    let bound = CString::new(root.as_os_str().as_bytes()).expect("a path without a NUL");
    // SAFETY: Both C strings are valid, and glibc copies both values.
    let reported = unsafe {
        let reported = bindtextdomain(DOMAIN.as_ptr(), bound.as_ptr());
        bind_textdomain_codeset(DOMAIN.as_ptr(), c"UTF-8".as_ptr());
        reported
    };
    assert!(!reported.is_null(), "gettext did not record the binding");

    // SAFETY: `dgettext` reads valid C strings and returns gettext-owned data.
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

    let _ = fs::remove_dir_all(&root);
}
