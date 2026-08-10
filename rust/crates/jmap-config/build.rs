// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Records where Evolution's own libraries live in everything this crate links.
//
// Evolution installs `libevolution-mail.so.0` and its siblings into a private
// directory (`/usr/lib/evolution`) rather than into the system one, so a binary
// that does not carry that directory in its `RUNPATH` links successfully and
// then fails to start — `error while loading shared libraries`, before `main`.
//
// `evo-sys` cannot arrange it on this crate's behalf. Cargo passes a build
// script's `-l` and `-L` flags on to every crate downstream, but scopes
// `rustc-link-arg` to the package that emitted it, and the `-Wl,-R` that
// Evolution's own `.pc` files carry arrives as one of the latter. So `evo-sys`
// publishes the directories as metadata instead — `cargo:libdirs`, which Cargo
// hands to the dependents of a crate with a `links` key as
// `DEP_EVOLUTION_SHELL_LIBDIRS` — and this turns them back into an rpath for
// the test binaries here, and for the module this crate will eventually be
// installed as.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");

    // Absent only if `evo-sys` stopped publishing it, which is a build to fail
    // rather than one to quietly produce binaries that cannot start.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // `-rpath` and not `-rpath-link`: this has to be recorded in the file,
        // not merely used while linking it.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
