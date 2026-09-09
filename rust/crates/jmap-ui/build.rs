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
// the test binaries here. It does NOT reach `jmap-config-module`, the
// separate cdylib crate the account-setup module is actually built from
// (rustc-link-arg is package-scoped, and that crate is a different package) —
// that crate carries its own copy of this exact build script for the same
// reason.

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

    // Which menu/action layer this crate's own `#[cfg(evolution_eui_manager)]`
    // blocks pick: `evo-sys`'s build script decided it against the Evolution
    // headers actually installed and published the answer the same way it
    // publishes `libdirs` — this crate reads it back and re-emits its own
    // `rustc-cfg`, `cargo:rustc-cfg` being scoped to the crate that sets it.
    let eui_manager = env::var("DEP_EVOLUTION_SHELL_EUI_MANAGER")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_EUI_MANAGER");
    if eui_manager == "1" {
        println!("cargo:rustc-cfg=evolution_eui_manager");
    }
}
