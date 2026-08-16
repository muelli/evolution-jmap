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

/// The class's own accessors, and the one thing asked of the page it extends.
///
/// The prefix is the *backend*'s, because that is the class this module
/// subclasses and every one of its accessors is a call its vfuncs may
/// legitimately make. The page is not subclassed and not extended by anything
/// here — it is only ever the object a backend hands back — so it is named a
/// function at a time, the way the GTK calls below are: `setup_defaults` needs
/// the address the user typed on the identity page and nothing else, and an
/// `e_mail_config_service_page_.*` would take on the page's scratch sources, its
/// backend lookup and its auto-configuration besides.
const ALLOWED_FUNCTIONS: &[&str] = &[
    "e_mail_config_service_backend_.*",
    "e_mail_config_service_page_get_email_address",
    // The one call `insert_widgets` needs back onto the page rather than the
    // backend: telling the assistant an entry changed, so `check_complete` is
    // asked again. See `EMailConfigPage` below on why the type it takes joins
    // the handles rather than being generated.
    "e_mail_config_page_changed",
];

/// The GTK calls, named one at a time rather than by prefix.
///
/// This is the whole of what M7's `insert_widgets` needs to build its page: a
/// grid packed into the `GtkBox` Evolution hands it, and in the grid a
/// right-aligned mnemonic label beside a horizontally-expanding entry, per
/// setting. Nothing here is a *convenience*; every name absent from this list is
/// a widget call the module cannot make.
///
/// One at a time and not `gtk_(grid|label|entry)_.*` because a prefix would take
/// on the rest of those classes' APIs — several hundred functions, most of them
/// touching a widget hierarchy that has to be realized, none of them exercised
/// by anything here. A binding surface is a promise that what is in it was
/// looked at, so it grows a line at a time with the code that calls it, and
/// `tests/gtk.rs` names each one back.
///
/// The property *bindings* that give these widgets their values are deliberately
/// not in here: they are `g_object_bind_property` on the `CamelSettings` object,
/// which is `gobject-sys` and already available, so a `GtkEntry`'s `text` needs
/// no GTK entry point of its own.
const ALLOWED_GTK_FUNCTIONS: &[&str] = &[
    // The container Evolution hands `insert_widgets`, and the grid that goes in
    // it.
    "gtk_box_pack_start",
    "gtk_grid_new",
    "gtk_grid_attach",
    "gtk_grid_set_row_spacing",
    "gtk_grid_set_column_spacing",
    // A label per setting: `_with_mnemonic` because the underscore in a
    // translated `_("_Server:")` is the keyboard shortcut, `set_mnemonic_widget`
    // because that shortcut has to land in the entry beside it, and
    // `set_xalign` to right-align it against the entry — the modern spelling of
    // what Evolution's older pages do with the deprecated `gtk_misc_set_alignment`.
    "gtk_label_new_with_mnemonic",
    "gtk_label_set_mnemonic_widget",
    "gtk_label_set_xalign",
    // The entry, and the two calls that make a row of them look like a form.
    "gtk_entry_new",
    "gtk_widget_set_hexpand",
    "gtk_widget_show_all",
    // The security toggle: one check button, bound (like the entries) through
    // a plain property binding rather than a `gtk_toggle_button_.*` call, so
    // this is the only entry point the toggle needs.
    "gtk_check_button_new_with_mnemonic",
    // Not called by the module: these are what `tests/gtk.rs` asks the running
    // GTK to confirm the classes above are, since the opaque handles carry no
    // layout to check the way `tests/layout.rs` checks the EDS structs.
    "gtk_widget_get_type",
    "gtk_container_get_type",
    "gtk_box_get_type",
    "gtk_grid_get_type",
    "gtk_label_get_type",
    "gtk_entry_get_type",
    "gtk_check_button_get_type",
];

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

/// GTK's *types*, which this crate does not generate even though it now calls
/// GTK functions.
///
/// There is no gtk-rs `gtk-sys` this crate could re-export them from: GTK 3's
/// sys crate is frozen at the 0.18 generation and depends on `glib-sys` 0.18,
/// while this workspace is on 0.22. Depending on both would put two
/// incompatible `GObject`s in one process — precisely the failure the blocklist
/// above exists to prevent — so the widget calls in [`ALLOWED_GTK_FUNCTIONS`]
/// are generated from these same headers rather than borrowed from a second
/// ecosystem, and the classes they take are declared opaque by
/// [`GTK_HANDLES`] instead of generated.
///
/// Opaque and not generated for two reasons. It keeps `GtkWidgetClass` and the
/// hundred vfunc slots below it out of a binding surface nobody reads; and,
/// more to the point, a generated layout for a GTK class is one nothing here
/// cross-checks — `tests/layout.rs` holds the EDS structs against
/// `g_type_query` because this crate *subclasses* them, whereas no GTK class is
/// subclassed, extended or allocated here. A struct definition would be a claim
/// about a layout for no reason to make one.
const BLOCKED_GTK_TYPES: &[&str] = &["Gtk.*", "_Gtk.*"];

/// Evolution's own page classes, blocked for exactly the reason the GTK ones
/// are: they are widgets.
///
/// `EMailConfigServicePage` is a `GtkScrolledWindow` two classes down
/// (`EMailConfigPage` → `EMailConfigActivityPage` → this), so generating it
/// means generating the GTK class structs the blocklist above exists to keep
/// out — bindgen says so plainly, with a `parent: GtkScrolledWindow` field
/// naming a type nothing here defines. It is also a class this crate has no
/// business knowing the shape of: nothing subclasses it, nothing allocates one,
/// and the single call made on it takes it as a pointer.
///
/// So the page joins [`EVO_HANDLES`] instead. The backend class next door is the
/// opposite case and stays generated: it *is* subclassed here, its layout is
/// what `class_init` writes vfunc pointers into, and `tests/layout.rs` holds
/// that layout against `g_type_query`.
const BLOCKED_EVO_TYPES: &[&str] = &["_?EMailConfig(Page|ActivityPage|ServicePage)[A-Za-z]*"];

/// Evolution's classes reached only through a pointer, as opaque handles — the
/// same zero-sized `#[repr(C)]` treatment, and the same claim, as [`GTK_HANDLES`].
///
/// The page a service backend extends, and the interface it implements.
///
/// `EMailConfigServicePage` is `tests/page.rs`'s subject: it stays zero-sized,
/// and is the one type both `e_mail_config_service_backend_get_page` and
/// `e_mail_config_service_page_get_email_address` speak of — the whole of what
/// `setup_defaults` does with a page.
///
/// `EMailConfigPage` is the interface `insert_widgets` needs instead:
/// `e_mail_config_page_changed` takes one, not a service page, because the
/// "tell the assistant an entry changed" call is on the interface every config
/// page implements, not on this provider's own subclass. Read against
/// `e-mail-config-service-page.c` (tag 3.52.3) rather than assumed —
/// `E_MAIL_CONFIG_SERVICE_PAGE_TYPE`'s `G_IMPLEMENT_INTERFACE (
/// E_TYPE_MAIL_CONFIG_PAGE, …)` is where that relationship is actually stated —
/// so the page handle `e_mail_config_service_backend_get_page` returns is one
/// `insert_widgets` may cast straight to this handle and hand to the call, the
/// same pointer-is-the-same-object cast `E_MAIL_CONFIG_PAGE()` is in C.
/// `BLOCKED_EVO_TYPES`'s pattern already covers this name, so it joins the
/// handle list rather than being bindgen's own guess at its layout — nothing
/// here subclasses it or reads a field of it either.
const EVO_HANDLES: &[&str] = &["EMailConfigServicePage", "EMailConfigPage"];

/// The GTK classes the calls above mention, as opaque handles.
///
/// One `#[repr(C)]` zero-sized struct each, with a private field, so that
/// nothing outside this crate can construct one and nothing inside it can read
/// through one: a pointer to any of these is only ever handed straight back to
/// GTK. `tests/gtk.rs` asserts they all stay zero-sized, which is the
/// machine-checkable form of "this crate does not know what a widget looks
/// like".
///
/// Separate types rather than one alias for `c_void`, which is what `GtkBox`
/// was while there were no calls to make with it. GTK's C API takes the same
/// object as a `GtkGrid *` in one call and a `GtkWidget *` in the next, and with
/// distinct types each of those crossings has to be written as a `.cast()` at
/// the call site — the Rust spelling of C's `GTK_GRID()`. Aliasing them all to
/// `c_void` would instead let a `GtkBox` be passed to `gtk_grid_attach`, which
/// compiles, is undefined behaviour, and is a mistake this file can simply
/// prevent. What licenses the casts that remain is that GTK really does relate
/// the classes this way, which is the one thing `tests/gtk.rs` asks the running
/// type system rather than assuming.
const GTK_HANDLES: &[&str] = &["GtkWidget", "GtkBox", "GtkGrid", "GtkLabel", "GtkEntry"];

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    let mut clang_args = Vec::new();
    let mut link_paths: Vec<String> = Vec::new();
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
        //
        // That forwarding stops at this package, though: Cargo scopes
        // `rustc-link-arg` to the crate whose build script emitted it, while
        // the `-l` flags it also emits are passed on to everything that links
        // this crate. A dependent therefore links fine and comes out with no
        // `RUNPATH` at all, which is a binary that cannot start. So the
        // directories are published as build-script metadata — reachable to a
        // dependent as `DEP_EVOLUTION_SHELL_LIBDIRS`, by way of this crate's
        // `links` key — and `jmap-config`'s own `build.rs` turns them back into
        // an rpath for the binaries it produces.
        for path in &lib.link_paths {
            let path = path.display().to_string();
            if !link_paths.contains(&path) {
                link_paths.push(path);
            }
        }
    }

    // A colon-separated list, as every other library path in the loader's world
    // is; a directory with a colon in its name would break it, and would break
    // `LD_LIBRARY_PATH` and `-Wl,-rpath` in exactly the same way.
    println!("cargo:libdirs={}", link_paths.join(":"));

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
        .raw_line("pub use eds_sys::*;");

    // The GTK classes these headers mention, as opaque handles; see GTK_HANDLES.
    for t in GTK_HANDLES {
        builder = builder.raw_line(format!(
            "#[repr(C)]\npub struct {t} {{\n    _opaque: [u8; 0],\n}}"
        ));
    }

    // And Evolution's, which need the struct tag as well as the typedef: unlike
    // the GTK ones, which bindgen writes into the signatures under their typedef
    // name, a blocklisted `EMailConfigServicePage` comes through as
    // `_EMailConfigServicePage` — the spelling the header's `typedef struct`
    // introduces. So the handle is defined under the tag, which is what the
    // generated declarations refer to, and aliased to the name the headers and
    // the callers use; that is also how bindgen itself writes every Evolution
    // class it does generate.
    for t in EVO_HANDLES {
        builder = builder.raw_line(format!(
            "#[repr(C)]\npub struct _{t} {{\n    _opaque: [u8; 0],\n}}\npub type {t} = _{t};"
        ));
    }

    for t in ALLOWED_TYPES {
        builder = builder.allowlist_type(t);
    }
    for f in ALLOWED_FUNCTIONS.iter().chain(ALLOWED_GTK_FUNCTIONS) {
        builder = builder.allowlist_function(f);
    }
    for t in BLOCKED_TYPES
        .iter()
        .chain(BLOCKED_GTK_TYPES)
        .chain(BLOCKED_EVO_TYPES)
    {
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
