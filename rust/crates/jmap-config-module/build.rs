// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Records the location of the Evolution libraries in the cdylib that this
// crate makes (`module-jmap-configuration.so`).
//
// The `build.rs` of `jmap-config` solves the same problem, but only for its own
// package: `cargo:rustc-link-arg` applies only to the package whose build
// script emits it. This separate crate (`crate-type = ["cdylib"]`) builds the
// module that Evolution loads, not `jmap-config`. Without this script, the
// module has no `RUNPATH`/`RPATH`, unlike each module that Evolution installs
// next to it. Then `dlopen` fails before the code of this crate runs, unless
// `libevolution-mail.so.0` is already loaded in the process or
// `LD_LIBRARY_PATH` points to `/usr/lib/evolution`. Thus the account setup
// module silently does not register with a real `EConfigLookup`.
//
// `evo-sys` is in `[dependencies]` only for its metadata: this crate calls none
// of its bindings. Cargo sets `DEP_EVOLUTION_SHELL_LIBDIRS` (from the
// `links = "evolution-shell"` key of `evo-sys`) only for the build script of a
// package with a direct, non-build dependency on it.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");

    // Missing only if `evo-sys` does not publish it. Then fail the build, and do
    // not silently make a module that cannot load.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // `-rpath` and not `-rpath-link`: the file must record it. The linker
        // must not only use it during the link.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
