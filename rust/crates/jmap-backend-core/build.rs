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

//! It also re-emits the one `eds-sys` feature cfg this crate's sources need.
//! The detection itself lives in `eds-sys/build.rs` and is never repeated
//! here; see `jmap-mail/build.rs`'s module comment for why only the
//! re-emission is duplicated per dependent.

/// The `eds-sys` feature `oauth2.rs` `#[cfg]`s on: whether the installed EDS
/// ships upstream's own dynamically-registered-client OAuth 2.0 service
/// (`EOAuth2ServiceDynamic`, method "OAuth2Dynamic"), which replaces this
/// project's own `EOAuth2Service` where it exists.
const EDS_FEATURES: &[&str] = &["eds_oauth2_dynamic"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=EVOLUTION_JMAP_LOCALEDIR");

    let dir = std::env::var("EVOLUTION_JMAP_LOCALEDIR")
        .unwrap_or_else(|_| "/usr/share/locale".to_owned());
    println!("cargo::rustc-env=EVOLUTION_JMAP_LOCALE_DIR={dir}");

    for feature in EDS_FEATURES {
        // Declared regardless of whether this EDS sets it, so `-D warnings`
        // does not trip over the `#[cfg]`s it turns out not to satisfy.
        println!("cargo::rustc-check-cfg=cfg({feature})");
        let key = format!("DEP_EVOLUTION_DATA_SERVER_{}", feature.to_uppercase());
        println!("cargo:rerun-if-env-changed={key}");
        if std::env::var_os(&key).is_some() {
            println!("cargo::rustc-cfg={feature}");
        }
    }
}
