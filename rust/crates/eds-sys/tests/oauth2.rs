// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `EOAuth2Service` is the contract that lets a JMAP account get a token, not a
// password, from libsecret through EDS. It is an interface, and
// `g_type_query()` reports nothing about an interface. Thus `tests/layout.rs`
// cannot check it, and `CamelSubscribable` and `ETimezoneCache` are also
// checked in other files. There is no `size_of` to compare.
//
// The offsets are what is important. The `e_oauth2_service_*()` wrappers of
// EDS read a function pointer from the vtable at an offset that the compiled
// library set. This crate writes one at an offset that bindgen computed from
// the header. If they disagree, the call goes through the wrong slot, for
// example into a `const gchar *`, at authentication time, in a user session.
//
// Thus this test is behavioural. It registers a temporary GObject that
// implements the interface and fills each slot that this crate can name. Then
// it calls the C wrappers of EDS and records which probe function ran and its
// result. A dispatch to the correct function proves that the two sides agree
// on the slot. A size comparison gives only evidence.

use eds_sys::*;
use std::ffi::{CStr, c_char, c_uint, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// The registry and the base class, which are classed and thus have a size.

/// `EOAuth2Services` is the object that keeps the known services of EDS.
/// `EOAuth2ServiceBase` is the `EExtension` that a service module subclasses.
/// Both are `GObject`s, so they answer `g_type_query()`. This crate uses both
/// across the ABI, and subclasses `EOAuth2ServiceBase`.
///
/// This test also checks `EExtension`, because `tests/layout.rs` covers it only
/// as the first bytes of the factory structs. `EOAuth2ServiceBase` contains
/// only an `EExtension` (`struct _EOAuth2ServiceBase { EExtension parent; }`),
/// so a subclass writes into the parent half immediately.
#[test]
fn the_oauth2_registry_and_base_extension_match_the_gtype_system() {
    for (name, gtype, instance, class) in [
        (
            "EExtension",
            unsafe { e_extension_get_type() },
            size_of::<EExtension>(),
            size_of::<EExtensionClass>(),
        ),
        (
            "EOAuth2ServiceBase",
            unsafe { e_oauth2_service_base_get_type() },
            size_of::<EOAuth2ServiceBase>(),
            size_of::<EOAuth2ServiceBaseClass>(),
        ),
        (
            "EOAuth2Services",
            unsafe { e_oauth2_services_get_type() },
            size_of::<EOAuth2Services>(),
            size_of::<EOAuth2ServicesClass>(),
        ),
    ] {
        // SAFETY: the class ref gives `g_type_query` a class to report on, and
        // is then released. This test owns the query struct.
        let q = unsafe {
            let klass = g_type_class_ref(gtype);
            assert!(!klass.is_null(), "{name}: g_type_class_ref returned NULL");
            let mut q = std::mem::zeroed::<GTypeQuery>();
            g_type_query(gtype, &mut q);
            g_type_class_unref(klass);
            q
        };
        assert_ne!(q.type_, 0, "{name}: g_type_query left the query zeroed");
        assert_eq!(
            q.instance_size as usize, instance,
            "{name}: instance size disagrees with g_type_query"
        );
        assert_eq!(
            q.class_size as usize, class,
            "{name}: class size disagrees with g_type_query"
        );
    }
}

/// Whether this EDS has OAuth2 support, from `e_oauth2_services_is_supported()`.
/// The distribution package sets this at compile time; a deployment cannot
/// change it. FALSE means that no token path is available on this machine.
/// The test fails loudly, so authentication does not silently ask for a
/// password.
#[test]
fn this_eds_was_built_with_oauth2_support() {
    // SAFETY: the call has no arguments and no state.
    assert_ne!(
        unsafe { e_oauth2_services_is_supported() },
        GFALSE,
        "this EDS reports no OAuth2 support; a JMAP account cannot use a token here"
    );
}

/// The three keys of a token in libsecret. They are `#define`d strings, not
/// exported symbols, so a typing error is not a link error. The result is a
/// refresh token that nothing reads, and an account that asks for consent at
/// each start. Thus they come from the header, as `E_SOURCE_CREDENTIAL_*` do.
#[test]
fn the_secret_keys_are_the_names_eds_stores_a_token_under() {
    assert_eq!(E_OAUTH2_SECRET_REFRESH_TOKEN, c"refresh_token");
    assert_eq!(E_OAUTH2_SECRET_ACCESS_TOKEN, c"access_token");
    assert_eq!(E_OAUTH2_SECRET_EXPIRES_AFTER, c"expires_after");
}

// ---------------------------------------------------------------------------
// Feature detection, not vtable dispatch: whether this EDS has the upstream
// OAuth2Dynamic parts (e-d-s `bf6e074`).

/// `eds-sys/build.rs` probes for the `e_source_oauth2_client_get_type` marker
/// to set the `eds_oauth2_dynamic` cfg. No header that this crate used declares
/// it (`Since: 3.64`, only on upstream master as of 2026-10-07). Thus this test,
/// gated on the absent cfg, compiles and runs on this EDS. Before `build.rs`
/// declared the cfg, its name here was an unexpected-cfg error under
/// `-D warnings`; this test was written against that failure. An EDS that
/// declares the marker does not compile this test. That is the signal to
/// write its `#[cfg(eds_oauth2_dynamic)]` sibling, as `tests/contacts.rs` has
/// one test for each side of `eds_death_date_field`.
#[test]
#[cfg(not(eds_oauth2_dynamic))]
fn eds_oauth2_dynamic_is_not_yet_available_on_this_eds() {}

/// The sibling of the test above: on an EDS that declares the marker, compare
/// the two strings that this project writes by hand with the upstream strings.
///
/// One is a `#define` of a string literal (`E_SOURCE_EXTENSION_OAUTH2_CLIENT`),
/// the other is the return value of a vfunc (`eos_dynamic_get_name`). Bindgen
/// gives neither, so `jmap_config::oauth2::EXTENSION_NAME` and
/// `jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD` are literals copied from
/// the upstream source. If a copy is wrong, an account goes into a group that
/// the upstream service does not read, or under a method that
/// `eos_default_can_process` does not match. Both cause a password prompt, not
/// an error.
#[test]
#[cfg(eds_oauth2_dynamic)]
fn upstreams_dynamic_oauth2_pieces_answer_to_the_names_this_project_spells() {
    // SAFETY: the calls have no arguments, and each type registers itself on
    // the first call. `g_type_class_ref` returns the initialised class. The
    // test keeps the reference, because it reads the strings below from it.
    unsafe {
        let extension_class = gobject_sys::g_type_class_ref(e_source_oauth2_client_get_type())
            .cast::<ESourceExtensionClass>();
        assert_eq!(
            CStr::from_ptr((*extension_class).name),
            c"OAuth2 Client",
            "ESourceOAuth2Client's keyfile group is not the one this project writes"
        );

        // The name of the service comes from a vfunc, so it needs an instance.
        // `EOAuth2ServiceDynamic` has no construct properties of its own.
        let service = gobject_sys::g_object_new(e_oauth2_service_dynamic_get_type(), ptr::null())
            .cast::<EOAuth2Service>();
        assert!(!service.is_null(), "g_object_new returned NULL");
        assert_eq!(
            CStr::from_ptr(e_oauth2_service_get_name(service)),
            c"OAuth2Dynamic",
            "EOAuth2ServiceDynamic's name is not the method this project writes"
        );
        gobject_sys::g_object_unref(service.cast());
    }
}

// ---------------------------------------------------------------------------
// The interface itself.

/// An interface, so `tests/layout.rs` cannot check it, and any `GObject` can
/// implement it. The services of EDS subclass `EOAuth2ServiceBase` so that the
/// module loader finds them, but the interface does not require it. Thus no
/// size check is available, and the type that this crate registers can have
/// the parent that the module loader needs.
#[test]
fn the_service_contract_is_an_interface_with_only_a_gobject_behind_it() {
    // SAFETY: these are type-system reads. The prerequisite array is
    // g_malloc'd, and the caller frees it.
    unsafe {
        let service = e_oauth2_service_get_type();
        assert_eq!(g_type_fundamental(service), gobject_sys::G_TYPE_INTERFACE);

        let mut q = std::mem::zeroed::<GTypeQuery>();
        g_type_query(service, &mut q);
        assert_eq!(q.type_, 0, "g_type_query knows an interface after all");

        let mut n = 0;
        let prerequisites = gobject_sys::g_type_interface_prerequisites(service, &mut n);
        let required = std::slice::from_raw_parts(prerequisites, n as usize).to_vec();
        glib_sys::g_free(prerequisites.cast());
        assert_eq!(
            required,
            vec![gobject_sys::G_TYPE_OBJECT],
            "EOAuth2Service requires more of an implementer than a GObject"
        );
    }
}

/// Which slots EDS puts a default behind, and which it leaves NULL.
///
/// The implementation is written against this list. For a NULL slot, the
/// wrapper `g_return_val_if_fail`s and answers nothing, so an empty slot makes
/// the service unusable. A slot with a default already follows RFC 6749, and
/// an override of it is optional.
#[test]
fn the_slots_eds_leaves_empty_are_the_ones_a_service_must_fill() {
    // SAFETY: the test holds a ref on the default vtable during the reads, and
    // then releases it.
    unsafe {
        let vtable = gobject_sys::g_type_default_interface_ref(e_oauth2_service_get_type())
            .cast::<EOAuth2ServiceInterface>();
        assert!(!vtable.is_null(), "the interface has no default vtable");

        // No default: a JMAP service must give its identity and the user's
        // destination, because generic code cannot.
        assert!((*vtable).get_name.is_none(), "get_name has a default");
        assert!(
            (*vtable).get_display_name.is_none(),
            "get_display_name has a default"
        );
        assert!(
            (*vtable).get_client_id.is_none(),
            "get_client_id has a default"
        );
        assert!(
            (*vtable).get_authentication_uri.is_none(),
            "get_authentication_uri has a default"
        );

        gobject_sys::g_type_default_interface_unref(vtable.cast());
    }
}

// ---------------------------------------------------------------------------
// The offsets, proved by dispatch through them.

/// One bit for each vtable slot, in the order of the C struct. The probe
/// implementation below sets a bit when EDS dispatches into it. If a bit stays
/// clear after its wrapper call, EDS read a different slot than this crate wrote.
mod slot {
    pub const CAN_PROCESS: u32 = 1 << 0;
    pub const GUESS_CAN_PROCESS: u32 = 1 << 1;
    pub const GET_FLAGS: u32 = 1 << 2;
    pub const GET_NAME: u32 = 1 << 3;
    pub const GET_DISPLAY_NAME: u32 = 1 << 4;
    pub const GET_CLIENT_ID: u32 = 1 << 5;
    pub const GET_CLIENT_SECRET: u32 = 1 << 6;
    pub const GET_AUTHENTICATION_URI: u32 = 1 << 7;
    pub const GET_REFRESH_URI: u32 = 1 << 8;
    pub const GET_REDIRECT_URI: u32 = 1 << 9;
    pub const PREPARE_AUTHENTICATION_URI_QUERY: u32 = 1 << 10;
    pub const GET_AUTHENTICATION_POLICY: u32 = 1 << 11;
    pub const EXTRACT_AUTHORIZATION_CODE: u32 = 1 << 12;
    pub const PREPARE_GET_TOKEN_FORM: u32 = 1 << 13;
    pub const PREPARE_REFRESH_TOKEN_FORM: u32 = 1 << 15;
    pub const EXTRACT_ERROR_MESSAGE: u32 = 1 << 17;
}

static REACHED: AtomicU32 = AtomicU32::new(0);

fn reached(slot: u32) {
    REACHED.fetch_or(slot, Ordering::SeqCst);
}

const PROBE_NAME: &CStr = c"jmap-eds-sys-probe";
const PROBE_DISPLAY_NAME: &CStr = c"JMAP probe";
const PROBE_CLIENT_ID: &CStr = c"probe-client-id";
const PROBE_CLIENT_SECRET: &CStr = c"probe-client-secret";
const PROBE_AUTHENTICATION_URI: &CStr = c"https://probe.invalid/authorize";
const PROBE_REFRESH_URI: &CStr = c"https://probe.invalid/token";
const PROBE_REDIRECT_URI: &CStr = c"https://probe.invalid/done";
const PROBE_CODE: &CStr = c"probe-authorization-code";
const PROBE_ERROR: &CStr = c"probe-error-message";

unsafe extern "C" fn probe_can_process(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
) -> i32 {
    reached(slot::CAN_PROCESS);
    GTRUE
}

unsafe extern "C" fn probe_guess_can_process(
    _service: *mut EOAuth2Service,
    _protocol: *const c_char,
    _hostname: *const c_char,
) -> i32 {
    reached(slot::GUESS_CAN_PROCESS);
    GTRUE
}

unsafe extern "C" fn probe_get_flags(_service: *mut EOAuth2Service) -> u32 {
    reached(slot::GET_FLAGS);
    E_OAUTH2_SERVICE_FLAG_EXTRACT_REQUIRES_PAGE_CONTENT
}

unsafe extern "C" fn probe_get_name(_service: *mut EOAuth2Service) -> *const c_char {
    reached(slot::GET_NAME);
    PROBE_NAME.as_ptr()
}

unsafe extern "C" fn probe_get_display_name(_service: *mut EOAuth2Service) -> *const c_char {
    reached(slot::GET_DISPLAY_NAME);
    PROBE_DISPLAY_NAME.as_ptr()
}

unsafe extern "C" fn probe_get_client_id(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
) -> *const c_char {
    reached(slot::GET_CLIENT_ID);
    PROBE_CLIENT_ID.as_ptr()
}

unsafe extern "C" fn probe_get_client_secret(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
) -> *const c_char {
    reached(slot::GET_CLIENT_SECRET);
    PROBE_CLIENT_SECRET.as_ptr()
}

unsafe extern "C" fn probe_get_authentication_uri(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
) -> *const c_char {
    reached(slot::GET_AUTHENTICATION_URI);
    PROBE_AUTHENTICATION_URI.as_ptr()
}

unsafe extern "C" fn probe_get_refresh_uri(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
) -> *const c_char {
    reached(slot::GET_REFRESH_URI);
    PROBE_REFRESH_URI.as_ptr()
}

unsafe extern "C" fn probe_get_redirect_uri(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
) -> *const c_char {
    reached(slot::GET_REDIRECT_URI);
    PROBE_REDIRECT_URI.as_ptr()
}

unsafe extern "C" fn probe_prepare_authentication_uri_query(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
    uri_query: *mut GHashTable,
) {
    reached(slot::PREPARE_AUTHENTICATION_URI_QUERY);
    // SAFETY: the wrapper gives a live hash table of owned strings, which is
    // what the EDS helper writes into.
    unsafe { e_oauth2_service_util_set_to_form(uri_query, c"probe_query".as_ptr(), c"1".as_ptr()) };
}

unsafe extern "C" fn probe_get_authentication_policy(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
    _uri: *const c_char,
) -> c_uint {
    reached(slot::GET_AUTHENTICATION_POLICY);
    E_OAUTH2_SERVICE_NAVIGATION_POLICY_ABORT
}

unsafe extern "C" fn probe_extract_authorization_code(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
    _page_title: *const c_char,
    _page_uri: *const c_char,
    _page_content: *const c_char,
    out_authorization_code: *mut *mut c_char,
) -> i32 {
    reached(slot::EXTRACT_AUTHORIZATION_CODE);
    // SAFETY: this is the out-parameter of the wrapper, which the caller frees.
    unsafe { *out_authorization_code = glib_sys::g_strdup(PROBE_CODE.as_ptr()) };
    GTRUE
}

unsafe extern "C" fn probe_prepare_get_token_form(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
    _authorization_code: *const c_char,
    form: *mut GHashTable,
) {
    reached(slot::PREPARE_GET_TOKEN_FORM);
    // SAFETY: as `probe_prepare_authentication_uri_query`.
    unsafe { e_oauth2_service_util_set_to_form(form, c"probe_get".as_ptr(), c"1".as_ptr()) };
}

unsafe extern "C" fn probe_prepare_refresh_token_form(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
    _refresh_token: *const c_char,
    form: *mut GHashTable,
) {
    reached(slot::PREPARE_REFRESH_TOKEN_FORM);
    // SAFETY: as `probe_prepare_authentication_uri_query`.
    unsafe { e_oauth2_service_util_set_to_form(form, c"probe_refresh".as_ptr(), c"1".as_ptr()) };
}

unsafe extern "C" fn probe_extract_error_message(
    _service: *mut EOAuth2Service,
    _source: *mut ESource,
    _page_title: *const c_char,
    _page_uri: *const c_char,
    _page_content: *const c_char,
    out_error_message: *mut *mut c_char,
) -> i32 {
    reached(slot::EXTRACT_ERROR_MESSAGE);
    // SAFETY: this is the out-parameter of the wrapper, which the caller frees.
    unsafe { *out_error_message = glib_sys::g_strdup(PROBE_ERROR.as_ptr()) };
    GTRUE
}

/// Fills the vtable that GLib gives for the probe type. It writes every slot
/// except the two `SoupMessage` slots, which the test below cannot call.
unsafe extern "C" fn probe_interface_init(iface: *mut c_void, _data: *mut c_void) {
    // SAFETY: GLib passes the interface struct of the type that this
    // initialiser is registered for, which is `EOAuth2Service`.
    let iface = unsafe { &mut *iface.cast::<EOAuth2ServiceInterface>() };
    iface.can_process = Some(probe_can_process);
    iface.guess_can_process = Some(probe_guess_can_process);
    iface.get_flags = Some(probe_get_flags);
    iface.get_name = Some(probe_get_name);
    iface.get_display_name = Some(probe_get_display_name);
    iface.get_client_id = Some(probe_get_client_id);
    iface.get_client_secret = Some(probe_get_client_secret);
    iface.get_authentication_uri = Some(probe_get_authentication_uri);
    iface.get_refresh_uri = Some(probe_get_refresh_uri);
    iface.get_redirect_uri = Some(probe_get_redirect_uri);
    iface.prepare_authentication_uri_query = Some(probe_prepare_authentication_uri_query);
    iface.get_authentication_policy = Some(probe_get_authentication_policy);
    iface.extract_authorization_code = Some(probe_extract_authorization_code);
    iface.prepare_get_token_form = Some(probe_prepare_get_token_form);
    iface.prepare_refresh_token_form = Some(probe_prepare_refresh_token_form);
    iface.extract_error_message = Some(probe_extract_error_message);
}

/// A `GObject` that implements only the interface.
fn probe_service() -> *mut EOAuth2Service {
    // SAFETY: the type name is new, the sizes come from the structs that GLib
    // allocates, and GLib copies the `GInterfaceInfo` before it returns.
    unsafe {
        let gtype = g_type_register_static_simple(
            gobject_sys::G_TYPE_OBJECT,
            c"JmapEdsSysProbeOAuth2Service".as_ptr(),
            size_of::<GObjectClass>() as u32,
            None,
            size_of::<GObject>() as u32,
            None,
            0,
        );
        assert_ne!(gtype, 0, "registering the probe type failed");
        let info = GInterfaceInfo {
            interface_init: Some(probe_interface_init),
            interface_finalize: None,
            interface_data: ptr::null_mut(),
        };
        g_type_add_interface_static(gtype, e_oauth2_service_get_type(), &info);

        let object =
            gobject_sys::g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null_mut());
        assert!(!object.is_null(), "instantiating the probe type failed");
        object.cast()
    }
}

/// A `GHashTable` of owned strings, as each `prepare_*_form` wrapper receives.
fn form() -> *mut GHashTable {
    // SAFETY: the standard table with string keys and g_free destructors.
    unsafe {
        glib_sys::g_hash_table_new_full(
            Some(glib_sys::g_str_hash),
            Some(glib_sys::g_str_equal),
            Some(glib_sys::g_free),
            Some(glib_sys::g_free),
        )
    }
}

fn form_has(table: *mut GHashTable, key: &CStr) -> bool {
    // SAFETY: the table is live and the key is NUL-terminated. The lookup
    // borrows.
    !unsafe { glib_sys::g_hash_table_lookup(table, key.as_ptr().cast()) }.is_null()
}

/// A source for the wrappers that require one. The registry does not back it:
/// EDS also calls `e_source_new_with_uid` with a NULL D-Bus object for a source
/// read from a keyfile.
fn a_source() -> *mut ESource {
    let mut error = ptr::null_mut();
    // SAFETY: the documented call, with a NUL-terminated uid, no D-Bus object
    // and a GError out-parameter.
    let source = unsafe {
        e_source_new_with_uid(c"jmap-oauth2-probe".as_ptr(), ptr::null_mut(), &mut error)
    };
    assert!(!source.is_null(), "e_source_new_with_uid failed");
    source
}

/// The most important test of this file: EDS dispatches into the function that
/// this crate wrote into each slot, and returns the result of that function.
///
/// The test calls sixteen of the eighteen vfuncs. The wrappers of
/// `prepare_get_token_message` and `prepare_refresh_token_message` require a
/// real `SoupMessage`. `soup_message_new` is not on the allowlist of this
/// crate, because the plugin does not construct one. Their offsets are
/// nevertheless checked: `prepare_refresh_token_form` follows the first and
/// `extract_error_message` follows the second. A slot of the wrong size at
/// either moves a dispatched slot. Dispatch through the last declared vfunc
/// extends this check to the whole struct.
#[test]
fn every_vtable_slot_dispatches_to_the_function_this_crate_wrote_into_it() {
    let service = probe_service();
    let source = a_source();
    REACHED.store(0, Ordering::SeqCst);

    // SAFETY: the implementer and source are live, and this test owns the
    // out-parameters and hash tables. Each g_strdup'd result is freed below.
    unsafe {
        assert_ne!(e_oauth2_service_can_process(service, source), GFALSE);
        assert_ne!(
            e_oauth2_service_guess_can_process(
                service,
                c"jmap".as_ptr(),
                c"probe.invalid".as_ptr()
            ),
            GFALSE
        );
        assert_eq!(
            e_oauth2_service_get_flags(service),
            E_OAUTH2_SERVICE_FLAG_EXTRACT_REQUIRES_PAGE_CONTENT
        );
        assert_eq!(
            CStr::from_ptr(e_oauth2_service_get_name(service)),
            PROBE_NAME
        );
        assert_eq!(
            CStr::from_ptr(e_oauth2_service_get_display_name(service)),
            PROBE_DISPLAY_NAME
        );
        assert_eq!(
            CStr::from_ptr(e_oauth2_service_get_client_id(service, source)),
            PROBE_CLIENT_ID
        );
        assert_eq!(
            CStr::from_ptr(e_oauth2_service_get_client_secret(service, source)),
            PROBE_CLIENT_SECRET
        );
        assert_eq!(
            CStr::from_ptr(e_oauth2_service_get_authentication_uri(service, source)),
            PROBE_AUTHENTICATION_URI
        );
        assert_eq!(
            CStr::from_ptr(e_oauth2_service_get_refresh_uri(service, source)),
            PROBE_REFRESH_URI
        );
        assert_eq!(
            CStr::from_ptr(e_oauth2_service_get_redirect_uri(service, source)),
            PROBE_REDIRECT_URI
        );
        assert_eq!(
            e_oauth2_service_get_authentication_policy(
                service,
                source,
                c"https://probe.invalid/done?code=x".as_ptr()
            ),
            E_OAUTH2_SERVICE_NAVIGATION_POLICY_ABORT
        );

        let query = form();
        e_oauth2_service_prepare_authentication_uri_query(service, source, query);
        assert!(
            form_has(query, c"probe_query"),
            "the query was not prepared"
        );
        glib_sys::g_hash_table_destroy(query);

        let get = form();
        e_oauth2_service_prepare_get_token_form(service, source, PROBE_CODE.as_ptr(), get);
        assert!(
            form_has(get, c"probe_get"),
            "the token form was not prepared"
        );
        glib_sys::g_hash_table_destroy(get);

        let refresh = form();
        e_oauth2_service_prepare_refresh_token_form(
            service,
            source,
            c"a-refresh".as_ptr(),
            refresh,
        );
        assert!(
            form_has(refresh, c"probe_refresh"),
            "the refresh form was not prepared"
        );
        glib_sys::g_hash_table_destroy(refresh);

        let mut code = ptr::null_mut();
        assert_ne!(
            e_oauth2_service_extract_authorization_code(
                service,
                source,
                c"a title".as_ptr(),
                c"https://probe.invalid/done".as_ptr(),
                ptr::null(),
                &mut code,
            ),
            GFALSE
        );
        assert_eq!(CStr::from_ptr(code), PROBE_CODE);
        glib_sys::g_free(code.cast());

        let mut message = ptr::null_mut();
        assert_ne!(
            e_oauth2_service_extract_error_message(
                service,
                source,
                c"a title".as_ptr(),
                c"https://probe.invalid/done?error=x".as_ptr(),
                ptr::null(),
                &mut message,
            ),
            GFALSE
        );
        assert_eq!(CStr::from_ptr(message), PROBE_ERROR);
        glib_sys::g_free(message.cast());

        g_object_unref(source.cast());
        g_object_unref(service.cast());
    }

    let expected = slot::CAN_PROCESS
        | slot::GUESS_CAN_PROCESS
        | slot::GET_FLAGS
        | slot::GET_NAME
        | slot::GET_DISPLAY_NAME
        | slot::GET_CLIENT_ID
        | slot::GET_CLIENT_SECRET
        | slot::GET_AUTHENTICATION_URI
        | slot::GET_REFRESH_URI
        | slot::GET_REDIRECT_URI
        | slot::PREPARE_AUTHENTICATION_URI_QUERY
        | slot::GET_AUTHENTICATION_POLICY
        | slot::EXTRACT_AUTHORIZATION_CODE
        | slot::PREPARE_GET_TOKEN_FORM
        | slot::PREPARE_REFRESH_TOKEN_FORM
        | slot::EXTRACT_ERROR_MESSAGE;
    assert_eq!(
        REACHED.load(Ordering::SeqCst),
        expected,
        "a wrapper answered without reaching the slot this crate wrote"
    );
}
