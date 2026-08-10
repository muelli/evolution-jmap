// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where this project's translated strings come from.
//!
//! Every module in this repository is dlopened into a process it does not own —
//! Evolution's shell, `evolution-source-registry`, whatever is using Camel —
//! and every one of those processes has already set up gettext for *itself*.
//! What none of them has done is tell gettext where *our* catalogue is, because
//! they have never heard of it. That is the whole of this module: one domain
//! name, one directory, and the call that associates them.
//!
//! ## The domain, and who looks a string up in it
//!
//! [`DOMAIN`] is `evolution-jmap`, so the installed catalogue for a language is
//! `<`[`LOCALE_DIR`]`>/<language>/LC_MESSAGES/evolution-jmap.mo`. Two different
//! callers reach into it:
//!
//! - **Camel, on our behalf.** A `CamelProvider` carries a
//!   `translation_domain`, and Camel calls `dgettext` with it when it displays
//!   the provider's name and description. So those two strings are translated
//!   without this crate doing anything — *provided* the domain is bound, which
//!   is why [`bind`] is called from the provider module's entry point rather
//!   than from wherever the first translated string happens to be.
//! - **This code, for text it emits itself** — [`translate`]. Nothing needs it
//!   yet; the account-setup labels and the user-visible parts of Camel error
//!   messages will.
//!
//! ## Why the directory is a build-time input
//!
//! gettext's compiled-in default is `/usr/share/locale`, which is right only
//! when the modules were installed under `/usr`. They need not be: the
//! destinations come from pkg-config and the tree can be staged anywhere, and a
//! module that looked for its catalogue somewhere it was not installed would
//! silently show English. So CMake passes `EVOLUTION_JMAP_LOCALEDIR` to cargo,
//! `build.rs` bakes it in, and a plain `cargo build` falls back to the same
//! `/usr/share/locale` gettext would have used anyway.
//!
//! ## What is *not* here
//!
//! `textdomain()` — the call that changes the *default* domain for the whole
//! process. A loadable module must never make it: the default domain belongs to
//! the program, and taking it would untranslate the host application. Every
//! lookup here names its domain explicitly, which is what `dgettext` is for.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::OnceLock;

unsafe extern "C" {
    fn bindtextdomain(domainname: *const c_char, dirname: *const c_char) -> *mut c_char;
    fn bind_textdomain_codeset(domainname: *const c_char, codeset: *const c_char) -> *mut c_char;
    fn dgettext(domainname: *const c_char, msgid: *const c_char) -> *mut c_char;
}

/// The gettext domain every translated string in this project belongs to.
///
/// Also the basename of the installed catalogue, and the `translation_domain`
/// the Camel provider hands to Camel — the three have to agree or the
/// catalogue is simply never opened, with no diagnostic anywhere.
pub const DOMAIN: &CStr = c"evolution-jmap";

/// The directory the catalogues were installed under.
///
/// `build.rs` resolves it from `EVOLUTION_JMAP_LOCALEDIR` or falls back to
/// gettext's own default, so this is never empty and the `panic!` below is a
/// build-time error about a directory name with a NUL in it, not something a
/// running module can reach.
pub const LOCALE_DIR: &CStr =
    match CStr::from_bytes_with_nul(concat!(env!("EVOLUTION_JMAP_LOCALE_DIR"), "\0").as_bytes()) {
        Ok(dir) => dir,
        Err(_) => panic!("EVOLUTION_JMAP_LOCALEDIR must not contain a NUL byte"),
    };

/// The binding, made at most once per process.
static BOUND: OnceLock<CString> = OnceLock::new();

/// Points gettext at this project's catalogues, and answers with where it
/// says they are.
///
/// Call it from a module's entry point, before anything can ask for a
/// translated string. It is idempotent because a process can load more than one
/// of this repository's modules — Evolution loads the account-setup module and
/// uses Camel's provider in the same address space — and each of them has to
/// assume it might be the first.
///
/// The return value is gettext's account of the binding rather than the
/// argument echoed back, which is the only way to tell a binding that took from
/// one that did not.
pub fn bind() -> &'static CString {
    BOUND.get_or_init(|| bind_to(LOCALE_DIR))
}

/// [`bind`] to a directory of the caller's choosing.
///
/// Separate from `bind` so that a test can prove the mechanism against a
/// catalogue it wrote itself; nothing in the modules should call it. Binding
/// twice to different directories is not an error to gettext — the last call
/// wins — so a second caller would silently move the catalogue out from under
/// the first.
///
/// Safe rather than `unsafe`: the arguments are C strings that outlive the
/// call, glibc copies the directory rather than borrowing it, and it takes its
/// own lock over the binding list. What the caller has to think about is
/// ordering, not memory.
pub fn bind_to(dir: &CStr) -> CString {
    // SAFETY: two C strings, valid for the call, and glibc copies both.
    let bound = unsafe {
        let bound = bindtextdomain(DOMAIN.as_ptr(), dir.as_ptr());
        // The catalogue's own charset is whatever the translators used;
        // asking for UTF-8 makes gettext convert, which is what lets
        // `translate` hand back a Rust `String` without a lossy step that
        // could show up as replacement characters in the account dialog.
        bind_textdomain_codeset(DOMAIN.as_ptr(), c"UTF-8".as_ptr());
        bound
    };

    if bound.is_null() {
        // glibc could not record the binding, which it only fails to do when
        // it is out of memory. Reporting an empty directory rather than the
        // one that was asked for keeps the failure visible: the caller's
        // argument is not evidence that gettext accepted it.
        return CString::default();
    }
    // SAFETY: a non-NULL return is a C string glibc owns and keeps until the
    // domain is rebound; it is copied here and not held.
    unsafe { CStr::from_ptr(bound) }.to_owned()
}

/// The translation of `msgid` for the user's language, or `msgid` itself.
///
/// "Or `msgid` itself" is the normal case rather than the failure case, and
/// covers all of: no catalogue installed for the domain, no catalogue for this
/// language, the message not in the catalogue, and a user whose locale is `C`.
/// gettext makes no distinction between them and neither does this — the
/// untranslated string is a correct answer, and the alternative of reporting an
/// error the caller can do nothing about would only mean an unwritten label.
///
/// Callers must have called [`bind`] first; before that the lookup goes to
/// whatever directory gettext defaults to, which is a different question with a
/// plausible-looking answer.
pub fn translate(msgid: &CStr) -> String {
    // SAFETY: `msgid` is valid for the call. `dgettext` never returns NULL —
    // with nothing to translate it returns the `msgid` pointer it was given —
    // and what it returns is owned by gettext, so it is copied out here.
    unsafe { CStr::from_ptr(dgettext(DOMAIN.as_ptr(), msgid.as_ptr())) }
        .to_string_lossy()
        .into_owned()
}
