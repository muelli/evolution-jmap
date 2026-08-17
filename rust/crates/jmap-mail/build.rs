// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Re-emits the EDS feature detection `eds-sys` did, so this crate can `#[cfg]`
//! on it.
//!
//! `eds-sys/build.rs` decides which parts of the Camel API the installed EDS
//! has by looking for marker identifiers in the bindings bindgen just generated
//! (its `EDS_FEATURES` table). The answer reaches `eds-sys` itself as a
//! `cargo::rustc-cfg` — but a `rustc-cfg` is per-crate, so a `#[cfg]` written
//! here would be false on every EDS, silently, which is the one failure mode
//! `docs/eds-version-matrix.md` exists to prevent.
//!
//! Cargo's channel for exactly this is a build script's `cargo::metadata`,
//! which it hands to the build scripts of the crates that depend on it, keyed
//! by the dependency's `links` value. So `eds-sys` publishes each detected
//! feature and this script turns it back into a cfg for this crate's targets —
//! the library *and* its tests, which is what lets a test drive the search
//! entry point the EDS in front of it actually has.
//!
//! The detection itself is deliberately *not* repeated here. Duplicating it
//! would mean two oracles that can disagree, and re-deriving it from a version
//! number would mean guessing which release changed what — the thing
//! `eds-sys`'s table refuses to do.

/// The features this crate's sources `#[cfg]` on, as the names `eds-sys`
/// publishes them under. A subset of that crate's table: the ones a Camel
/// provider's own code has to care about.
const FEATURES: &[&str] = &[
    // Whether `CamelFolderSearch` still exists as an object a folder can
    // delegate its search vfuncs to — `crate::folder`.
    "camel_folder_search_object",
    // Whether the summary database's row structs are still called
    // `CamelMIRecord`/`CamelFIRecord`, which also settles the shape of
    // `summary_header_save` — `crate::summary`, `crate::message_info`.
    "camel_summary_records",
    // Whether `CamelProvider` still has the three URL-keyed service-cache
    // fields — `crate::provider`.
    "camel_provider_url_helpers",
    // Whether a folder's uid list is borrowed (`get_uids`/`free_uids`) or
    // copied (`dup_uids`). Only the tests ask a folder directly, but the cfg is
    // re-emitted for every target alike.
    "camel_folder_get_uids",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    for feature in FEATURES {
        // Declared for every feature, present or not, so that `-D warnings`
        // does not trip over the `#[cfg]`s this EDS turns out not to satisfy.
        println!("cargo::rustc-check-cfg=cfg({feature})");

        // `links = "evolution-data-server"`, uppercased, is the prefix cargo
        // puts on `eds-sys`'s metadata keys.
        let key = format!("DEP_EVOLUTION_DATA_SERVER_{}", feature.to_uppercase());
        println!("cargo:rerun-if-env-changed={key}");
        if std::env::var_os(&key).is_some() {
            println!("cargo::rustc-cfg={feature}");
        }
    }
}
