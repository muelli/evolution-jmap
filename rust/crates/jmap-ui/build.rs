// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This script sets an RPATH to the private library directories of Evolution
// on the test binaries of this crate.
//
// `rustc-link-arg` applies only to the package that emits it. Thus `evo-sys`
// publishes the directories as metadata (`DEP_EVOLUTION_SHELL_LIBDIRS`), and
// this script changes that metadata into an rpath.
//
// The rpath does not go into `jmap-config-module`, the separate cdylib that
// Evolution loads. That crate has its own build script for its rpath.
//
// This script also emits the `evolution_eui_manager` cfg of `evo-sys` again.
// The cfg selects GtkAction/GtkUIManager or EUIAction/EUIManager, from the
// version of the installed Evolution. A `rustc-cfg` applies only to the crate
// that sets it, not to its dependents.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_EUI_MANAGER");
    println!("cargo:rustc-check-cfg=cfg(evolution_eui_manager)");

    // Missing only if `evo-sys` does not publish it. Then fail the build:
    // do not make binaries that cannot start.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // `-rpath`, not `-rpath-link`: the path must be in the file, not only in the link step.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }

    let eui_manager = env::var("DEP_EVOLUTION_SHELL_EUI_MANAGER")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_EUI_MANAGER");
    if eui_manager == "1" {
        println!("cargo:rustc-cfg=evolution_eui_manager");
    }
}
