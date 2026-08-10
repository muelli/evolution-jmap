// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bakes the directory the translation catalogues were installed under into
//! the crate.
//!
//! `EVOLUTION_JMAP_LOCALEDIR` comes from CMake, which knows the install prefix;
//! a plain `cargo build` has no prefix to speak of and gets gettext's own
//! compiled-in default, so an uninstalled build behaves exactly as it would if
//! nothing bound the domain at all.
//!
//! The value is re-exported under a second name so that `i18n.rs` can reach it
//! with `env!` rather than `option_env!` — `env!` expands to a literal, which
//! is what lets the directory be a `&CStr` constant instead of something
//! allocated at every call.

fn main() {
    println!("cargo::rerun-if-env-changed=EVOLUTION_JMAP_LOCALEDIR");

    let dir = std::env::var("EVOLUTION_JMAP_LOCALEDIR")
        .unwrap_or_else(|_| "/usr/share/locale".to_owned());
    println!("cargo::rustc-env=EVOLUTION_JMAP_LOCALE_DIR={dir}");
}
