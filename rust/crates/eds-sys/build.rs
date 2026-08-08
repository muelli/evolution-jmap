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
    // A calendar object, in the two shapes the `ECalMetaBackend` vfuncs use
    // it: libical-glib's `ICalComponent`, which `load_component_sync` hands
    // back, and libecal's `ECalComponent` wrapper, which is what
    // `save_component_sync` is given a list of. Both are pulled in
    // transitively already, as field and argument types; naming them here is
    // what also brings their *class* structs, which tests/layout.rs checks
    // against `g_type_query` like every other type we cross the ABI with.
    "ICal.*",
    "ECalComponent.*",
    // The error domain every EDS client speaks. Deliberately just the enum,
    // not the whole EClient class: the backends produce these codes, they
    // never talk to an EClient.
    "EClientError",
    // The address-book-specific half of it. `EBookMetaBackend` itself matches
    // on `E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND` — a load that fails with it
    // means "drop this one from the cache" rather than "the sync failed" — so
    // reporting a missing contact any other way is a stuck cache entry.
    "EBookClientError",
    // And the calendar's, for the same reason and the same match:
    // `E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND` is how a component says it is gone.
    // The two domains are separate quarks, so the book's code cannot stand in
    // for it — tests/errors.rs pins that.
    "ECalClientError",
    // M5's mail provider. `CamelProvider` is the static struct a
    // `libcameljmap.so` registers on load, and the rest is the object graph it
    // names: a store (offline, so the summary cache works disconnected), a
    // transport, their common `CamelService` parent, the settings objects a
    // service is configured through and the session that owns it.
    "CamelProvider.*",
    "CamelService.*",
    "CamelStore.*",
    "CamelOfflineStore.*",
    "CamelTransport.*",
    "CamelSession.*",
    "CamelSettings.*",
    "CamelNetworkSettings.*",
    // The settings object a JMAP service is configured through. Camel's stock
    // classes stop at `CamelOfflineSettings` and none of them implements
    // `CamelNetworkSettings` (tests/camel.rs pins that), so the provider
    // declares its own subclass of the former that implements the latter —
    // which needs both the parent's structs and the enum the `security-method`
    // property carries.
    "CamelOfflineSettings.*",
    "CamelNetworkSecurityMethod",
    "CamelURL",
    // The folder half of a store. `CamelFolderInfo` is what
    // `get_folder_info_sync` returns — a plain struct, not an object, so it is
    // named here to bring its flags enum along; `CamelFolder` is the object the
    // provider's own folder derives from, through `CamelOfflineFolder`, which
    // is where a folder's disconnected copy of its content lives.
    // `CamelFolderSummary` is what that folder's rows live in — the object a
    // message count, a uid list and a `CamelMessageInfo` all come out of.
    // Exact names rather than a `CamelFolder.*` prefix, which would also pull
    // in `CamelFolderSearch` and `CamelFolderThread` — two more class structs
    // this layer would be claiming to have checked.
    "CamelFolder",
    "CamelFolderClass",
    // What a folder tells Camel it has changed — four uid lists, and the
    // argument of the `changed` signal Evolution redraws a message list on.
    // Plain struct behind a boxed type, like `CamelProvider`, so `g_type_query`
    // knows nothing of it and tests/camel.rs stands in for tests/layout.rs.
    "CamelFolderChangeInfo",
    "CamelFolderInfo",
    "CamelFolderInfoFlags",
    "CamelFolderSummary",
    "CamelFolderSummaryClass",
    // The flags word of the folder *object*, which is a different word from
    // `CamelFolderInfoFlags` with different bits in it: the info's flags say
    // what kind of folder this is, the folder's say how Camel treats it —
    // whether new mail in it is filtered, whether it is the account's trash.
    // Two enums one bit-width apart is exactly the kind of thing that is only
    // caught by naming both.
    "CamelFolderFlags",
    "CamelOfflineFolder.*",
    // A folder's contents, a row at a time. `CamelMessageInfo` is the object
    // Camel keeps one summary row in and `CamelMessageInfoBase` the subclass
    // that actually stores the columns — the one a summary with no message-info
    // type of its own instantiates, which is this provider's case.
    // `CamelMessageFlags` is the row's flags word, a third flags enum with a
    // third set of bits, and `CamelSummaryMessageID` the union that says how
    // wide a stored message id is. `CamelNamedFlags` and `CamelNameValueArray`
    // are what the user flags and the headers come back as.
    "CamelMessageInfo.*",
    "CamelMessageFlags",
    "CamelSummaryMessageID",
    "CamelNamedFlags",
    "CamelNameValueArray",
    // How an address list becomes the single string a summary row stores.
    // `CamelInternetAddress` is the RFC 5322 one, `CamelAddress` its abstract
    // parent, which is where the formatting entry point is declared.
    "CamelAddress.*",
    "CamelInternetAddress.*",
];

const ALLOWED_FUNCTIONS: &[&str] = &[
    "e_backend_.*",
    "e_(book|cal)_backend_.*",
    "e_(book|cal)_meta_backend_.*",
    "e_data_(book|cal)_.*",
    "e_(book|cal)_cache_.*",
    "e_cache_.*",
    "e_source_.*",
    // The registry's own side of a `.source` file. No backend calls these —
    // it is handed a finished `ESource` — but they are how a keyfile becomes
    // one without a running `evolution-source-registry`, which is what lets
    // the documented manual test recipe be checked by a test. M6's collection
    // backend meets server-side sources for real.
    "e_server_side_source_.*",
    "e_collection_backend_.*",
    "e_contact_.*",
    "e_vcard_.*",
    // Building, reading and rendering a calendar object. `i_cal_component_.*`
    // rather than all of `i_cal_.*`: the mapping in `jmap-ical` does the
    // property- and value-level work in Rust on the text, so the component is
    // the only libical type that has to cross the boundary.
    "i_cal_component_.*",
    "e_cal_component_.*",
    "e_client_error_.*",
    "e_(book|cal)_client_error_.*",
    // How a backend is handed its credentials: EDS fetches them from
    // libsecret and passes an ENamedParameters to connect_sync.
    "e_named_parameters_.*",
    // Not an EDS symbol, but the entry point every loadable EDS module must
    // export; having the signature in scope keeps M2's trampoline honest.
    "e_module_.*",
    // The mail provider's side. `camel_provider_module_init` is in here for the
    // same reason as `e_module_load`: it is declared, not defined, by Camel, so
    // having the declaration in scope is what makes the module's `extern "C"`
    // definition a signature the compiler can check rather than a guess.
    "camel_provider_.*",
    "camel_service_.*",
    "camel_store_.*",
    "camel_offline_store_.*",
    // `camel_folder_info_new` and `_free` are the allocator pair the folder
    // tree is built and torn down with; the type accessor is what
    // tests/layout.rs queries.
    "camel_folder_info_.*",
    "camel_folder_get_type",
    // What a constructed folder is asked about itself: the path Camel keys it
    // by, the name the user sees, the store it hangs off, and the flags word
    // that says how Camel treats it. Still not all of `camel_folder_.*`, which
    // would match every `camel_folder_search_*` function too and drag its class
    // struct in behind them.
    "camel_folder_(get|set)_(full_name|display_name|flags)",
    "camel_folder_get_parent_store",
    // The summary a folder keeps its rows in: `take_folder_summary` is how one
    // is put on a folder — it takes the reference rather than adding to it —
    // `get_folder_summary` how it is read back, and
    // `has_summary_capability` is the flag test Camel itself makes before it
    // asks a folder for a message count.
    "camel_folder_(get|take)_folder_summary",
    "camel_folder_has_summary_capability",
    // What a folder is asked for once it has a summary to answer from, and
    // what it says back when a refresh changed something.
    // `camel_folder_refresh_info_sync` is the wrapper around the vfunc this
    // provider overrides — named so a test can call it the way Camel does —
    // `get_message_count` and `get_uids` are the two questions the base class
    // answers straight out of the summary, and `camel_folder_changed` is how
    // the answer to them reaches a window that is already open.
    "camel_folder_refresh_info_sync",
    "camel_folder_get_message_count",
    "camel_folder_(get|free)_uids",
    "camel_folder_changed",
    "camel_folder_change_info_.*",
    "camel_folder_summary_.*",
    "camel_offline_folder_get_type",
    // Filling and reading one summary row. `camel_message_info_new_from_headers`
    // rides in on the same prefix although it is declared in
    // camel-folder-summary.h, and that is wanted rather than tolerated: it is
    // the path a message parsed locally takes, and therefore the only oracle
    // there is for the two columns a provider has to *compute* — the 64-bit
    // digests Camel threads on. A provider whose digests disagreed with it
    // would split a conversation in two the moment the two met.
    "camel_message_info_.*",
    // The set a row's user flags — Evolution's labels — are kept in. Replaced
    // wholesale rather than flag by flag, because JMAP's `keywords` is the
    // whole truth about a message's labels: a keyword the server stopped
    // sending is one the user took off somewhere else.
    "camel_named_flags_.*",
    "camel_name_value_array_.*",
    // Formatting an address list the way a summary row stores it: build a
    // `CamelInternetAddress`, then `camel_address_format` on it.
    "camel_address_.*",
    "camel_internet_address_.*",
    "camel_transport_.*",
    "camel_session_.*",
    "camel_settings_.*",
    "camel_network_settings_.*",
    "camel_offline_settings_.*",
];

/// The names an `ESource` extension is looked up by are `#define`d strings,
/// not exported symbols, so a typo in one is not a link error — it is an
/// address book that silently reports no host. Take them from the headers
/// rather than retyping them. `E_SOURCE_CREDENTIAL_*` are the same thing for
/// the `ENamedParameters` a backend is authenticated with, where a typo is a
/// password that reads back as absent.
/// `EDS_CAMEL_PROVIDER_DIR` is the same kind of thing on Camel's side: the
/// environment variable that points the provider loader at an uninstalled
/// build, which the manual test recipe will need and which is a `#define`, so a
/// typo is a provider directory that is silently never searched.
/// `CAMEL_FOLDER_TYPE_BIT` and `_MASK` are the same class of thing again: a
/// folder's type is a small integer packed into the flags word, and the shift
/// and the mask are `#define`s. Retyping either is a folder that reads back as
/// some other type.
const ALLOWED_VARS: &[&str] = &[
    "E_SOURCE_EXTENSION_.*",
    "E_SOURCE_CREDENTIAL_.*",
    "EDS_CAMEL_PROVIDER_DIR",
    "CAMEL_FOLDER_TYPE_BIT",
    "CAMEL_FOLDER_TYPE_MASK",
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
    for pkg in [
        "libebackend-1.2",
        "libedata-book-1.2",
        "libedata-cal-2.0",
        // Camel ships in the same tarball and carries the same version, so the
        // 3.52 floor applies unchanged.
        "camel-1.2",
    ] {
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
        // String `#define`s come out as `&CStr`, so passing one to a
        // `*const gchar` parameter is `.as_ptr()` and not a cast from a byte
        // array that may or may not carry its NUL.
        .generate_cstr(true)
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
    for v in ALLOWED_VARS {
        builder = builder.allowlist_var(v);
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
