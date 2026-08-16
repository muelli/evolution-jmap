// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The translation domain the modules bind, and what a lookup does when there
//! is no catalogue to find.
//!
//! Everything here is true of the machine it runs on rather than of a
//! catalogue this test wrote — no `.mo` file is involved, and the binding is
//! the one the build configured. The other half, that a catalogue installed
//! where [`bind`] points is actually found, needs a locale and its own
//! process; it is in `tests/catalogue.rs`.
//!
//! [`bind`]: jmap_backend_core::i18n::bind

use jmap_backend_core::i18n::{DOMAIN, LOCALE_DIR, bind, translate, translate_with};

/// The binding gettext reports is the directory the build put in the binary.
///
/// `bindtextdomain` answers with the domain's current binding, so the
/// assertion is gettext's own account of what happened rather than a repeat of
/// the argument that was passed in. Absent the call, gettext would answer with
/// its compiled-in default (`/usr/share/locale`) — which is also this crate's
/// fallback when nothing configured one, so the test says nothing on a default
/// build. It says everything on an installed one, which is the case that
/// matters: a module installed under a prefix has to look for its catalogue
/// under that prefix.
#[test]
fn binding_the_domain_points_gettext_at_the_configured_directory() {
    assert_eq!(bind().as_c_str(), LOCALE_DIR);
}

/// Binding twice is binding once.
///
/// Each module's entry point binds, and a process can hold several of them —
/// the Camel provider and the account-setup module live in the same Evolution.
/// They agree about the directory, so a second call would be harmless rather
/// than wrong; it is the same value from the same `OnceLock` all the same, and
/// that is what pins the call to the one place that can be reasoned about.
#[test]
fn the_binding_is_made_once_and_shared() {
    assert!(std::ptr::eq(bind(), bind()));
}

/// A message with no catalogue behind it comes back as itself.
///
/// This is the property `crate::provider`'s `translation_domain` comment
/// claims for the state the repository actually ships in today — the domain is
/// named, no `.mo` is installed under it, and gettext's answer is the
/// untranslated string rather than an empty one or a crash. It is worth an
/// assertion because the alternative failure is silent: a module that returned
/// `""` for every label would look like a translation problem, not a
/// programming one.
#[test]
fn a_message_with_no_catalogue_is_returned_untranslated() {
    bind();
    let msgid = c"For reading and storing mail on JMAP servers.";
    assert_eq!(translate(msgid), msgid.to_str().expect("ASCII"));
}

/// A message with arguments in it comes back with the arguments in it.
///
/// The plain case, and the one every caller writes: no catalogue, so the
/// template is the msgid, and each `%N$s` becomes the argument it numbers.
#[test]
fn a_messages_placeholders_are_filled_from_its_arguments() {
    bind();
    assert_eq!(
        translate_with(c"%1$s is in %2$s", &["a meeting", "Europe/Berlin"]),
        "a meeting is in Europe/Berlin"
    );
}

/// The arguments are read in one pass, not substituted one after another.
///
/// The values here are a server's, a user's, or a time zone database's — never
/// this code's — so one of them can perfectly well contain something that looks
/// like a placeholder. Substituting argument by argument would then let the
/// first argument's text be rewritten by the second, which is how a value from
/// outside starts choosing what the message says. A single left-to-right scan
/// over the template cannot do that: what an argument expands to is output, not
/// input.
#[test]
fn an_argument_that_looks_like_a_placeholder_is_not_expanded_again() {
    bind();
    assert_eq!(
        translate_with(c"%1$s, %2$s", &["%2$s", "Berlin"]),
        "%2$s, Berlin"
    );
}

/// A placeholder no argument answers is left standing, and so is a stray `%`.
///
/// Both are a broken translation rather than a broken program, and there is
/// nothing sensible to put in their place — so the marker is shown as it is
/// written. That is the visible failure a translator can be told about, where
/// dropping it silently would leave a sentence missing a word, and panicking
/// would take a module down over a `.mo` file.
///
/// The wider point is that none of this is `printf`. Handing a translated
/// string to a real format function is the classic way a catalogue becomes an
/// attack on the program that loads it — a `%n` in the translation, and the
/// arguments the caller passed are not what gets read. Here the only thing that
/// means anything is `%N$s`, and everything else is a character.
#[test]
fn a_placeholder_with_no_argument_stays_as_written() {
    bind();
    assert_eq!(
        translate_with(c"%1$s, %7$s, 50%, %d, %n", &["one"]),
        "one, %7$s, 50%, %d, %n"
    );
}

/// The domain is the one Camel and gettext file a catalogue under.
///
/// Not a tautology: the constant is also the `translation_domain` the Camel
/// provider hands to Camel (checked from that side in `jmap-mail`), and the
/// basename an installed `evolution-jmap.mo` has to carry. Spelling it out here
/// means a change to it is a change to a test, not a silently unfindable
/// catalogue.
#[test]
fn the_domain_is_the_projects_own() {
    assert_eq!(DOMAIN, c"evolution-jmap");
}
