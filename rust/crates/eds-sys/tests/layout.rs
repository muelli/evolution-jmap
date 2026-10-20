// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The bindings come from the installed EDS headers. Nothing else makes sure
// that a Rust `struct` to subclass has the shape of the C struct that the
// runtime allocates. `g_type_query()` reports the instance and class sizes
// that GObject uses for `g_type_register_static()`, and they must agree with
// `size_of`. If they do not, each vfunc override in the backends silently
// writes at the wrong offset. Thus this is the most important test of the FFI
// layer.

use eds_sys::*;
use std::mem::size_of;

/// `g_type_query()` fills the struct only for a classed type whose class has a
/// reference, so take a class ref first. Abstract types (all the meta
/// backends) accept a class ref; only an instance of them fails.
fn query(gtype: GType) -> GTypeQuery {
    unsafe {
        let klass = g_type_class_ref(gtype);
        assert!(!klass.is_null(), "g_type_class_ref returned NULL");
        let mut q = std::mem::zeroed::<GTypeQuery>();
        g_type_query(gtype, &mut q);
        g_type_class_unref(klass);
        assert_ne!(q.type_, 0, "g_type_query left the query zeroed");
        q
    }
}

/// The EDS that this test binary was built against and runs on, for the
/// reader of a failure.
///
/// This file runs against a matrix of EDS releases. A mismatch report must
/// name the type and the version, because people read the log of a leg after
/// its container is gone. Three versions can disagree, and each disagreement
/// is a different fault:
///
/// - the version that pkg-config resolved when `build.rs` chose the include
///   paths ([`EDS_HEADER_VERSION`]),
/// - the version that the headers state (`EDS_MAJOR_VERSION` and its two
///   siblings, `#define`d in `libedataserver/eds-version.h`), and
/// - the version of the shared library that this process loaded
///   (`eds_major_version` and its siblings, which are `extern const guint`).
///
/// If the first two disagree, the `.pc` file does not agree with its headers.
/// If the last two disagree, a build for one EDS runs on a different EDS. The
/// ABI contract in `docs/eds-versions.md` forbids this.
fn eds_versions() -> String {
    format!(
        "EDS {EDS_HEADER_VERSION} (pkg-config), headers {}, runtime {}",
        header_version(),
        runtime_version(),
    )
}

/// The version of the installed headers, from the `#define`s that bindgen
/// copied.
fn header_version() -> String {
    format!("{EDS_MAJOR_VERSION}.{EDS_MINOR_VERSION}.{EDS_MICRO_VERSION}")
}

/// The version of the loaded `libedataserver`. Only this one of the three
/// describes the code that this process calls.
fn runtime_version() -> String {
    unsafe { format!("{eds_major_version}.{eds_minor_version}.{eds_micro_version}") }
}

/// The message of a failed size check: the type that changed and its EDS. One
/// function makes the message testable, without a format string in each
/// assertion.
fn layout_message(name: &str, which: &str) -> String {
    format!(
        "{name}: {which} size disagrees with g_type_query, under {}",
        eds_versions()
    )
}

/// Checks one instance/class struct pair against the registered type.
macro_rules! assert_layout {
    ($get_type:expr, $instance:ty, $class:ty) => {{
        let name = stringify!($instance);
        let q = query(unsafe { $get_type() });
        assert_eq!(
            q.instance_size as usize,
            size_of::<$instance>(),
            "{}",
            layout_message(name, "instance")
        );
        assert_eq!(
            q.class_size as usize,
            size_of::<$class>(),
            "{}",
            layout_message(name, "class")
        );
    }};
}

/// A mismatch must fail "loudly, with the version and the offending type in
/// the output". In a matrix of otherwise identical legs, a message without
/// the version is not sufficient.
#[test]
fn a_layout_failure_names_the_type_and_the_eds_it_ran_against() {
    let message = layout_message("EBookMetaBackend", "instance");
    assert!(
        message.contains("EBookMetaBackend"),
        "the offending type is missing from {message:?}"
    );
    assert!(
        message.contains(EDS_HEADER_VERSION),
        "the pkg-config version is missing from {message:?}"
    );
    assert!(
        message.contains(&header_version()),
        "the header version is missing from {message:?}"
    );
    assert!(
        message.contains(&runtime_version()),
        "the runtime version is missing from {message:?}"
    );
}

/// The `.pc` file from pkg-config must agree with its headers. If not,
/// `build.rs` chose its include paths from a version that bindgen did not read.
/// The test compares only the release part, because pkg-config keeps a suffix
/// that a distribution adds, and the `#define`s cannot.
#[test]
fn pkg_config_describes_the_headers_it_pointed_at() {
    let stated = header_version();
    assert!(
        EDS_HEADER_VERSION.starts_with(&stated),
        "pkg-config says EDS {EDS_HEADER_VERSION}, the headers say {stated}"
    );
}

/// The ABI contract: an EDS module must run against the version of these
/// libraries that it was built against. `eds_check_version` returns NULL when
/// the running library can serve code compiled for the given version, and an
/// English explanation when it cannot.
///
/// This check changes "the plugin segfaults on a newer EDS" into a failed
/// test. It uses the compiled-in `#define`s, so it is correct for each EDS
/// that a matrix leg installs.
#[test]
fn the_running_eds_can_serve_what_these_bindings_were_compiled_against() {
    let complaint =
        unsafe { eds_check_version(EDS_MAJOR_VERSION, EDS_MINOR_VERSION, EDS_MICRO_VERSION) };
    if !complaint.is_null() {
        let text = unsafe { std::ffi::CStr::from_ptr(complaint) };
        panic!("{}: {}", eds_versions(), text.to_string_lossy());
    }
}

#[test]
fn backend_layouts_match_the_gtype_system() {
    assert_layout!(e_backend_get_type, EBackend, EBackendClass);
    assert_layout!(e_source_get_type, ESource, ESourceClass);
}

/// The type that the collection backend subclasses. It is in `libebackend`
/// beside `EBackend`, not in a data library. `evolution-source-registry`
/// dispatches its vfuncs, not a factory subprocess. Thus a layout error here
/// fails in the process that owns all accounts, not in one address book.
#[test]
fn collection_backend_layout_matches_the_gtype_system() {
    assert_layout!(
        e_collection_backend_get_type,
        ECollectionBackend,
        ECollectionBackendClass
    );
}

/// The slots that the collection backend overrides, as for the meta backends
/// below. A changed name or signature is then a compile error here, not a
/// call with the wrong arity at run time.
#[test]
fn the_collection_backend_class_exposes_the_vfuncs_the_backend_overrides() {
    let collection = unsafe { std::mem::zeroed::<ECollectionBackendClass>() };
    assert!(collection.populate.is_none());
    assert!(collection.dup_resource_id.is_none());
    assert!(collection.child_added.is_none());
    assert!(collection.child_removed.is_none());
}

#[test]
fn book_backend_layouts_match_the_gtype_system() {
    assert_layout!(e_book_backend_get_type, EBookBackend, EBookBackendClass);
    assert_layout!(
        e_book_meta_backend_get_type,
        EBookMetaBackend,
        EBookMetaBackendClass
    );
    assert_layout!(e_book_cache_get_type, EBookCache, EBookCacheClass);
}

#[test]
fn cal_backend_layouts_match_the_gtype_system() {
    assert_layout!(e_cal_backend_get_type, ECalBackend, ECalBackendClass);
    assert_layout!(
        e_cal_meta_backend_get_type,
        ECalMetaBackend,
        ECalMetaBackendClass
    );
    assert_layout!(e_cal_cache_get_type, ECalCache, ECalCacheClass);
}

/// The factories, which have subclasses as the backends do.
/// `jmap-backend-book`, `jmap-backend-cal` and `jmap-backend-collection` each
/// declare a `#[repr(C)]` class struct that starts with one of these. Each
/// writes into the parent half: `factory_name`, `backend_type`, the
/// `component_kind` of the calendar and the `prepare_mail` of the collection.
/// Bindgen computes the offsets of these writes from the header. The compiled
/// library sets the offsets that EDS reads. The size check makes sure that
/// they agree. Without it, the symptom is a `g_object_new(0)` for each address
/// book, with no cause shown.
#[test]
fn backend_factory_layouts_match_the_gtype_system() {
    // `EExtension`, the parent of the factories, has no accessor in the
    // allowlist because the backends do not name it. The structs below cover
    // it as their first bytes, which is where the backends use it.
    assert_layout!(
        e_backend_factory_get_type,
        EBackendFactory,
        EBackendFactoryClass
    );
    assert_layout!(
        e_book_backend_factory_get_type,
        EBookBackendFactory,
        EBookBackendFactoryClass
    );
    assert_layout!(
        e_cal_backend_factory_get_type,
        ECalBackendFactory,
        ECalBackendFactoryClass
    );
    assert_layout!(
        e_collection_backend_factory_get_type,
        ECollectionBackendFactory,
        ECollectionBackendFactoryClass
    );
}

/// The three slots that the factory subclasses fill, by name, as
/// `meta_backend_classes_expose_the_vfuncs_the_backends_override` does for the
/// backends. Only `prepare_mail` is a vfunc, and the collection factory
/// overrides it.
#[test]
fn the_collection_factory_class_exposes_the_fields_the_factory_fills() {
    let factory = unsafe { std::mem::zeroed::<ECollectionBackendFactoryClass>() };
    assert!(factory.prepare_mail.is_none());
    assert_eq!(factory.backend_type, 0);
    assert!(factory.factory_name.is_null());
}

/// The contact types. `jmap-backend-book` casts an `EContact *` to `EVCard *`
/// to render it. This C upcast is correct only while `EVCard` is the first
/// member of `EContact`, so this layout test checks it.
#[test]
fn contact_layouts_match_the_gtype_system_and_a_contact_leads_with_its_vcard() {
    assert_layout!(e_vcard_get_type, EVCard, EVCardClass);
    assert_layout!(e_contact_get_type, EContact, EContactClass);

    let contact = unsafe { std::mem::zeroed::<EContact>() };
    let _: EVCard = contact.parent;
    let class = unsafe { std::mem::zeroed::<EContactClass>() };
    let _: EVCardClass = class.parent_class;
}

/// The two component types that the calendar vfuncs pass across the boundary.
/// They come from libical-glib and libecal, not from the backend libraries, so
/// their layouts depend on the ABI of different libraries.
#[test]
fn component_layouts_match_the_gtype_system() {
    assert_layout!(i_cal_component_get_type, ICalComponent, ICalComponentClass);
    assert_layout!(e_cal_component_get_type, ECalComponent, ECalComponentClass);
}

/// The classed types that the mail provider subclasses or receives. They come
/// from `libcamel-1.2`, a third library with a third ABI. `CamelProvider` is
/// not in this list: it is a boxed type, and GObject cannot report its size.
/// tests/camel.rs checks it with a round trip through the provider registry.
#[test]
fn camel_layouts_match_the_gtype_system() {
    assert_layout!(camel_service_get_type, CamelService, CamelServiceClass);
    assert_layout!(camel_store_get_type, CamelStore, CamelStoreClass);
    assert_layout!(
        camel_offline_store_get_type,
        CamelOfflineStore,
        CamelOfflineStoreClass
    );
    assert_layout!(
        camel_transport_get_type,
        CamelTransport,
        CamelTransportClass
    );
    // The mechanism chain that holds the named authtype of the provider. The
    // class of `CamelSasl` holds the `CamelServiceAuthType *` slot that
    // `camel_sasl_authtype` reads, before twenty reserved pointers. If this
    // class struct is one field short, the provider authtype goes into the
    // `try_empty_password_sync` slot of Camel.
    assert_layout!(camel_sasl_get_type, CamelSasl, CamelSaslClass);
    assert_layout!(
        camel_sasl_xoauth2_get_type,
        CamelSaslXOAuth2,
        CamelSaslXOAuth2Class
    );
    assert_layout!(camel_session_get_type, CamelSession, CamelSessionClass);
    assert_layout!(camel_settings_get_type, CamelSettings, CamelSettingsClass);
    assert_layout!(
        camel_store_settings_get_type,
        CamelStoreSettings,
        CamelStoreSettingsClass
    );
    assert_layout!(
        camel_offline_settings_get_type,
        CamelOfflineSettings,
        CamelOfflineSettingsClass
    );
    assert_layout!(camel_folder_get_type, CamelFolder, CamelFolderClass);
    assert_layout!(
        camel_offline_folder_get_type,
        CamelOfflineFolder,
        CamelOfflineFolderClass
    );
    assert_layout!(
        camel_folder_summary_get_type,
        CamelFolderSummary,
        CamelFolderSummaryClass
    );
    assert_layout!(
        camel_message_info_get_type,
        CamelMessageInfo,
        CamelMessageInfoClass
    );
    assert_layout!(
        camel_message_info_base_get_type,
        CamelMessageInfoBase,
        CamelMessageInfoBaseClass
    );
    assert_layout!(camel_address_get_type, CamelAddress, CamelAddressClass);
    assert_layout!(
        camel_internet_address_get_type,
        CamelInternetAddress,
        CamelInternetAddressClass
    );
    assert_layout!(
        camel_data_wrapper_get_type,
        CamelDataWrapper,
        CamelDataWrapperClass
    );
    assert_layout!(camel_medium_get_type, CamelMedium, CamelMediumClass);
    assert_layout!(camel_mime_part_get_type, CamelMimePart, CamelMimePartClass);
    assert_layout!(
        camel_multipart_get_type,
        CamelMultipart,
        CamelMultipartClass
    );
    assert_layout!(
        camel_mime_message_get_type,
        CamelMimeMessage,
        CamelMimeMessageClass
    );
    assert_layout!(
        camel_data_cache_get_type,
        CamelDataCache,
        CamelDataCacheClass
    );
    assert_layout!(camel_stream_get_type, CamelStream, CamelStreamClass);
    assert_layout!(
        camel_stream_mem_get_type,
        CamelStreamMem,
        CamelStreamMemClass
    );
    assert_layout!(
        camel_stream_null_get_type,
        CamelStreamNull,
        CamelStreamNullClass
    );
    assert_layout!(camel_stream_fs_get_type, CamelStreamFs, CamelStreamFsClass);
    assert_layout!(
        camel_stream_filter_get_type,
        CamelStreamFilter,
        CamelStreamFilterClass
    );
    assert_layout!(
        camel_mime_filter_get_type,
        CamelMimeFilter,
        CamelMimeFilterClass
    );
    assert_layout!(
        camel_mime_filter_basic_get_type,
        CamelMimeFilterBasic,
        CamelMimeFilterBasicClass
    );
    assert_layout!(
        camel_mime_filter_crlf_get_type,
        CamelMimeFilterCRLF,
        CamelMimeFilterCRLFClass
    );
    assert_layout!(
        camel_mime_filter_linewrap_get_type,
        CamelMimeFilterLinewrap,
        CamelMimeFilterLinewrapClass
    );
    assert_layout!(
        camel_mime_parser_get_type,
        CamelMimeParser,
        CamelMimeParserClass
    );
    assert_layout!(
        camel_mime_filter_preview_get_type,
        CamelMimeFilterPreview,
        CamelMimeFilterPreviewClass
    );
    assert_layout!(
        camel_mime_filter_canon_get_type,
        CamelMimeFilterCanon,
        CamelMimeFilterCanonClass
    );
    assert_layout!(
        camel_mime_filter_tohtml_get_type,
        CamelMimeFilterToHTML,
        CamelMimeFilterToHTMLClass
    );
    assert_layout!(
        camel_html_parser_get_type,
        CamelHTMLParser,
        CamelHTMLParserClass
    );
    assert_layout!(
        camel_mime_filter_html_get_type,
        CamelMimeFilterHTML,
        CamelMimeFilterHTMLClass
    );
    assert_layout!(
        camel_mime_filter_enriched_get_type,
        CamelMimeFilterEnriched,
        CamelMimeFilterEnrichedClass
    );
    assert_layout!(
        camel_mime_filter_gzip_get_type,
        CamelMimeFilterGZip,
        CamelMimeFilterGZipClass
    );
    assert_layout!(
        camel_mime_filter_windows_get_type,
        CamelMimeFilterWindows,
        CamelMimeFilterWindowsClass
    );
    assert_layout!(
        camel_mime_filter_bestenc_get_type,
        CamelMimeFilterBestenc,
        CamelMimeFilterBestencClass
    );
    assert_layout!(
        camel_mime_filter_charset_get_type,
        CamelMimeFilterCharset,
        CamelMimeFilterCharsetClass
    );
    assert_layout!(
        camel_mime_filter_from_get_type,
        CamelMimeFilterFrom,
        CamelMimeFilterFromClass
    );
    assert_layout!(
        camel_mime_filter_yenc_get_type,
        CamelMimeFilterYenc,
        CamelMimeFilterYencClass
    );
    assert_layout!(
        camel_mime_filter_progress_get_type,
        CamelMimeFilterProgress,
        CamelMimeFilterProgressClass
    );
    assert_layout!(
        camel_stream_buffer_get_type,
        CamelStreamBuffer,
        CamelStreamBufferClass
    );
    assert_layout!(camel_sexp_get_type, CamelSExp, CamelSExpClass);
    // EDS 3.60 removed `camel-folder-search.h`, so that leg has no type to
    // check. The gate stays because 3.52, the shipped target, has the type.
    // `docs/eds-version-matrix.md` records the port of the code that uses it.
    #[cfg(camel_folder_search_object)]
    assert_layout!(
        camel_folder_search_get_type,
        CamelFolderSearch,
        CamelFolderSearchClass
    );
    assert_layout!(
        camel_operation_get_type,
        CamelOperation,
        CamelOperationClass
    );
    assert_layout!(
        camel_nntp_address_get_type,
        CamelNNTPAddress,
        CamelNNTPAddressClass
    );
    assert_layout!(
        camel_local_settings_get_type,
        CamelLocalSettings,
        CamelLocalSettingsClass
    );
    assert_layout!(camel_certdb_get_type, CamelCertDB, CamelCertDBClass);
    assert_layout!(
        camel_text_index_get_type,
        CamelTextIndex,
        CamelTextIndexClass
    );
    assert_layout!(
        camel_text_index_name_get_type,
        CamelTextIndexName,
        CamelTextIndexNameClass
    );
}

/// Bindgen can regenerate `GObject`, `GError` and similar types from the EDS
/// headers. The layouts agree, so the tests above pass. But the types are then
/// different from the gtk-rs types, and each pointer between eds-sys and the
/// `glib` crate needs a cast that hides the mismatch. These assignments
/// compile only while the re-exports are the gtk-rs types.
#[test]
fn glib_types_are_the_gtk_rs_ones_not_regenerated_copies() {
    let obj: *mut GObject = std::ptr::null_mut();
    let _: *mut gobject_sys::GObject = obj;

    let err: *mut GError = std::ptr::null_mut();
    let _: *mut glib_sys::GError = err;

    let cancellable: *mut GCancellable = std::ptr::null_mut();
    let _: *mut gio_sys::GCancellable = cancellable;

    // The parent slot of an EDS instance is that GObject.
    let backend = unsafe { std::mem::zeroed::<EBackend>() };
    let _: gobject_sys::GObject = backend.parent;
}

/// The meta backends supply a cache and offline and online behaviour. This test
/// makes sure that the generated class structs have the vfuncs that the book
/// and calendar backends override.
#[test]
fn meta_backend_classes_expose_the_vfuncs_the_backends_override() {
    let book = unsafe { std::mem::zeroed::<EBookMetaBackendClass>() };
    assert!(book.connect_sync.is_none());
    assert!(book.disconnect_sync.is_none());
    assert!(book.get_changes_sync.is_none());
    assert!(book.list_existing_sync.is_none());
    assert!(book.load_contact_sync.is_none());
    assert!(book.save_contact_sync.is_none());
    assert!(book.remove_contact_sync.is_none());

    let cal = unsafe { std::mem::zeroed::<ECalMetaBackendClass>() };
    assert!(cal.connect_sync.is_none());
    assert!(cal.disconnect_sync.is_none());
    assert!(cal.get_changes_sync.is_none());
    assert!(cal.list_existing_sync.is_none());
    assert!(cal.load_component_sync.is_none());
    assert!(cal.save_component_sync.is_none());
    assert!(cal.remove_component_sync.is_none());
}
