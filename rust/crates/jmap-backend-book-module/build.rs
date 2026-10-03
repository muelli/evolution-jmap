// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Records where Evolution's own libraries live, in the cdylib this crate
// actually produces (`libebookbackendjmap.so`).
//
// `jmap-backend-book` links `libevolution-mail.so` transitively (through
// `jmap-config`, which needs `evo-sys` for its own reasons), and that NEEDED
// entry carries no `RUNPATH`/`RPATH` of its own: `cargo:rustc-link-arg` from
// a dependency's build script only ever applies to that dependency's own
// build, never to a downstream crate's final link. Without this, the module
// `dlopen`s fine on a machine whose ld cache already has Evolution's own
// private libdir in it (true wherever `evolution` itself is installed
// alongside this backend) but fails everywhere else — a minimal container
// building only the EDS headers and runtime being exactly such a case. Same
// problem, same fix as `jmap-config-module/build.rs`.
//
// `evo-sys` is in `[dependencies]` for metadata only: this crate calls none
// of its bindings, but Cargo only sets `DEP_EVOLUTION_SHELL_LIBDIRS` (from
// its `links = "evolution-shell"` key) for the build script of a package with
// a direct, non-build dependency on it.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");

    // Absent only if `evo-sys` stopped publishing it, which is a build to fail
    // rather than one to quietly produce a module that cannot load.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // `-rpath` and not `-rpath-link`: this has to be recorded in the file,
        // not merely used while linking it.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
