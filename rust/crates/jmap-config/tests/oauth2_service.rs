// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests `oauth2_service::Service` through the `e_oauth2_service_*` wrappers of
// EDS. `eds-sys/tests/oauth2.rs` uses the same style to prove the raw ABI. Here
// it proves that the implementation of this crate gives correct answers:
//
// - The two vfuncs with no default that are per-account (`get_client_id`,
//   `get_authentication_uri`) read what `oauth2::apply` wrote.
// - The three vfuncs whose default is wrong for a per-account value also read
//   it.
// - `can_process` is deliberately not filled (see the module docs of
//   `oauth2_service`). It says yes only when `[Authentication] method` is the
//   name of this service.
//
// The tests build instances through a real `EOAuth2Services` registry, not a
// bare `g_object_new`. `EOAuth2ServiceBase` is not a bare `GObject`: its
// `constructed()` (`e-oauth2-service-base.c`, read, not assumed) reads the
// `extensible` construct-only property and calls `e_oauth2_services_add()` on
// it. Without that property, GObject still constructs an instance with the
// `NULL` default. But it logs two `E_IS_EXTENSIBLE`/`E_IS_OAUTH2_SERVICES`
// assertion failures as `CRITICAL`, and this project does not accept that as a
// pass. The real object costs one helper. It also lets the tests call
// `e_oauth2_services_find` directly, which is the entry point of real code.
//
// The compiler includes this file only where this project registers its own
// service. On an EDS with `EOAuth2ServiceDynamic`, `oauth2_service::register`
// does not register this service, and nothing constructs or dispatches it. Its
// vfuncs would answer NULL there in all cases: they read the `Extension` of
// this crate, and `ESourceOAuth2Client` then owns the group. On such a build,
// other tests replace this coverage:
// `the_entry_point_leaves_the_oauth2_service_to_eds` in `tests/module.rs`, and
// `oauth2_service.rs` of the three backend modules. They assert that the
// registry finds the upstream service for these accounts.
#![cfg(not(eds_oauth2_dynamic))]

use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, EOAuth2Service, EOAuth2ServiceInterface, EOAuth2Services,
    ESource, e_oauth2_service_can_process, e_oauth2_service_get_authentication_uri,
    e_oauth2_service_get_client_id, e_oauth2_service_get_client_secret,
    e_oauth2_service_get_display_name, e_oauth2_service_get_name,
    e_oauth2_service_get_redirect_uri, e_oauth2_service_get_refresh_uri, e_oauth2_service_get_type,
    e_oauth2_service_prepare_authentication_uri_query, e_oauth2_service_prepare_get_token_form,
    e_oauth2_services_find, e_oauth2_services_new, e_source_authentication_set_method,
    e_source_get_extension, e_source_new_with_uid,
};
use glib_sys::{
    GFALSE, GHashTable, g_free, g_hash_table_destroy, g_hash_table_lookup, g_hash_table_new_full,
    g_str_equal, g_str_hash,
};
use gobject_sys::{
    G_TYPE_OBJECT, GValue, g_object_new_with_properties, g_object_unref, g_type_class_ref,
    g_type_interface_peek, g_value_init, g_value_set_object, g_value_unset,
};
use jmap_backend_core::subclass::register_static;
use jmap_config::oauth2::{self, Config};
use jmap_config::oauth2_service::{NAME, Service};
use jmap_tracing_capture::capture;
use tracing::Level;

/// Runs `body` under the shared capturing subscriber. Returns the level of
/// each emitted event, in emission order.
fn capture_levels(body: impl FnOnce()) -> Vec<Level> {
    capture(body)
        .1
        .into_iter()
        .map(|(level, ..)| level)
        .collect()
}

struct TestSource(*mut ESource);

impl TestSource {
    /// A source with a unique uid.
    ///
    /// The uid must be different for each source, not only for each test.
    /// `oauth2_service` keeps the PKCE verifier that it makes in a
    /// **process-global map keyed by source uid** (`pkce_verifiers()`). This
    /// is correct: the two halves of one authorization flow run in the process
    /// of the credentials prompter, and the uid identifies the account between
    /// them.
    ///
    /// Previously, all sources here had the same literal uid. Thus two
    /// parallel tests shared one entry in that map. A test between its
    /// `prepare_authentication_uri_query` and its `prepare_get_token_form`
    /// could get its verifier overwritten by the other test. Or, after it
    /// redeemed its own verifier, it could find another one and get a report
    /// that a single-use secret was reused. This caused the intermittent
    /// `the_token_form_redeems_the_verifier_behind_the_authorization_challenge`
    /// failure from 2026-08-24. The cause was collisions between tests, not a
    /// defect in the keying.
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let uid = format!(
            "jmap-oauth2-service-test-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let uid = CString::new(uid).expect("no NUL in a generated uid");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn written(self, config: &Config) -> Self {
        // SAFETY: a live source.
        unsafe { oauth2::apply(self.0, config) };
        self
    }

    /// Sets `[Authentication] method`. The default `can_process` compares this
    /// value with the answer of `get_name()`.
    fn with_authentication_method(self, method: &CStr) -> Self {
        // SAFETY: a live source. `E_SOURCE_EXTENSION_AUTHENTICATION` is an
        // extension name of EDS, and EDS can always create it.
        unsafe {
            let auth =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_method(auth, method.as_ptr());
        }
        self
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn config() -> Config {
    Config {
        client_id: Some("client-abc123".to_owned()),
        client_secret: Some("s3cret".to_owned()),
        authorization_endpoint: Some("https://jmap.example.com/authorize".to_owned()),
        token_endpoint: Some("https://jmap.example.com/token".to_owned()),
        redirect_uri: Some("https://client.example.org/callback".to_owned()),
        scope: Some("urn:ietf:params:oauth:scope:mail offline_access".to_owned()),
        resource: Some("https://jmap.example.com/session".to_owned()),
    }
}

/// A live instance of [`Service`], registered into `registry` as
/// `e_oauth2_service_base_constructed` requires. The header comment of this
/// file tells why a bare `g_object_new` is not sufficient.
fn service_in(registry: *mut EOAuth2Services) -> *mut EOAuth2Service {
    let gtype = register_static::<Service>();
    let name = c"extensible";

    // SAFETY: a new zeroed GValue. It is initialised to hold an object and set
    // to a live `registry` before `g_object_new_with_properties` reads it. The
    // unset after that releases only the ref of this call.
    unsafe {
        let mut value: GValue = std::mem::zeroed();
        g_value_init(&mut value, G_TYPE_OBJECT);
        g_value_set_object(&mut value, registry.cast());
        let mut names = [name.as_ptr()];
        let service = g_object_new_with_properties(gtype, 1, names.as_mut_ptr(), &value)
            .cast::<EOAuth2Service>();
        g_value_unset(&mut value);
        assert!(!service.is_null(), "g_object_new_with_properties failed");
        service
    }
}

/// `EOAuth2Services` is a process-wide singleton: `oauth2_services_constructor`
/// in `e-oauth2-services.c` returns the same object while a ref exists. This
/// has no effect here, because the services of all tests give the same
/// answers. But the registrations of one test are visible to other tests.
fn registry() -> *mut EOAuth2Services {
    // SAFETY: the function has no arguments.
    unsafe { e_oauth2_services_new() }
}

fn borrowed(pointer: *const c_char) -> Option<String> {
    // SAFETY: each wrapper below returns NULL or a NUL-terminated string that
    // is valid for the duration of this call. `to_owned` needs only that.
    (!pointer.is_null()).then(|| {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    })
}

#[test]
fn the_name_and_display_name_are_fixed_and_not_source_specific() {
    let service = service_in(registry());

    // SAFETY: a live implementer of the interface.
    let name = unsafe { e_oauth2_service_get_name(service) };
    let display_name = unsafe { e_oauth2_service_get_display_name(service) };

    assert_eq!(borrowed(name).as_deref(), Some("JMAP"));
    // A test binary binds no catalogue, thus gettext returns the msgid. This
    // is the documented "no translation available" case, not a failure.
    assert_eq!(borrowed(display_name).as_deref(), Some("JMAP"));
}

#[test]
fn every_borrowed_field_reads_back_what_was_applied() {
    let service = service_in(registry());
    let source = TestSource::new().written(&config());

    // SAFETY: a live implementer and a live source, for the duration of each
    // call.
    unsafe {
        assert_eq!(
            borrowed(e_oauth2_service_get_client_id(service, source.0)).as_deref(),
            Some("client-abc123")
        );
        assert_eq!(
            borrowed(e_oauth2_service_get_client_secret(service, source.0)).as_deref(),
            Some("s3cret")
        );
        assert_eq!(
            borrowed(e_oauth2_service_get_authentication_uri(service, source.0)).as_deref(),
            Some("https://jmap.example.com/authorize")
        );
        assert_eq!(
            borrowed(e_oauth2_service_get_refresh_uri(service, source.0)).as_deref(),
            Some("https://jmap.example.com/token")
        );
        assert_eq!(
            borrowed(e_oauth2_service_get_redirect_uri(service, source.0)).as_deref(),
            Some("https://client.example.org/callback")
        );
    }
}

/// The authorization request must NAME the scope that this client registered.
/// If `scope` is omitted, the server decides if it uses the registered scope
/// (RFC 6749 §3.3). Fastmail sent `error=invalid_scope` for that omission
/// (observed live 2026-08-23).
///
/// The test calls `e_oauth2_service_prepare_authentication_uri_query` of EDS.
/// Thus it also tests the chain to the default (which builds the standard
/// query, here checked through `response_type`), not only the added key.
#[test]
fn prepare_authentication_uri_query_names_the_registered_scope() {
    let service = service_in(registry());
    let source = TestSource::new().written(&config());

    // SAFETY: standard GLib hash-table construction. The table owns the
    // string keys and values, as in the table that the EDS caller builds.
    let query = unsafe {
        g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        )
    };
    // SAFETY: a live implementer, a live source, and a live table.
    unsafe {
        e_oauth2_service_prepare_authentication_uri_query(service, source.0, query);

        let scope = g_hash_table_lookup(query, c"scope".as_ptr().cast());
        assert!(!scope.is_null(), "the query carries no scope");
        assert_eq!(
            CStr::from_ptr(scope.cast()).to_str().unwrap(),
            "urn:ietf:params:oauth:scope:mail offline_access"
        );
        // The wrapper ran the EDS default before it dispatched to the slot of
        // this crate. Thus the standard query is here with the added keys. The
        // wrapper gives this guarantee, not the slot. The test
        // `the_authentication_query_slot_adds_only_this_services_own_keys`
        // fails if the slot also runs the default.
        let response_type = g_hash_table_lookup(query, c"response_type".as_ptr().cast());
        assert!(
            !response_type.is_null(),
            "the standard RFC 6749 query is gone"
        );
        // RFC 8707: the query also contains the stored resource.
        let resource = g_hash_table_lookup(query, c"resource".as_ptr().cast());
        assert!(!resource.is_null(), "the query carries no resource");
        assert_eq!(
            CStr::from_ptr(resource.cast()).to_str().unwrap(),
            "https://jmap.example.com/session"
        );
        // RFC 7636 PKCE: EDS 3.52 has no PKCE, but providers require it. The
        // challenge must be here, and S256 is the only permitted method.
        let method = g_hash_table_lookup(query, c"code_challenge_method".as_ptr().cast());
        assert!(
            !method.is_null(),
            "the query carries no code_challenge_method"
        );
        assert_eq!(CStr::from_ptr(method.cast()).to_str().unwrap(), "S256");
        let challenge = g_hash_table_lookup(query, c"code_challenge".as_ptr().cast());
        assert!(!challenge.is_null(), "the query carries no code_challenge");
        assert_eq!(
            CStr::from_ptr(challenge.cast()).to_str().unwrap().len(),
            43,
            "an S256 challenge is 32 base64url octets"
        );

        g_hash_table_destroy(query);
    }
}

/// The verifier redeemed at the token endpoint must be the secret behind the
/// challenge in the authorization request (RFC 7636 §4.5). The test calls the
/// two EDS entry points in sequence, as the credentials prompter does.
#[test]
fn the_token_form_redeems_the_verifier_behind_the_authorization_challenge() {
    use base64::Engine as _;
    use sha2::Digest as _;

    let service = service_in(registry());
    let source = TestSource::new().written(&config());

    // SAFETY: standard GLib hash tables and live objects in all calls.
    unsafe {
        let query = g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        );
        e_oauth2_service_prepare_authentication_uri_query(service, source.0, query);
        let challenge = CStr::from_ptr(
            g_hash_table_lookup(query, c"code_challenge".as_ptr().cast()).cast::<c_char>(),
        )
        .to_str()
        .unwrap()
        .to_owned();

        let form = g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        );
        e_oauth2_service_prepare_get_token_form(service, source.0, c"an-auth-code".as_ptr(), form);
        let verifier = g_hash_table_lookup(form, c"code_verifier".as_ptr().cast());
        assert!(
            !verifier.is_null(),
            "the token form carries no code_verifier"
        );
        let verifier = CStr::from_ptr(verifier.cast::<c_char>()).to_str().unwrap();
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(sha2::Sha256::digest(verifier.as_bytes())),
            challenge,
            "the redeemed verifier is not the secret behind the sent challenge"
        );
        // Single-use: a second token form gets no verifier.
        let form2 = g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        );
        e_oauth2_service_prepare_get_token_form(service, source.0, c"an-auth-code".as_ptr(), form2);
        assert!(
            g_hash_table_lookup(form2, c"code_verifier".as_ptr().cast()).is_null(),
            "a PKCE verifier must be single-use"
        );

        g_hash_table_destroy(query);
        g_hash_table_destroy(form);
        g_hash_table_destroy(form2);
    }
}

/// A deployment that advertises no scopes stores no scope. The query must
/// stay the same as the EDS default built it. An empty `scope` is not
/// permitted, because RFC 6749 treats it as a separate (probably wrong) request.
#[test]
fn prepare_authentication_uri_query_adds_nothing_without_a_stored_scope() {
    let service = service_in(registry());
    let source = TestSource::new().written(&Config {
        scope: None,
        ..config()
    });

    // SAFETY: the same as above.
    let query = unsafe {
        g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        )
    };
    // SAFETY: the same as above.
    unsafe {
        e_oauth2_service_prepare_authentication_uri_query(service, source.0, query);
        assert!(
            g_hash_table_lookup(query, c"scope".as_ptr().cast()).is_null(),
            "no stored scope must mean no scope parameter at all"
        );
        g_hash_table_destroy(query);
    }
}

/// A stored but empty scope is also not a scope to send: the same rule as
/// `None`. Previously, a manual raw-pointer presence check did this, and this
/// test kept that behaviour.
#[test]
fn prepare_authentication_uri_query_treats_an_empty_stored_scope_as_absent() {
    let service = service_in(registry());
    let source = TestSource::new().written(&Config {
        scope: Some(String::new()),
        ..config()
    });

    // SAFETY: the same as above.
    let query = unsafe {
        g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        )
    };
    // SAFETY: the same as above.
    unsafe {
        e_oauth2_service_prepare_authentication_uri_query(service, source.0, query);
        assert!(
            g_hash_table_lookup(query, c"scope".as_ptr().cast()).is_null(),
            "an empty stored scope must mean no scope parameter at all"
        );
        g_hash_table_destroy(query);
    }
}

/// The same rule as for scope, for the RFC 8707 resource indicator: do not
/// send an empty stored value.
#[test]
fn prepare_authentication_uri_query_treats_an_empty_stored_resource_as_absent() {
    let service = service_in(registry());
    let source = TestSource::new().written(&Config {
        resource: Some(String::new()),
        ..config()
    });
    let query = empty_table();

    let slot = vtable()
        .prepare_authentication_uri_query
        .expect("this crate fills the slot");
    // SAFETY: the signature of the slot, a live implementer, a live source and
    // a live table that owns the two halves of each entry.
    unsafe { slot(service, source.0, query) };

    assert!(
        !has_key(query, c"resource"),
        "an empty stored resource must mean no resource parameter at all"
    );

    // SAFETY: a live table that this test owns.
    unsafe { g_hash_table_destroy(query) };
}

#[test]
fn an_unconfigured_source_answers_null_for_every_borrowed_field_rather_than_creating_the_extension()
{
    let service = service_in(registry());
    let source = TestSource::new();

    // SAFETY: the same as `every_borrowed_field_reads_back_what_was_applied`.
    unsafe {
        assert!(e_oauth2_service_get_client_id(service, source.0).is_null());
        assert!(e_oauth2_service_get_client_secret(service, source.0).is_null());
        assert!(e_oauth2_service_get_authentication_uri(service, source.0).is_null());
        assert!(e_oauth2_service_get_refresh_uri(service, source.0).is_null());
        assert!(e_oauth2_service_get_redirect_uri(service, source.0).is_null());
    }
}

#[test]
fn can_process_says_yes_only_when_the_authentication_method_is_this_services_name() {
    let service = service_in(registry());
    let matching = TestSource::new().with_authentication_method(NAME);
    let other = TestSource::new().with_authentication_method(c"Basic");
    let unset = TestSource::new();

    // SAFETY: a live implementer and live sources.
    unsafe {
        assert_ne!(
            e_oauth2_service_can_process(service, matching.0),
            GFALSE,
            "a source authenticating by this service's own name should match"
        );
        assert_eq!(
            e_oauth2_service_can_process(service, other.0),
            GFALSE,
            "a source authenticating by a different method should not match"
        );
        assert_eq!(
            e_oauth2_service_can_process(service, unset.0),
            GFALSE,
            "a source with no Authentication extension should not match"
        );
    }
}

#[test]
fn the_registry_finds_this_service_for_a_matching_source_and_nothing_for_another() {
    // The path that production code calls: not `can_process` on a known
    // service, but `e_oauth2_services_find` over all registered services. For
    // this path, this file builds a real `EOAuth2Services` singleton and a real
    // registration through `constructed()`.
    //
    // Not asserted: pointer identity between the answer of `find` and the
    // `service` that this test registered. `EOAuth2Services` is the
    // process-wide singleton that `e-oauth2-services.c` documents. The tests
    // of this crate never remove what they add. Each registered `Service`
    // (from this test or a concurrent test) is stateless and gives the same
    // `can_process` answer for a matching source. Thus more than one can be
    // registered when this test runs, and `find` can return any of them. The
    // important result, unique to the registration of *this* service: a
    // matching source finds a service and a non-matching source finds none.
    let registry = registry();
    let _service = service_in(registry);
    let matching = TestSource::new().with_authentication_method(NAME);
    let other = TestSource::new().with_authentication_method(c"Basic");

    // SAFETY: a live registry with `_service` registered in it, and live
    // sources. A non-NULL result is a new ref that this test releases.
    unsafe {
        let found = e_oauth2_services_find(registry, matching.0);
        assert!(
            !found.is_null(),
            "a source authenticating by this service's own name should be found"
        );
        g_object_unref(found.cast());

        assert!(
            e_oauth2_services_find(registry, other.0).is_null(),
            "a source authenticating by a different method should not be found"
        );
    }
}

/// The `EOAuth2ServiceInterface` vtable, found as the EDS wrapper finds it:
/// `E_OAUTH2_SERVICE_GET_INTERFACE`. This is the interface vtable of the
/// [`Service`] class, not the *default* vtable of the interface. The default
/// vtable still holds `eos_default_*` for each slot that this crate does not fill.
fn vtable() -> &'static EOAuth2ServiceInterface {
    let gtype = register_static::<Service>();
    // SAFETY: a registered class type and the interface that it implements.
    // This class ref is deliberately never released: the type is
    // process-global, and all other tests also hold an instance of it.
    unsafe {
        let class = g_type_class_ref(gtype);
        let vtable = g_type_interface_peek(class, e_oauth2_service_get_type())
            .cast::<EOAuth2ServiceInterface>();
        assert!(
            !vtable.is_null(),
            "Service does not implement EOAuth2Service"
        );
        &*vtable
    }
}

fn empty_table() -> *mut GHashTable {
    // SAFETY: standard GLib hash-table construction. The EDS `prepare_*`
    // callers build this shape, and `g_hash_table_replace` needs it to own the
    // two halves.
    unsafe {
        g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        )
    }
}

fn has_key(table: *mut GHashTable, key: &CStr) -> bool {
    // SAFETY: a live table with string keys.
    !unsafe { g_hash_table_lookup(table, key.as_ptr().cast()) }.is_null()
}

/// The interface slot is an **additive** hook, not a chain link.
///
/// `e_oauth2_service_prepare_authentication_uri_query` (3.52.3,
/// `e-oauth2-service.c:648`) always calls
/// `eos_default_prepare_authentication_uri_query` itself, *before* it
/// dispatches to the vtable. It skips the slot only when the slot still holds
/// that default (`:654`). Thus a filled slot that also calls the default runs
/// it two times. The EDS services set the convention:
/// `eos_google_prepare_authentication_uri_query` and
/// `eos_outlook_prepare_authentication_uri_query` each set only their own keys.
/// They do not keep or call the default.
///
/// Only a direct call to the slot shows the difference. Through the wrapper,
/// the two shapes make the same table, because
/// `e_oauth2_service_util_set_to_form` is insert-or-remove, and two runs give
/// the same result as one.
#[test]
fn the_authentication_query_slot_adds_only_this_services_own_keys() {
    let service = service_in(registry());
    let source = TestSource::new().written(&config());
    let query = empty_table();

    let slot = vtable()
        .prepare_authentication_uri_query
        .expect("this crate fills the slot");
    // SAFETY: the signature of the slot, a live implementer, a live source and
    // a live table that owns the two halves of each entry.
    unsafe { slot(service, source.0, query) };

    assert!(has_key(query, c"scope"), "the slot must add the scope");
    assert!(
        has_key(query, c"resource"),
        "the slot must add the resource"
    );
    assert!(
        has_key(query, c"code_challenge"),
        "the slot must add the PKCE challenge"
    );
    for base in [c"response_type", c"client_id", c"redirect_uri"] {
        assert!(
            !has_key(query, base),
            "the slot re-ran EDS's default: `{}` is the wrapper's to set, and it \
             already set it before dispatching here",
            base.to_string_lossy()
        );
    }

    // SAFETY: a live table that this test owns.
    unsafe { g_hash_table_destroy(query) };
}

/// The same as above, for the code grant.
/// `e_oauth2_service_prepare_get_token_form` (`e-oauth2-service.c:822`) and
/// `prepare_refresh_token_form` (`:895`) have the same shape.
#[test]
fn the_token_form_slots_add_only_this_services_own_keys() {
    let service = service_in(registry());
    let source = TestSource::new().written(&config());

    // Authorize first, so that the code grant has a stored verifier to redeem.
    // The credentials prompter calls the two in this sequence.
    let query = empty_table();
    let authorize = vtable()
        .prepare_authentication_uri_query
        .expect("this crate fills the slot");
    // SAFETY: the same as in the test above.
    unsafe { authorize(service, source.0, query) };

    let form = empty_table();
    let slot = vtable()
        .prepare_get_token_form
        .expect("this crate fills the slot");
    // SAFETY: the same as above. `authorization_code` is a NUL-terminated
    // literal.
    unsafe { slot(service, source.0, c"an-auth-code".as_ptr(), form) };

    assert!(has_key(form, c"resource"), "the slot must add the resource");
    assert!(
        has_key(form, c"code_verifier"),
        "the slot must redeem the PKCE verifier"
    );
    for base in [c"code", c"client_id", c"client_secret", c"grant_type"] {
        assert!(
            !has_key(form, base),
            "the get-token slot re-ran EDS's default: `{}` is the wrapper's",
            base.to_string_lossy()
        );
    }

    let refresh = empty_table();
    let slot = vtable()
        .prepare_refresh_token_form
        .expect("this crate fills the slot");
    // SAFETY: the same as above.
    unsafe { slot(service, source.0, c"a-refresh-token".as_ptr(), refresh) };

    assert!(
        has_key(refresh, c"resource"),
        "the slot must add the resource"
    );
    for base in [c"refresh_token", c"client_id", c"grant_type"] {
        assert!(
            !has_key(refresh, base),
            "the refresh slot re-ran EDS's default: `{}` is the wrapper's",
            base.to_string_lossy()
        );
    }

    // SAFETY: live tables that this test owns.
    unsafe {
        g_hash_table_destroy(query);
        g_hash_table_destroy(form);
        g_hash_table_destroy(refresh);
    }
}

/// A code exchange with no stored PKCE verifier causes a repeating consent
/// window with no known cause. Thus the log level must be high enough to find
/// in the journal: `warn!`, not `debug!`.
///
/// A verifier is missing here only for a second redemption (single-use), or
/// when it was never stored (no uid). `e_source_new*` always makes a uid, thus
/// no test can build the second case. This test covers the first case: it
/// redeems the same code two times.
#[test]
fn a_missing_pkce_verifier_at_redemption_is_logged_at_warn_level() {
    let service = service_in(registry());
    let source = TestSource::new().written(&config());

    let query = empty_table();
    let authorize = vtable()
        .prepare_authentication_uri_query
        .expect("this crate fills the slot");
    let form = empty_table();
    let form2 = empty_table();
    let slot = vtable()
        .prepare_get_token_form
        .expect("this crate fills the slot");

    let first_levels = capture_levels(|| {
        // SAFETY: live objects and tables in all calls.
        unsafe {
            authorize(service, source.0, query);
            slot(service, source.0, c"an-auth-code".as_ptr(), form);
        }
    });
    assert!(
        !first_levels.contains(&Level::WARN),
        "a redemption with a stashed verifier must not warn: {first_levels:?}"
    );

    let second_levels = capture_levels(|| {
        // SAFETY: the same as above. The verifier is already redeemed one time.
        unsafe { slot(service, source.0, c"an-auth-code".as_ptr(), form2) };
    });
    assert!(
        second_levels.contains(&Level::WARN),
        "a redemption with no stashed verifier must warn, not just debug!: {second_levels:?}"
    );

    // SAFETY: live tables that this test owns.
    unsafe {
        g_hash_table_destroy(query);
        g_hash_table_destroy(form);
        g_hash_table_destroy(form2);
    }
}
