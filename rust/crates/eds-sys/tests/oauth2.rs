// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `EOAuth2Service` is the contract a JMAP account will hand EDS so that a
// token, rather than a password, comes back out of libsecret. It is an
// *interface*, and `g_type_query()` reports nothing about one — see
// `tests/layout.rs`, which is why `CamelSubscribable` and `ETimezoneCache` are
// pinned elsewhere too. So there is no `size_of` to compare, and the usual
// load-bearing check of this FFI layer does not apply.
//
// What is checkable, and is what actually matters, is the *offsets*: EDS's own
// `e_oauth2_service_*()` wrappers read a function pointer out of the vtable at
// an offset the compiled library decided, and this crate writes one in at an
// offset bindgen computed from the header. If those disagree, the symptom is a
// call through whatever pointer happens to sit at the wrong slot — a jump into
// a `const gchar *`, at authentication time, in a user's session.
//
// So the check here is behavioural: register a throwaway GObject that
// implements the interface, fill every slot this crate can name, and then call
// EDS's C wrappers and see which of our functions ran and what came back. A
// dispatch that arrives at the function we put in that slot is proof the two
// sides agree about where the slot is, which a size comparison would only have
// been evidence for.

use eds_sys::*;
use std::ffi::{CStr, c_char, c_uint, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// The registry and the base class, which *are* classed and so can be sized.

/// `EOAuth2Services` is the object EDS keeps the known services in, and
/// `EOAuth2ServiceBase` the `EExtension` a service module subclasses. Both are
/// ordinary `GObject`s, so unlike the interface they answer `g_type_query()` —
/// and both are types this crate will cross the ABI with, `EOAuth2ServiceBase`
/// by being subclassed outright.
///
/// `EExtension` itself is checked here rather than in `tests/layout.rs` for the
/// same reason: until now nothing named it, and the factories cover it only as
/// the leading bytes of their own structs. `EOAuth2ServiceBase` *is* an
/// `EExtension` and nothing else — `struct _EOAuth2ServiceBase { EExtension
/// parent; }` — so a subclass of it writes into the parent half immediately.
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
        // SAFETY: a class ref is taken so that `g_type_query` has a class to
        // report on, and dropped again; the query struct is ours.
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

/// Whether this EDS was built with OAuth2 at all. `e_oauth2_services_is_supported()`
/// is EDS's own answer, and it is a compile-time decision of the distribution's
/// package, not something a deployment can turn on later. A FALSE here means
/// the whole token path is unavailable on this machine however correct the code
/// is, which is worth failing loudly for rather than discovering as an
/// authentication that silently falls back to asking for a password.
#[test]
fn this_eds_was_built_with_oauth2_support() {
    // SAFETY: no arguments, no state.
    assert_ne!(
        unsafe { e_oauth2_services_is_supported() },
        GFALSE,
        "this EDS reports no OAuth2 support; a JMAP account cannot use a token here"
    );
}

/// The three keys EDS files a token under in libsecret. They are `#define`d
/// strings rather than exported symbols, so retyping one is not a link error —
/// it is a refresh token written where nothing ever reads it, and an account
/// that asks for consent again on every start. Taken from the header for the
/// same reason `E_SOURCE_CREDENTIAL_*` is.
#[test]
fn the_secret_keys_are_the_names_eds_stores_a_token_under() {
    assert_eq!(E_OAUTH2_SECRET_REFRESH_TOKEN, c"refresh_token");
    assert_eq!(E_OAUTH2_SECRET_ACCESS_TOKEN, c"access_token");
    assert_eq!(E_OAUTH2_SECRET_EXPIRES_AFTER, c"expires_after");
}

// ---------------------------------------------------------------------------
// The interface itself.

/// An interface, which is what puts it out of `tests/layout.rs`'s reach, and
/// one any `GObject` may implement: EDS's own services subclass
/// `EOAuth2ServiceBase` so that the module machinery finds them, but that is a
/// discovery convention rather than a prerequisite of the interface. Both
/// halves matter to the implementation to come — the first says a size check is
/// not available, the second says the type this crate registers is free to have
/// whatever parent the module loader needs.
#[test]
fn the_service_contract_is_an_interface_with_only_a_gobject_behind_it() {
    // SAFETY: plain type-system reads; the prerequisite array is g_malloc'd
    // for the caller to free.
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
/// This is the list the implementation is written against: a NULL slot is one
/// whose wrapper `g_return_val_if_fail`s and answers nothing, so leaving it
/// empty is not "behave conservatively" but "this service cannot be used". The
/// defaulted ones are the opposite — they already do the RFC 6749 thing, and
/// overriding one is a decision to differ from it, not a requirement.
#[test]
fn the_slots_eds_leaves_empty_are_the_ones_a_service_must_fill() {
    // SAFETY: the default vtable is ref'd for the length of the reads and
    // unref'd again.
    unsafe {
        let vtable = gobject_sys::g_type_default_interface_ref(e_oauth2_service_get_type())
            .cast::<EOAuth2ServiceInterface>();
        assert!(!vtable.is_null(), "the interface has no default vtable");

        // No default: a JMAP service has to say who it is and where it sends
        // the user, because nothing generic can.
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
// The offsets, proved by dispatching through them.

/// One bit per vtable slot, in the order the C struct declares them, set by the
/// probe implementation below when EDS dispatches into it. A bit that stays
/// clear after its wrapper was called means EDS read a different slot than the
/// one this crate wrote.
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
    // SAFETY: the wrapper hands us a live hash table of owned strings, which
    // is exactly what EDS's own helper writes into.
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
    // SAFETY: the wrapper's out-parameter, which the caller frees.
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
    // SAFETY: the wrapper's out-parameter, which the caller frees.
    unsafe { *out_error_message = glib_sys::g_strdup(PROBE_ERROR.as_ptr()) };
    GTRUE
}

/// Fills the vtable GLib hands us for the probe type. Every slot the header
/// declares is written, including the two `SoupMessage` ones that the test
/// below cannot call — see there for why they are still filled.
unsafe extern "C" fn probe_interface_init(iface: *mut c_void, _data: *mut c_void) {
    // SAFETY: GLib passes the interface struct for the type we registered this
    // initialiser against, which is `EOAuth2Service`'s.
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

/// A `GObject` that implements the interface and nothing else, registered once
/// for the process.
fn probe_service() -> *mut EOAuth2Service {
    // SAFETY: a fresh type name, sizes taken from the structs GLib will
    // allocate, and a `GInterfaceInfo` GLib copies before it returns.
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

/// A `GHashTable` of owned strings, which is what every `prepare_*_form`
/// wrapper is given.
fn form() -> *mut GHashTable {
    // SAFETY: the standard string-keyed table with g_free destructors.
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
    // SAFETY: a live table and a NUL-terminated key; the lookup borrows.
    !unsafe { glib_sys::g_hash_table_lookup(table, key.as_ptr().cast()) }.is_null()
}

/// A source to hand the wrappers that insist on one. Not backed by the
/// registry — `e_source_new_with_uid` with a NULL D-Bus object is what EDS
/// itself does for a source read from a keyfile.
fn a_source() -> *mut ESource {
    let mut error = ptr::null_mut();
    // SAFETY: a NUL-terminated uid, no D-Bus object, and a GError
    // out-parameter, which is the documented call.
    let source = unsafe {
        e_source_new_with_uid(c"jmap-oauth2-probe".as_ptr(), ptr::null_mut(), &mut error)
    };
    assert!(!source.is_null(), "e_source_new_with_uid failed");
    source
}

/// The load-bearing test of this file: EDS dispatches into the function this
/// crate wrote into each slot, and hands back what that function returned.
///
/// Sixteen of the eighteen vfuncs are called here. The two that are not are
/// `prepare_get_token_message` and `prepare_refresh_token_message`, whose
/// wrappers demand a real `SoupMessage` — and `soup_message_new` is
/// deliberately not on this crate's allowlist, because nothing in the plugin
/// constructs one. Their offsets are still pinned, transitively and just as
/// firmly: `prepare_refresh_token_form` sits immediately after the first and
/// `extract_error_message` immediately after the second, so a slot of the wrong
/// size at either would move a slot that *is* dispatched here. Dispatching
/// through the last declared vfunc is what makes that argument reach the whole
/// struct rather than a prefix of it.
#[test]
fn every_vtable_slot_dispatches_to_the_function_this_crate_wrote_into_it() {
    let service = probe_service();
    let source = a_source();
    REACHED.store(0, Ordering::SeqCst);

    // SAFETY: a live implementer, a live source, and out-parameters and hash
    // tables owned here; every g_strdup'd result is freed below.
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
                c"https://probe.invalid/done?code=x".as_ptr(),
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
