// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Records where Evolution's own libraries live, in the cdylib this crate
// actually produces (`module-jmap-configuration.so`).
//
// `jmap-config`'s own `build.rs` solves this exact problem, but only for
// itself: `cargo:rustc-link-arg` is scoped to the package whose build script
// emits it. The module Evolution loads is built by this separate crate
// (`crate-type = ["cdylib"]`), not by `jmap-config` directly. Without this,
// the built module carries no `RUNPATH`/`RPATH` at all, unlike every module
// Evolution installs alongside it. `dlopen`ing it then fails before this
// crate's own code ever runs, unless the process already has
// `libevolution-mail.so.0` resident, or `LD_LIBRARY_PATH` points at
// `/usr/lib/evolution`. That silently keeps the account-setup module from
// registering with a real `EConfigLookup`.
//
// `evo-sys` is in `[dependencies]` for metadata only: this crate calls none
// of its bindings. Cargo sets `DEP_EVOLUTION_SHELL_LIBDIRS` (from `evo-sys`'s
// `links = "evolution-shell"` key) only for the build script of a package
// with a direct, non-build dependency on it.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");

    // Absent only if `evo-sys` stopped publishing it. Fail the build then,
    // rather than quietly produce a module that cannot load.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // `-rpath` and not `-rpath-link`: this has to be recorded in the file,
        // not merely used while linking it.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
