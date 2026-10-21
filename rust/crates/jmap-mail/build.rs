// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Re-emits the EDS feature cfgs of `eds-sys`, so that this crate can `#[cfg]`
//! on them.
//!
//! A `cargo::rustc-cfg` applies to one crate only. Without this script, each
//! `#[cfg]` in this crate is silently false on every EDS.
//!
//! `eds-sys` publishes each detected feature as `cargo::metadata`. Cargo gives
//! it to this script, which emits it again as a cfg for the library and its tests.
//!
//! This script does not repeat the detection. A second detector can disagree
//! with the first, and a version number does not tell which release changed what.

/// The features that this crate's sources `#[cfg]` on, with the names that
/// `eds-sys` publishes. These are the Camel provider subset of its table.
const FEATURES: &[&str] = &[
    // Whether `CamelFolderSearch` exists as an object that a folder can give
    // its search vfuncs to: `crate::folder`.
    "camel_folder_search_object",
    // Whether `CamelFolderClass` has the 3.58 slot that answers a body term
    // from the server: `crate::search_body`.
    "camel_folder_search_body_sync",
    // Whether the summary database row structs have the names
    // `CamelMIRecord`/`CamelFIRecord`. This also sets the shape of
    // `summary_header_save`: `crate::summary`, `crate::message_info`.
    "camel_summary_records",
    // Whether `CamelProvider` has `auto_detect` (removed in 3.58), and
    // `url_hash` and `url_equal` (removed in 3.60): `crate::provider`.
    "camel_provider_auto_detect",
    "camel_provider_url_helpers",
    // Whether a folder lends its uid list (`get_uids`/`free_uids`) or gives a
    // copy (`dup_uids`). Only the tests use it, but all targets get the cfg.
    "camel_folder_get_uids",
    // Whether the message cache writes a new entry to a temporary name and
    // renames it into place (`camel_data_cache_add_atomic`/`commit_atomic`/
    // `discard_atomic`), or writes to the final path (`camel_data_cache_add`):
    // `crate::cache`.
    "camel_data_cache_atomic",
    // Whether EDS has the upstream `CamelSaslXOAuth2Dynamic`, the mechanism of
    // its `EOAuth2ServiceDynamic`. If so, this provider publishes that type and
    // does not declare its own: `crate::sasl`.
    "eds_oauth2_dynamic",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    for feature in FEATURES {
        // Declare every feature, present or not, so that `-D warnings` accepts
        // a `#[cfg]` that this EDS does not satisfy.
        println!("cargo::rustc-check-cfg=cfg({feature})");

        // Cargo makes the metadata key prefix from the `links` value of
        // `eds-sys` ("evolution-data-server"), in upper case.
        let key = format!("DEP_EVOLUTION_DATA_SERVER_{}", feature.to_uppercase());
        println!("cargo:rerun-if-env-changed={key}");
        if std::env::var_os(&key).is_some() {
            println!("cargo::rustc-cfg={feature}");
        }
    }
}
