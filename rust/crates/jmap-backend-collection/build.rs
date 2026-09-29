// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Re-emits the one `eds-sys` feature cfg `child_added.rs` and `mail_child.rs`
//! need — see `jmap-mail/build.rs`'s module comment for why the detection
//! lives in `eds-sys` and only the re-emission is duplicated per dependent.

/// Whether the installed EDS has `ESourceAuthentication:credential-store-id`
/// (eds#663) to bind a child's token-cache key from, in place of
/// `credential-name`.
const EDS_FEATURES: &[&str] = &["eds_credential_store_id"];

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
