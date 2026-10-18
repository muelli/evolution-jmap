// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Records where Evolution's own libraries live, in the cdylib this crate
// actually produces (`module-jmap-backend.so`).
//
// `jmap-backend-collection` links `libevolution-mail.so` transitively,
// through `jmap-config`, which needs `evo-sys` for its own reasons. That
// NEEDED entry carries no `RUNPATH`/`RPATH` of its own: `cargo:rustc-link-arg`
// from a dependency's build script applies only to that dependency's own
// build, never to a downstream crate's final link. Without this, the
// module `dlopen`s fine on a machine whose ld cache already lists
// Evolution's own private libdir (true wherever `evolution` itself is
// installed alongside this backend), but fails everywhere else. A minimal
// container that builds only the EDS headers and runtime is exactly such a
// case. The problem and the fix are the same as in
// `jmap-config-module/build.rs`.
//
// `evo-sys` is in `[dependencies]` for metadata only. This crate calls none
// of its bindings. Cargo sets `DEP_EVOLUTION_SHELL_LIBDIRS`, from `evo-sys`'s
// `links = "evolution-shell"` key, only for the build script of a package
// with a direct, non-build dependency on `evo-sys`.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");

    // This variable is absent only if `evo-sys` stops publishing it. If that
    // happens, the build must fail here instead of producing a module that
    // cannot load.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // Uses `-rpath`, not `-rpath-link`: the linker must record this path
        // in the file, not just use it during the link.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
