// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the translated strings of this project come from.
//!
//! Every module in this repository is dlopened into a process that it does
//! not own: the Evolution shell, `evolution-source-registry`, or a process
//! that uses Camel. Each of these processes already configured gettext for
//! *itself*. But none of them told gettext where *our* catalogue is, because
//! they do not know about it. This module supplies only that: one domain
//! name, one directory, and the call that associates them.
//!
//! ## The domain, and the callers that look up a string in it
//!
//! [`DOMAIN`] is `evolution-jmap`, so the installed catalogue for a language
//! is `<`[`LOCALE_DIR`]`>/<language>/LC_MESSAGES/evolution-jmap.mo`. Two
//! different callers use it:
//!
//! - **Camel, for us.** A `CamelProvider` has a `translation_domain`, and
//!   Camel calls `dgettext` with it when it shows the name and description of
//!   the provider. Thus Camel translates those two strings without help from
//!   this crate, *if* the domain is bound. This is why the entry point of the
//!   provider module calls [`bind`], and not the location of the first
//!   translated string.
//! - **This code, for text that it emits.** It uses [`translate`], or
//!   [`translate_with`] when the sentence must name something. The refusals
//!   of the calendar backend were the first of these. The account-setup
//!   labels and the user-visible parts of Camel error messages also use them.
//!
//! ## Why the directory is a build-time input
//!
//! The compiled-in default of gettext is `/usr/share/locale`. This is correct
//! only when the modules are installed under `/usr`, which is not always
//! true. The destinations come from pkg-config, and the tree can be staged
//! in any location. A module that looks for its catalogue in the wrong
//! location shows English without a warning. Thus CMake passes
//! `EVOLUTION_JMAP_LOCALEDIR` to cargo, and `build.rs` compiles it in. A
//! plain `cargo build` uses the same `/usr/share/locale` as gettext.
//!
//! ## What is *not* here
//!
//! `textdomain()`, the call that changes the *default* domain for the full
//! process. A loadable module must never make this call. The default domain
//! belongs to the program, and a change to it would remove the translation
//! of the host application. Every lookup here names its domain explicitly,
//! which is the purpose of `dgettext`.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::OnceLock;

unsafe extern "C" {
    fn bindtextdomain(domainname: *const c_char, dirname: *const c_char) -> *mut c_char;
    fn bind_textdomain_codeset(domainname: *const c_char, codeset: *const c_char) -> *mut c_char;
    fn dgettext(domainname: *const c_char, msgid: *const c_char) -> *mut c_char;
}

/// The gettext domain of every translated string in this project.
///
/// It is also the basename of the installed catalogue, and the
/// `translation_domain` that the Camel provider gives to Camel. These three
/// must agree. If not, gettext never opens the catalogue, and no diagnostic
/// tells why.
pub const DOMAIN: &CStr = c"evolution-jmap";

/// The directory where the catalogues are installed.
///
/// `build.rs` gets it from `EVOLUTION_JMAP_LOCALEDIR`, or uses the default of
/// gettext. Thus it is never empty. The `panic!` below is a build-time error
/// for a directory name that contains a NUL. A running module cannot get to
/// it.
pub const LOCALE_DIR: &CStr =
    match CStr::from_bytes_with_nul(concat!(env!("EVOLUTION_JMAP_LOCALE_DIR"), "\0").as_bytes()) {
        Ok(dir) => dir,
        Err(_) => panic!("EVOLUTION_JMAP_LOCALEDIR must not contain a NUL byte"),
    };

/// The binding, made at most once per process.
static BOUND: OnceLock<CString> = OnceLock::new();

/// Tells gettext where the catalogues of this project are, and returns the
/// location that gettext reports.
///
/// Call it from the entry point of a module, before a translated string is
/// necessary. It is idempotent, because a process can load more than one
/// module of this repository. (Evolution loads the account-setup module and
/// uses the Camel provider in the same address space.) Each module must
/// assume that it is possibly the first.
///
/// The return value is the binding that gettext reports, not the argument.
/// This is the only way to tell a binding that worked from one that did
/// not.
pub fn bind() -> &'static CString {
    BOUND.get_or_init(|| bind_to(LOCALE_DIR))
}

/// [`bind`] to a directory of the caller's choosing.
///
/// This is separate from `bind`, so that a test can prove the mechanism with
/// a catalogue that the test wrote. The modules must not call it. For
/// gettext, two bindings to different directories are not an error: the last
/// call wins. Thus a second caller would move the catalogue away from the
/// first caller, without a warning.
///
/// This function is safe, not `unsafe`. The arguments are C strings that
/// outlive the call. glibc copies the directory and does not borrow it, and
/// glibc uses its own lock on the binding list. The caller must think about
/// the sequence of calls, not about memory.
pub fn bind_to(dir: &CStr) -> CString {
    // SAFETY: two C strings, valid for the call, and glibc copies both.
    let bound = unsafe {
        let bound = bindtextdomain(DOMAIN.as_ptr(), dir.as_ptr());
        // The translators choose the charset of the catalogue. A request for
        // UTF-8 makes gettext convert it. Thus `translate` can return a Rust
        // `String` without a lossy step, which could show replacement
        // characters in the account dialog.
        bind_textdomain_codeset(DOMAIN.as_ptr(), c"UTF-8".as_ptr());
        bound
    };

    if bound.is_null() {
        // glibc could not record the binding. This occurs only when it has no
        // memory. The function reports an empty directory, not the requested
        // one, so that the failure stays visible. The argument of the caller
        // does not prove that gettext accepted it.
        return CString::default();
    }
    // SAFETY: a non-NULL return is a C string that glibc owns and keeps until
    // the domain is bound again. The code copies it here and does not keep
    // it.
    unsafe { CStr::from_ptr(bound) }.to_owned()
}

/// Where gettext now thinks the catalogues of this project are.
///
/// [`bind`] and [`bind_to`] cannot answer this question for a caller that did
/// not just call one of them. A query through a new binding would always
/// answer yes, whatever the truth. `bindtextdomain` with a NULL directory is
/// the read-only form: it reports the binding and changes nothing. With it, a
/// test can prove that the entry point of a module bound the domain.
///
/// Before the first binding, this is the compiled-in default of gettext, not
/// [`LOCALE_DIR`]. The two are the same on an uninstalled build. Thus the
/// tests that need the difference start from a directory that is neither of
/// them.
pub fn binding() -> CString {
    // SAFETY: a NUL-terminated domain and a NULL directory, which is the
    // documented query form. The returned string belongs to glibc. The code
    // copies it before a new binding can occur.
    let bound = unsafe { bindtextdomain(DOMAIN.as_ptr(), ptr::null()) };
    if bound.is_null() {
        return CString::default();
    }
    // SAFETY: a non-NULL return is a C string that glibc owns and keeps until
    // the domain is bound again.
    unsafe { CStr::from_ptr(bound) }.to_owned()
}

/// Marks `msgid` for extraction and returns it immediately, untranslated.
///
/// The `N_()` of gettext: the string must get to a translator, but the lookup
/// does not occur *here*. Two situations need this, and both are ordinary
/// here:
///
/// - **A different caller does the lookup.** The name and description of a
///   `CamelProvider` are `'static` C strings. Camel passes them to `dgettext`
///   with the [`DOMAIN`] of the provider when it shows them. [`translate`] on
///   them would be wrong for two reasons. The module loads long before the
///   language of the user is important to that string. And the result would
///   be a translation into the locale at load time, frozen for the life of
///   the process.
/// - **The string is a constant.** [`translate`] returns an owned `String`
///   and cannot be in a `const`. This is a `const fn`, so a marked message can
///   be a constant, and the translation occurs at the point of use.
///
/// It compiles to nothing. Its only function is to be a word that
/// `xgettext --keyword` can look for. This is also why its name has capitals,
/// against the Rust naming convention: every project that uses gettext,
/// the defaults of every extraction tool, and every translator know `N_`.
#[allow(non_snake_case)]
pub const fn N_(msgid: &CStr) -> &CStr {
    msgid
}

/// The translation of `msgid` for the user's language, or `msgid` itself.
///
/// "Or `msgid` itself" is the normal case, not the failure case. It includes
/// all of these: no installed catalogue for the domain, no catalogue for this
/// language, the message not in the catalogue, and a user with the locale
/// `C`. gettext does not distinguish between them, and this function also
/// does not. The untranslated string is a correct answer. An error that the
/// caller cannot correct would only cause an empty label.
///
/// Call [`bind`] before this function. Before that, the lookup goes to the
/// default directory of gettext. That is a different question with an answer
/// that looks correct.
pub fn translate(msgid: &CStr) -> String {
    // SAFETY: `msgid` is valid for the call. `dgettext` never returns NULL:
    // with nothing to translate, it returns the `msgid` pointer that it got.
    // gettext owns the return value, so the code copies it here.
    unsafe { CStr::from_ptr(dgettext(DOMAIN.as_ptr(), msgid.as_ptr())) }
        .to_string_lossy()
        .into_owned()
}

/// [`translate`], but it returns the pointer of gettext, not a copy.
///
/// This is for the one type of caller that [`translate`] cannot serve: a
/// `const gchar *` vfunc (`EOAuth2Service::get_display_name` is the first).
/// Such a vfunc must answer synchronously. It cannot return an owned
/// `String`, because a caller in a different language cannot free it.
/// `dgettext` makes this safe, not a new hazard. With nothing to translate,
/// it returns the same `msgid` pointer that it got. With a loaded catalogue,
/// it returns a pointer into memory that glibc keeps for the binding of the
/// domain, and nothing in this process frees that memory. Both pointers are
/// valid for the life of the process. The `'static` name of a
/// `CamelProvider` uses the same guarantee when Camel calls `dgettext` on it
/// directly, not through this module.
///
/// Call [`bind`] before this function, as for [`translate`].
pub fn translate_static(msgid: &'static CStr) -> *const c_char {
    // SAFETY: `msgid` outlives the call. `dgettext` never returns NULL: with
    // nothing to translate, it returns the `msgid` pointer unchanged. The code
    // does not copy the result, because the purpose is to keep the pointer
    // that gettext considers valid for the life of the process.
    unsafe { dgettext(DOMAIN.as_ptr(), msgid.as_ptr()) }.cast_const()
}

/// [`translate`], with `%1$s`-style placeholders filled from `arguments`.
///
/// The form of a message that must name something: a time zone, a folder,
/// or the words of a server. The template is a *translation*. Thus two rules
/// apply, and both are mandatory:
///
/// - **The placeholders are numbered.** Word order changes between
///   languages: "the time zone %2$s cannot state %1$s" is the natural German
///   order of a sentence that English writes in the opposite order. With
///   bare `%s`es, a translator must choose between a natural sentence and a
///   correct one. Thus every placeholder here has the position of its
///   argument. A translation can put them in any order, or repeat one.
/// - **This is not `printf`, and must never become `printf`.** A catalogue is
///   data from a file that this program did not write, loaded at run time. A
///   translated string in a real format function is the textbook way for
///   that data to get control of the process: a `%n` that is not in the
///   source, read against arguments that do not exist. The substitution below
///   knows exactly one construct and copies all other bytes. Thus a hostile
///   or incorrect `.mo` can only make the sentence read badly.
///
/// A placeholder with no argument stays as written. It is not removed and it
/// is not fatal. It is a bug in a translation, and the visible marker lets a
/// user report it. Ill-formed `%` sequences are ordinary characters, so a
/// message can contain `50%` without special syntax.
///
/// As for [`translate`], `xgettext --keyword` knows this name. Thus xgettext
/// extracts a literal written here in its position, and the literal needs no
/// [`N_`]. See `po/POTFILES.in`: the file that contains the string must be in
/// that list.
pub fn translate_with(msgid: &CStr, arguments: &[&str]) -> String {
    substitute(&translate(msgid), arguments)
}

/// Fills the placeholders of `template` from `arguments`, in one
/// left-to-right pass.
///
/// One pass, not one replacement for each argument. This is important
/// because of what the arguments *are*: values from a server, from the
/// calendar of a user, or from a time zone database. With one replacement for
/// each argument, the second replacement would examine the text of the first
/// argument. Thus a value that contains `%2$s` would get the next argument
/// inserted into it, and an external value would control the message. Here,
/// the expansion of an argument goes to the output and is never examined
/// again.
fn substitute(template: &str, arguments: &[&str]) -> String {
    let mut filled = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(at) = rest.find('%') {
        filled.push_str(&rest[..at]);
        rest = &rest[at..];
        match placeholder(rest) {
            Some((position, length)) if (1..=arguments.len()).contains(&position) => {
                filled.push_str(arguments[position - 1]);
                rest = &rest[length..];
            }
            // Not a placeholder, or one with no argument: the `%` is an
            // ordinary character. The scan steps over only the `%`, so that it
            // continues and examines the remaining text again.
            _ => {
                filled.push('%');
                rest = &rest[1..];
            }
        }
    }

    filled.push_str(rest);
    filled
}

/// The argument position at the start of `text`, and its length in bytes.
///
/// `text` starts at a `%`. A placeholder is that `%`, one or more decimal
/// digits, and `$s`. This is the positional form of `printf`, so the tools of
/// a translator recognise it and the format checks of `xgettext` apply. All
/// other text is not a placeholder. This includes a number that is too large
/// to be a position.
fn placeholder(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut end = 1;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == 1 || bytes.get(end) != Some(&b'$') || bytes.get(end + 1) != Some(&b's') {
        return None;
    }
    Some((text[1..end].parse().ok()?, end + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_placeholder_gives_its_position_and_byte_length() {
        assert_eq!(placeholder("%12$s"), Some((12, 5)));
        assert_eq!(placeholder("%1$s"), Some((1, 4)));
    }

    #[test]
    fn digits_without_a_dollar_s_suffix_are_not_a_placeholder() {
        // This input gets to the suffix check (`end` after the digits), and
        // the no-digit case does not short-circuit it. Thus this test fails
        // only when the code evaluates both parts of the guard.
        assert_eq!(placeholder("%12xs"), None);
    }

    #[test]
    fn no_digits_at_all_is_not_a_placeholder() {
        assert_eq!(placeholder("%$s"), None);
    }
}
