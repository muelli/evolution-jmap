// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Re-emits the `eds-sys` feature configurations for tests.

/// Identifies whether EDS provides `EOAuth2ServiceDynamic`.
const EDS_FEATURES: &[&str] = &["eds_oauth2_dynamic"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    for feature in EDS_FEATURES {
        // Declare the configuration key for rustc check-cfg.
        println!("cargo::rustc-check-cfg=cfg({feature})");
        let key = format!("DEP_EVOLUTION_DATA_SERVER_{}", feature.to_uppercase());
        println!("cargo:rerun-if-env-changed={key}");
        if std::env::var_os(&key).is_some() {
            println!("cargo::rustc-cfg={feature}");
        }
    }
}
