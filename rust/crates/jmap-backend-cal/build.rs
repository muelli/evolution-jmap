// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Propagates `eds-sys` feature cfgs for test builds.

/// Emits `eds_oauth2_dynamic` if provided by the installed EDS.
const EDS_FEATURES: &[&str] = &["eds_oauth2_dynamic"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

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
