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

use jmap_backend_core::i18n::{DOMAIN, LOCALE_DIR, bind, translate};

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
