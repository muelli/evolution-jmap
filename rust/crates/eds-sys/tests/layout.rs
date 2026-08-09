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
        camel_mime_message_get_type,
        CamelMimeMessage,
        CamelMimeMessageClass
    );
    assert_layout!(
        camel_data_cache_get_type,
        CamelDataCache,
        CamelDataCacheClass
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
