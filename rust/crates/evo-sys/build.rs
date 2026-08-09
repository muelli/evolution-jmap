// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The same arrangement as eds-sys's build.rs, one library up: generated at
// build time rather than checked in, because Evolution's class structs change
// between releases and a stale committed binding is a silently-wrong ABI
// rather than a build failure. tests/layout.rs cross-checks the result against
// the running GObject type system.

use std::env;
use std::path::PathBuf;

/// Evolution 3.52 is the target, as it is for EDS: the milestone note in the
/// roadmap is that 3.56 replaced GtkUIManager, so a floor lower than this one
/// would be claiming support for releases nothing here has been held against.
const MIN_EVO: &str = "3.52";

/// The one class the setup module subclasses, and its accessors. Everything
/// else in Evolution's headers — the pages, the assistant, the notebook — is
/// reached through the vfuncs of this class or not at all, so the surface this
/// crate claims to have audited stays one file wide.
const ALLOWED_TYPES: &[&str] = &["EMailConfigServiceBackend.*"];

const ALLOWED_FUNCTIONS: &[&str] = &["e_mail_config_service_backend_.*"];

/// Types that already exist, and must not be minted a second time.
///
/// Two lots of them, for the same reason with two different owners. The `G*`
/// spellings are eds-sys's argument verbatim: a regenerated `GObject` has the
/// right layout and the wrong identity, so it is blocked and re-exported from
/// the gtk-rs sys crates. The `E*` and `Camel*` ones are the same argument
/// against *this* crate: `EMailConfigServiceBackend` is an `EExtension` and
/// hands out `ESource`s, `CamelProvider`s and `CamelSettings`, all of which
/// eds-sys already generated from the very same headers. Blocked here and
/// re-exported from there, they stay one type, which is what lets the module's
/// vfuncs be written in terms of the crates that already read and write those
/// objects.
///
/// `ESourceRegistry` and friends match the `ESource` prefix and would be
/// blocked with it; that is correct — eds-sys has them too.
const BLOCKED_TYPES: &[&str] = &[
    "G[A-Z].*",
    "_G[A-Z].*",
    "g[a-z]+",
    "va_list",
    "__va_list_tag",
    "E(Extension|Source)[A-Za-z]*",
    "_E(Extension|Source)[A-Za-z]*",
    "Camel.*",
    "_Camel.*",
];

/// GTK, which this crate deliberately does *not* speak yet.
///
/// `GtkBox` is in these headers only as the container `insert_widgets` is
/// handed, and there is no gtk-rs `gtk-sys` this crate could re-export it from:
/// GTK 3's sys crate is frozen at the 0.18 generation and depends on
/// `glib-sys` 0.18, while this workspace is on 0.22. Depending on both would
/// put two incompatible `GObject`s in one process — precisely the failure the
/// blocklist above exists to prevent — so the pointer types are declared
/// opaque here instead, and the widget calls M7 eventually needs will be
/// generated from these same headers rather than borrowed from a second
/// ecosystem.
///
/// Opaque and not generated also keeps `GtkWidgetClass` and the hundred vfuncs
/// below it out of a binding surface nobody reads.
const BLOCKED_GTK_TYPES: &[&str] = &["Gtk.*", "_Gtk.*"];

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    let mut clang_args = Vec::new();
    for pkg in ["evolution-shell-3.0", "evolution-mail-3.0"] {
        let lib = pkg_config::Config::new()
            .atleast_version(MIN_EVO)
            .probe(pkg)
            .unwrap_or_else(|e| panic!("{pkg} >= {MIN_EVO} not found: {e}"));
        clang_args.extend(
            lib.include_paths
                .iter()
                .map(|p| format!("-I{}", p.display())),
        );
        // Evolution's libraries live in its own private directory
        // (`/usr/lib/evolution`) rather than in the system one, so a binary
        // that links them has to record where they were found or it will not
        // start. Nothing here has to arrange that: both `.pc` files carry a
        // `-Wl,-R<libdir>` in their `Libs:` for exactly this reason, and the
        // `pkg_config` crate forwards it as a link argument — the test binaries
        // come out with the directory in their `RUNPATH`, checked with
        // `readelf -d`. Emitting a second copy from `lib.link_paths` here was
        // tried and only duplicated the entry.
    }

    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(clang_args)
        .clang_arg("-Wno-deprecated-declarations")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Reproducible builds, as in eds-sys.
        .generate_comments(false)
        .layout_tests(false)
        .derive_default(false)
        .prepend_enum_name(false)
        .default_enum_style(bindgen::EnumVariation::Consts)
        .generate_cstr(true)
        .wrap_unsafe_ops(true)
        // One glob, not four: eds-sys re-exports glib-sys, gobject-sys and
        // gio-sys itself, so taking its namespace whole is what makes an
        // `ESource *` here the same Rust type as an `ESource *` in the
        // backends.
        .raw_line("pub use eds_sys::*;")
        // The GTK types these headers mention, as pointers only. `c_void` and
        // not a zero-sized struct: a pointer to one of these is only ever
        // passed back to Evolution or to GTK, and this way nothing in Rust can
        // be tempted to construct one.
        .raw_line("pub type GtkBox = ::std::ffi::c_void;");

    for t in ALLOWED_TYPES {
        builder = builder.allowlist_type(t);
    }
    for f in ALLOWED_FUNCTIONS {
        builder = builder.allowlist_function(f);
    }
    for t in BLOCKED_TYPES.iter().chain(BLOCKED_GTK_TYPES) {
        builder = builder.blocklist_type(t);
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed on the Evolution headers");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR unset"));
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("could not write bindings.rs");
}
