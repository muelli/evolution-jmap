// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles into the crate the directory that contains the installed
//! translation catalogues.
//!
//! `EVOLUTION_JMAP_LOCALEDIR` comes from CMake, which knows the install
//! prefix. A plain `cargo build` has no prefix and gets gettext's own
//! compiled-in default. Thus an uninstalled build behaves exactly as if
//! nothing bound the domain.
//!
//! The value is re-exported under a second name, so `i18n.rs` can use
//! `env!` and not `option_env!`. `env!` expands to a literal, so the
//! directory can be a `&CStr` constant and not an allocation at each call.

//! It also re-emits the one `eds-sys` feature cfg that this crate's sources
//! need. The detection itself is in `eds-sys/build.rs` and this file does not
//! repeat it. `jmap-mail/build.rs`'s module comment tells why each dependent
//! duplicates only the re-emission.

/// The `eds-sys` feature that `oauth2.rs` uses in `#[cfg]`: whether the
/// installed EDS has upstream's own dynamically-registered-client OAuth 2.0
/// service (`EOAuth2ServiceDynamic`, method "OAuth2Dynamic"). Where it
/// exists, it replaces this project's own `EOAuth2Service`.
const EDS_FEATURES: &[&str] = &["eds_oauth2_dynamic"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=EVOLUTION_JMAP_LOCALEDIR");

    let dir = std::env::var("EVOLUTION_JMAP_LOCALEDIR")
        .unwrap_or_else(|_| "/usr/share/locale".to_owned());
    println!("cargo::rustc-env=EVOLUTION_JMAP_LOCALE_DIR={dir}");

    for feature in EDS_FEATURES {
        // Declare it even if this EDS does not set it, so `-D warnings` does
        // not fail on the `#[cfg]`s that it does not satisfy.
        println!("cargo::rustc-check-cfg=cfg({feature})");
        let key = format!("DEP_EVOLUTION_DATA_SERVER_{}", feature.to_uppercase());
        println!("cargo:rerun-if-env-changed={key}");
        if std::env::var_os(&key).is_some() {
            println!("cargo::rustc-cfg={feature}");
        }
    }
}
