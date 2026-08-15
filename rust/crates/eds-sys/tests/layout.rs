// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The bindings are generated from whatever EDS headers are installed, so
// nothing guarantees on its own that the Rust `struct` we will subclass has
// the same shape as the C one the runtime allocates. Ask the type system
// itself: `g_type_query()` reports the instance and class sizes GObject uses
// for `g_type_register_static()`, and those must match our `size_of`. If they
// ever drift, every vfunc override in the backends silently writes into the
// wrong offset, so this is the load-bearing test of the whole FFI layer.

use eds_sys::*;
use std::mem::size_of;

/// `g_type_query()` only fills the struct for classed types whose class has
/// been referenced at least once, so take a class ref first. Abstract types
/// (all the meta backends) can be class-ref'd; only instantiating them fails.
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

/// Checks one instance/class struct pair against the registered type.
macro_rules! assert_layout {
    ($get_type:expr, $instance:ty, $class:ty) => {{
        let name = stringify!($instance);
        let q = query(unsafe { $get_type() });
        assert_eq!(
            q.instance_size as usize,
            size_of::<$instance>(),
            "{name}: instance size disagrees with g_type_query"
        );
        assert_eq!(
            q.class_size as usize,
            size_of::<$class>(),
            "{name}: class size disagrees with g_type_query"
        );
    }};
}

#[test]
fn backend_layouts_match_the_gtype_system() {
    assert_layout!(e_backend_get_type, EBackend, EBackendClass);
    assert_layout!(e_source_get_type, ESource, ESourceClass);
}

/// The type M6's collection backend subclasses. It lives in `libebackend`
/// beside `EBackend` rather than in either of the data libraries, and it is the
/// one class whose vfuncs are dispatched by `evolution-source-registry` itself
/// rather than by a factory subprocess — so a layout drift here misfires in the
/// process that owns every account, not in one address book's.
#[test]
fn collection_backend_layout_matches_the_gtype_system() {
    assert_layout!(
        e_collection_backend_get_type,
        ECollectionBackend,
        ECollectionBackendClass
    );
}

/// And the slots on it the collection backend overrides, for the same reason
/// the meta backends' are checked below: a name that moved or a signature that
/// changed is a compile error here rather than a wrong-arity call at runtime.
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

/// The factories, which are subclassed exactly like the backends and are the
/// gap in this file: `jmap-backend-book`, `jmap-backend-cal` and
/// `jmap-backend-collection` each declare a `#[repr(C)]` class struct leading
/// with one of these and then *write into the parent half* — `factory_name`,
/// `backend_type`, the calendar's `component_kind`, and the collection's
/// `prepare_mail`. Those writes land at offsets bindgen computed from the
/// header, and what EDS reads them back at is decided by the compiled library.
/// The size check is what says the two agree; without it the whole factory
/// mechanism is an unverified bet, and the symptom of losing it is a
/// `g_object_new(0)` per address book with no hint as to why.
///
/// Originally `docs/AUDIT-FFI.md`'s F5, on the branch that fix was written on;
/// the squash that brought F1–F4 to master left it behind, which is
/// `docs/AUDIT-FFI-20260810.md`'s F12. The collection factory is new since, and
/// is the same bet a third time.
#[test]
fn backend_factory_layouts_match_the_gtype_system() {
    // `EExtension`, the factories' own parent, has no accessor in the
    // allowlist — the backends never name it — so it is covered only as the
    // leading bytes of the structs below, which is where they use it.
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

/// And the three slots the factories are subclassed to fill, by name — the same
/// check `meta_backend_classes_expose_the_vfuncs_the_backends_override` makes
/// for the backends. `prepare_mail` is the one that is a vfunc rather than a
/// value, and the one M6 overrides.
#[test]
fn the_collection_factory_class_exposes_the_fields_the_factory_fills() {
    let factory = unsafe { std::mem::zeroed::<ECollectionBackendFactoryClass>() };
    assert!(factory.prepare_mail.is_none());
    assert_eq!(factory.backend_type, 0);
    assert!(factory.factory_name.is_null());
}

/// The contact types. `jmap-backend-book` casts an `EContact *` it was handed to
/// `EVCard *` to render it — the C upcast, which is only an upcast while
/// `EVCard` really is the first member of `EContact`. That is a claim about a
/// layout, so it belongs here rather than in a comment.
#[test]
fn contact_layouts_match_the_gtype_system_and_a_contact_leads_with_its_vcard() {
    assert_layout!(e_vcard_get_type, EVCard, EVCardClass);
    assert_layout!(e_contact_get_type, EContact, EContactClass);

    let contact = unsafe { std::mem::zeroed::<EContact>() };
    let _: EVCard = contact.parent;
    let class = unsafe { std::mem::zeroed::<EContactClass>() };
    let _: EVCardClass = class.parent_class;
}

/// The two component types the calendar vfuncs pass across the boundary. They
/// come from libical-glib and libecal rather than from the backend libraries
/// the rest of this file checks, so their layouts are a separate bet on a
/// separate library's ABI.
#[test]
fn component_layouts_match_the_gtype_system() {
    assert_layout!(i_cal_component_get_type, ICalComponent, ICalComponentClass);
    assert_layout!(e_cal_component_get_type, ECalComponent, ECalComponentClass);
}

/// The classed types M5's mail provider subclasses or is handed. These come
/// from `libcamel-1.2`, a third library with a third ABI, and unlike the
/// backend libraries Camel's own `CamelProvider` is *not* in this list: it is a
/// boxed type with no size GObject can report, so tests/camel.rs pins it by a
/// round trip through the provider registry instead.
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

/// bindgen will happily regenerate `GObject`, `GError` and friends from the
/// EDS headers. The layouts would still match, so the tests above would still
/// pass — but the types would be *distinct* from the gtk-rs ones, and every
/// pointer crossing between eds-sys and the `glib` crate would need a cast
/// that silences exactly the mismatch a cast should be catching. These
/// assignments only compile while the re-exports are the real thing.
#[test]
fn glib_types_are_the_gtk_rs_ones_not_regenerated_copies() {
    let obj: *mut GObject = std::ptr::null_mut();
    let _: *mut gobject_sys::GObject = obj;

    let err: *mut GError = std::ptr::null_mut();
    let _: *mut glib_sys::GError = err;

    let cancellable: *mut GCancellable = std::ptr::null_mut();
    let _: *mut gio_sys::GCancellable = cancellable;

    // ...and the parent slot of an EDS instance really is that GObject.
    let backend = unsafe { std::mem::zeroed::<EBackend>() };
    let _: gobject_sys::GObject = backend.parent;
}

/// The whole point of the meta backends is that they hand us a cache and an
/// offline/online story for free; make sure the vfuncs we will override in M3
/// and M4 actually exist in the generated class structs.
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
