// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Records the location of the Evolution libraries in each binary that this
// crate links.
//
// Evolution installs `libevolution-mail.so.0` and related libraries into a
// private directory (`/usr/lib/evolution`), not into the system directory. A
// binary without that directory in its `RUNPATH` links, but then does not
// start: `error while loading shared libraries`, before `main`.
//
// `evo-sys` cannot do this for this crate. Cargo passes the `-l` and `-L` flags
// of a build script to all downstream crates, but it limits `rustc-link-arg`
// to the package that emitted it. The `-Wl,-R` in the Evolution `.pc` files is
// a `rustc-link-arg`. Thus `evo-sys` publishes the directories as metadata
// (`cargo:libdirs`, which Cargo gives to the dependents of a crate with a
// `links` key as `DEP_EVOLUTION_SHELL_LIBDIRS`). This script changes them back
// into an rpath for the test binaries here. It does NOT apply to
// `jmap-config-module`, the separate cdylib crate that builds the account
// setup module (rustc-link-arg is package-scoped, and that crate is a
// different package). For the same reason, that crate has its own copy of the
// rpath part of this build script.
//
// This script also emits again the `eds-sys` feature cfgs that the sources of
// this crate need. The module comment of `jmap-mail/build.rs` tells why the
// detection is in `eds-sys` and only the re-emission is in each dependent.

use std::env;

/// The `eds-sys` features that the sources of this crate use in `#[cfg]`:
///
/// - `eds_credential_store_id`: the installed EDS has
///   `ESourceAuthentication:credential-store-id` (eds#663) for the token-cache
///   key of an OAuth 2.0 account, in place of `credential-name`: `account.rs`.
/// - `eds_oauth2_dynamic`: the installed EDS has upstream's
///   `EOAuth2ServiceDynamic` and `ESourceOAuth2Client`. This project then uses
///   them, and does not register its own service and storage extension:
///   `oauth2.rs` and some tests.
const EDS_FEATURES: &[&str] = &["eds_credential_store_id", "eds_oauth2_dynamic"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");

    // Missing only if `evo-sys` does not publish it. Then fail the build, and do
    // not silently make binaries that cannot start.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // `-rpath` and not `-rpath-link`: the file must record it. The linker
        // must not only use it during the link.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }

    for feature in EDS_FEATURES {
        // Declared also if this EDS does not set it, so that `-D warnings` does
        // not fail on the `#[cfg]`s that it does not satisfy.
        println!("cargo::rustc-check-cfg=cfg({feature})");
        let key = format!("DEP_EVOLUTION_DATA_SERVER_{}", feature.to_uppercase());
        println!("cargo:rerun-if-env-changed={key}");
        if std::env::var_os(&key).is_some() {
            println!("cargo::rustc-cfg={feature}");
        }
    }
}
