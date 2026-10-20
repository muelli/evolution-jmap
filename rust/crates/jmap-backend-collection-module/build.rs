// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Records the libdir of Evolution's own libraries in the cdylib that this
// crate produces (`module-jmap-backend.so`).
//
// `jmap-backend-collection` links `libevolution-mail.so` through
// `jmap-config`, which needs `evo-sys`. That NEEDED entry has no `RUNPATH` or
// `RPATH`, because `cargo:rustc-link-arg` from the build script of a
// dependency applies only to the build of that dependency. Without this
// script, `dlopen` of the module succeeds only where the ld cache already
// lists the private libdir of Evolution. That is true where `evolution` is
// installed, but not in a minimal container with only the EDS headers and
// runtime. `jmap-config-module/build.rs` has the same problem and fix.
//
// `evo-sys` is in `[dependencies]` for metadata only, and this crate calls
// none of its bindings. Cargo sets `DEP_EVOLUTION_SHELL_LIBDIRS` only for the
// build script of a package with a direct, non-build dependency on `evo-sys`.
// The value comes from the `links = "evolution-shell"` key of `evo-sys`.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");

    // The variable is absent only if `evo-sys` stops publishing it. Then the
    // build must fail here, not produce a module that cannot load.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // Use `-rpath`, not `-rpath-link`: the linker must record the path in
        // the file, not only use it during the link.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
