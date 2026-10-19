// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
// Sets RPATH for Evolution private library directories so cdylib resolves dependencies when dlopened.
// `evo-sys` is in dependencies solely to receive `DEP_EVOLUTION_SHELL_LIBDIRS`.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_EVOLUTION_SHELL_LIBDIRS");

    // Fail the build if evo-sys does not publish DEP_EVOLUTION_SHELL_LIBDIRS.
    let libdirs = env::var("DEP_EVOLUTION_SHELL_LIBDIRS")
        .expect("evo-sys published no DEP_EVOLUTION_SHELL_LIBDIRS");
    for dir in libdirs.split(':').filter(|dir| !dir.is_empty()) {
        // Record directory paths in the binary via -rpath.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
