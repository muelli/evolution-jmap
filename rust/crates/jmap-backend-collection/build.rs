// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Re-emits the `eds-sys` feature cfgs that the targets of this crate need.
//! The module comment of `jmap-mail/build.rs` tells why the detection is in
//! `eds-sys` and each dependent repeats only the re-emission.

/// - `eds_credential_store_id`: the installed EDS has
///   `ESourceAuthentication:credential-store-id` (eds#663). A child then binds
///   its token-cache key from it, not from `credential-name`. Used by
///   `child_added.rs` and `mail_child.rs`.
/// - `eds_oauth2_dynamic`: the installed EDS has its own
///   `EOAuth2ServiceDynamic`. This module then registers no service of its own.
///   `tests/oauth2_service.rs` asserts this.
const EDS_FEATURES: &[&str] = &["eds_credential_store_id", "eds_oauth2_dynamic"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    for feature in EDS_FEATURES {
        // Declare each cfg, also when this EDS does not set it. Then
        // `-D warnings` does not fail on a `#[cfg]` that is not satisfied.
        println!("cargo::rustc-check-cfg=cfg({feature})");
        let key = format!("DEP_EVOLUTION_DATA_SERVER_{}", feature.to_uppercase());
        println!("cargo:rerun-if-env-changed={key}");
        if std::env::var_os(&key).is_some() {
            println!("cargo::rustc-cfg={feature}");
        }
    }
}
