// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Generates the EDS bindings at build time rather than checking them in: the
// struct layouts differ between EDS releases, and a stale committed binding
// would be a silently-wrong ABI rather than a build failure. tests/layout.rs
// cross-checks the result against the running GObject type system.

use std::env;
use std::path::PathBuf;

/// EDS 3.52 is the target platform (Debian trixie / Fedora 40 era). Older
/// releases lack `e_book_meta_backend_*`, which the whole design rests on.
const MIN_EDS: &str = "3.52";

/// Everything the backends touch, and nothing else — a bare `libebackend.h`
/// pulls in libsoup, libxml2 and json-glib, which would take minutes to
/// generate and produce a binding surface nobody reads.
const ALLOWED_TYPES: &[&str] = &[
    "EBackend.*",
    "E(Book|Cal)Backend.*",
    "E(Book|Cal)MetaBackend.*",
    "EData(Book|Cal).*",
    "E(Book|Cal)Cache.*",
    "ECache.*",
    "ESource.*",
    "ECollectionBackend.*",
    "EModule.*",
    "EExtension.*",
    "EContact.*",
    "EVCard.*",
    "EComponent.*",
];

const ALLOWED_FUNCTIONS: &[&str] = &[
    "e_backend_.*",
    "e_(book|cal)_backend_.*",
    "e_(book|cal)_meta_backend_.*",
    "e_data_(book|cal)_.*",
    "e_(book|cal)_cache_.*",
    "e_cache_.*",
    "e_source_.*",
    "e_collection_backend_.*",
    "e_contact_.*",
    "e_vcard_.*",
    // Not an EDS symbol, but the entry point every loadable EDS module must
    // export; having the signature in scope keeps M2's trampoline honest.
    "e_module_.*",
];

/// `GType` and friends come from the gtk-rs sys crates so that eds-sys
/// interoperates with the wider glib ecosystem instead of minting its own
/// incompatible `GObject`. Anything matching these is emitted as a bare name
/// and resolved through the glob re-exports below.
///
/// Both spellings have to be listed. Blocking only the `GObject` typedef
/// makes bindgen fall back to emitting the `_GObject` *tag* struct the header
/// declares alongside it — a second, incompatible `GObject` sitting in the
/// parent slot of every EDS instance struct, with the right layout and the
/// wrong identity. With the tag blocked too, bindgen uses the typedef name
/// and picks up the gtk-rs one.
const BLOCKED_TYPES: &[&str] = &[
    "G[A-Z].*",
    "_G[A-Z].*",
    "g[a-z]+",
    "va_list",
    "__va_list_tag",
];

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    // The pkg_config crate emits the cargo:rustc-link-lib/-search lines for
    // us; we only need the include paths for clang.
    let mut clang_args = vec!["-DE_CAL_DISABLE_DEPRECATED".to_string()];
    for pkg in ["libebackend-1.2", "libedata-book-1.2", "libedata-cal-2.0"] {
        let lib = pkg_config::Config::new()
            .atleast_version(MIN_EDS)
            .probe(pkg)
            .unwrap_or_else(|e| panic!("{pkg} >= {MIN_EDS} not found: {e}"));
        clang_args.extend(
            lib.include_paths
                .iter()
                .map(|p| format!("-I{}", p.display())),
        );
    }

    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(clang_args)
        // The headers are not ours and are full of deprecations; their
        // warnings would drown any real problem.
        .clang_arg("-Wno-deprecated-declarations")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Reproducible builds: no comment text (it embeds header paths on
        // some clang versions) and a stable item order.
        .generate_comments(false)
        .layout_tests(false)
        .derive_default(false)
        .prepend_enum_name(false)
        // Plain consts, not `ModuleConsts`: the blocklisted GLib enums are
        // type aliases in the gtk-rs sys crates, and a module-per-enum style
        // makes bindgen refer to them as `GSomeFlags::Type`, which does not
        // exist there.
        .default_enum_style(bindgen::EnumVariation::Consts)
        // Edition 2024 denies unsafe ops in unsafe fns; let bindgen write the
        // inner `unsafe` blocks instead of blanket-allowing the lint.
        .wrap_unsafe_ops(true)
        .raw_line("pub use glib_sys::*;")
        .raw_line("pub use gobject_sys::*;")
        .raw_line("pub use gio_sys::*;")
        // glib-sys and gobject-sys both export these (same C symbols, two
        // declarations), which makes the globs above ambiguous. An explicit
        // re-export takes precedence over a glob and settles it.
        .raw_line(
            "pub use glib_sys::{GIOCondition, g_io_condition_get_type, g_variant_get_gtype, \
             G_IO_ERR, G_IO_HUP, G_IO_IN, G_IO_NVAL, G_IO_OUT, G_IO_PRI};",
        );

    for t in ALLOWED_TYPES {
        builder = builder.allowlist_type(t);
    }
    for f in ALLOWED_FUNCTIONS {
        builder = builder.allowlist_function(f);
    }
    for t in BLOCKED_TYPES {
        builder = builder.blocklist_type(t);
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed on the EDS headers");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR unset"));
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("could not write bindings.rs");
}
