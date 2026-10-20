// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The translation domain that the modules bind, and the result of a lookup
//! when there is no catalogue to find.
//!
//! These tests check the machine that they run on, not a catalogue that the
//! test wrote. No `.mo` file is involved, and the binding is the one that the
//! build configured. The other half (gettext finds a catalogue installed where
//! [`bind`] points) needs a locale and its own process. It is in
//! `tests/catalogue.rs`.
//!
//! [`bind`]: jmap_backend_core::i18n::bind

use jmap_backend_core::i18n::{
    DOMAIN, LOCALE_DIR, bind, translate, translate_static, translate_with,
};

/// The binding that gettext reports is the directory that the build put in the
/// binary.
///
/// `bindtextdomain` returns the current binding of the domain. Thus the
/// assertion checks the report of gettext, not a copy of the argument. Without
/// the call, gettext returns its compiled-in default (`/usr/share/locale`).
/// This is also the fallback of this crate when no directory is configured, so
/// the test proves nothing on a default build. On an installed build, it
/// proves the important case: a module installed under a prefix must look for
/// its catalogue under that prefix.
#[test]
fn binding_the_domain_points_gettext_at_the_configured_directory() {
    assert_eq!(bind().as_c_str(), LOCALE_DIR);
}

/// A second bind has the same result as the first bind.
///
/// The entry point of each module binds, and a process can hold several
/// modules: the Camel provider and the account-setup module are in the same
/// Evolution. They use the same directory, so a second call is harmless. But
/// it also returns the same value from the same `OnceLock`, so the bind occurs
/// in one known location.
#[test]
fn the_binding_is_made_once_and_shared() {
    assert!(std::ptr::eq(bind(), bind()));
}

/// A message with no catalogue returns unchanged.
///
/// This is the state where the domain has a name but no `.mo` is installed
/// under it. Gettext then returns the untranslated string, not an empty string
/// or a crash. The assertion is necessary because the other failure is
/// silent: a module that returned `""` for each label would look like a
/// translation problem, not a programming problem.
#[test]
fn a_message_with_no_catalogue_is_returned_untranslated() {
    bind();
    let msgid = c"For reading and storing mail on JMAP servers.";
    assert_eq!(translate(msgid), msgid.to_str().expect("ASCII"));
}

/// A message with arguments returns with the arguments filled in.
///
/// This is the plain case that each caller writes. There is no catalogue, so
/// the template is the msgid, and each `%N$s` becomes the argument number N.
#[test]
fn a_messages_placeholders_are_filled_from_its_arguments() {
    bind();
    assert_eq!(
        translate_with(c"%1$s is in %2$s", &["a meeting", "Europe/Berlin"]),
        "a meeting is in Europe/Berlin"
    );
}

/// The template is read in one pass, not substituted one argument at a time.
///
/// The values come from a server, a user, or a time zone database, never from
/// this code. Thus a value can contain text that looks like a placeholder.
/// Substitution argument by argument would let the second argument change the
/// text of the first. Thus an external value could control the message. A
/// single left-to-right scan of the template prevents this: the expansion of
/// an argument is output, not input.
#[test]
fn an_argument_that_looks_like_a_placeholder_is_not_expanded_again() {
    bind();
    assert_eq!(
        translate_with(c"%1$s, %2$s", &["%2$s", "Berlin"]),
        "%2$s, Berlin"
    );
}

/// A placeholder with no argument stays unchanged, and so does a stray `%`.
///
/// Both are a broken translation, not a broken program, and no replacement is
/// correct. Thus the marker shows as it is written. A translator can see and
/// report this failure. If the marker were silently removed, the sentence would
/// lose a word. A panic would stop a module because of a `.mo` file.
///
/// Also, this is not `printf`. A translated string given to a real format
/// function is a usual way for a catalogue to attack the program that loads
/// it: with a `%n` in the translation, the function does not read the
/// arguments of the caller. Here only `%N$s` has a meaning, and all other text
/// is characters.
#[test]
fn a_placeholder_with_no_argument_stays_as_written() {
    bind();
    assert_eq!(
        translate_with(c"%1$s, %7$s, 50%, %d, %n", &["one"]),
        "one, %7$s, 50%, %d, %n"
    );
}

/// With no catalogue, the static-pointer form returns the same pointer that it
/// received, not only an equal string.
///
/// `dgettext` documents this: with nothing to translate, it returns the
/// `msgid` pointer unchanged, not a copy. `translate_static` uses this to
/// answer a `const gchar *` vfunc without an allocation. Thus the test checks
/// the pointer, not only its contents.
#[test]
fn a_message_with_no_catalogue_keeps_its_own_pointer() {
    bind();
    let msgid = c"For reading and storing mail on JMAP servers.";
    assert_eq!(translate_static(msgid), msgid.as_ptr());
}

/// The domain is the name under which Camel and gettext find a catalogue.
///
/// This is not a tautology. The constant is also the `translation_domain` that
/// the Camel provider gives to Camel (`jmap-mail` checks that side). It is
/// also the basename of an installed `evolution-jmap.mo`. With the literal
/// here, a change to it also changes a test, and the catalogue does not
/// silently become unfindable.
#[test]
fn the_domain_is_the_projects_own() {
    assert_eq!(DOMAIN, c"evolution-jmap");
}
