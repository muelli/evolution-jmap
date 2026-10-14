// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Sets RPATH for Evolution's private library directories on this crate's own
// test binaries: `rustc-link-arg` is package-scoped, so `evo-sys` republishes
// the directories as metadata (`DEP_EVOLUTION_SHELL_LIBDIRS`) instead of
// emitting them itself, and this turns that back into an rpath. Does not
// reach `jmap-config-module`, the separate cdylib the account-setup module is
// actually built from; that crate carries its own copy of this script.
//
// Also re-emits `evo-sys`'s `evolution_eui_manager` cfg (GtkAction/
// GtkUIManager vs EUIAction/EUIManager, decided against the Evolution headers
// actually installed), since a `rustc-cfg` only reaches the crate that sets
// it, not its dependents.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_EUI_MANAGER");
    println!("cargo:rustc-check-cfg=cfg(evolution_eui_manager)");

    // Absent only if `evo-sys` stopped publishing it, which is a build to fail
    // rather than one to quietly produce binaries that cannot start.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // `-rpath` and not `-rpath-link`: this has to be recorded in the file,
        // not merely used while linking it.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }

    let eui_manager = env::var("DEP_EVOLUTION_SHELL_EUI_MANAGER")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_EUI_MANAGER");
    if eui_manager == "1" {
        println!("cargo:rustc-cfg=evolution_eui_manager");
    }
}
