// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `oauth2_service::Service`, dispatched through EDS's own `e_oauth2_service_*`
// wrappers — the same style `eds-sys/tests/oauth2.rs` proves the raw ABI with,
// now proving this crate's actual implementation answers correctly: the four
// vfuncs with no default read what `oauth2::apply` wrote, the three vfuncs
// whose default is wrong for a per-account value do too, and `can_process` —
// deliberately left unfilled, see `oauth2_service`'s module docs — says yes
// only when `[Authentication] method` is this service's own name.
//
// Instances are built through a real `EOAuth2Services` registry rather than a
// bare `g_object_new`, because `EOAuth2ServiceBase` is not a bare `GObject`:
// its `constructed()` (`e-oauth2-service-base.c`, read rather than assumed)
// reads the `extensible` construct-only property and calls
// `e_oauth2_services_add()` on it. Omitting that property still constructs an
// instance — GObject applies the property's `NULL` default either way — but
// it does so through two `E_IS_EXTENSIBLE`/`E_IS_OAUTH2_SERVICES` assertion
// failures logged as `CRITICAL`, which is not a shape of "passing" this
// project accepts anywhere else. Building the real object this type expects
// to be built with, once, costs one helper and also lets
// `e_oauth2_services_find` be exercised directly — the entry point real code
// will actually call.

use std::ffi::{CStr, CString, c_char};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, EOAuth2Service, EOAuth2Services, ESource,
    e_oauth2_service_can_process, e_oauth2_service_get_authentication_uri,
    e_oauth2_service_get_client_id, e_oauth2_service_get_client_secret,
    e_oauth2_service_get_display_name, e_oauth2_service_get_name,
    e_oauth2_service_get_redirect_uri, e_oauth2_service_get_refresh_uri, e_oauth2_services_find,
    e_oauth2_services_new, e_source_authentication_set_method, e_source_get_extension,
    e_source_new_with_uid,
};
use glib_sys::GFALSE;
use gobject_sys::{
    G_TYPE_OBJECT, GValue, g_object_new_with_properties, g_object_unref, g_value_init,
    g_value_set_object, g_value_unset,
};
use jmap_backend_core::subclass::register_static;
use jmap_config::oauth2::{self, Config};
use jmap_config::oauth2_service::{NAME, Service};

struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        let uid = CString::new("jmap-oauth2-service-test").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, no D-Bus object and a GError
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

    /// Sets `[Authentication] method` — the value `can_process`'s default
    /// matches against `get_name()`'s answer.
    fn with_authentication_method(self, method: &CStr) -> Self {
        // SAFETY: a live source; `E_SOURCE_EXTENSION_AUTHENTICATION` is EDS's
        // own extension name and always creatable.
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
    }
}

/// A live instance of [`Service`], registered into `registry` the way
/// `e_oauth2_service_base_constructed` requires — see this file's own header
/// comment for why a bare `g_object_new` is not that.
fn service_in(registry: *mut EOAuth2Services) -> *mut EOAuth2Service {
    let gtype = register_static::<Service>();
    let name = c"extensible";

    // SAFETY: a freshly zeroed GValue, initialised to hold an object and set
    // to a live `registry` before `g_object_new_with_properties` reads it;
    // unset afterwards, which only drops this call's own ref.
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

/// `EOAuth2Services` is a process-wide singleton (`e-oauth2-services.c`'s
/// `oauth2_services_constructor` hands back the same object for as long as
/// anything holds a ref) — harmless here, since every test's service answers
/// identically, but worth naming so a reader does not expect one test's
/// registrations to be invisible to another's.
fn registry() -> *mut EOAuth2Services {
    // SAFETY: no arguments.
    unsafe { e_oauth2_services_new() }
}

fn borrowed(pointer: *const c_char) -> Option<String> {
    // SAFETY: every wrapper below either returns NULL or a NUL-terminated
    // string valid for the length of this call, which is all `to_owned` needs.
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
    // No catalogue is bound in a test binary, so gettext echoes the msgid
    // back — the documented "no translation available" case, not a failure.
    assert_eq!(borrowed(display_name).as_deref(), Some("JMAP"));
}

#[test]
fn every_borrowed_field_reads_back_what_was_applied() {
    let service = service_in(registry());
    let source = TestSource::new().written(&config());

    // SAFETY: a live implementer and a live source, for the length of each
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

#[test]
fn an_unconfigured_source_answers_null_for_every_borrowed_field_rather_than_creating_the_extension()
{
    let service = service_in(registry());
    let source = TestSource::new();

    // SAFETY: as `every_borrowed_field_reads_back_what_was_applied`.
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
    // The path production code actually calls: not `can_process` on a
    // service in hand, but `e_oauth2_services_find` over every registered
    // service, which is what a real `EOAuth2Services` singleton and a real
    // `constructed()`-driven registration are being built for in this file.
    //
    // Not asserted: pointer identity between `find`'s answer and the
    // `service` this test just registered. `EOAuth2Services` is the
    // process-wide singleton `e-oauth2-services.c` documents, this crate's
    // tests never remove what they add, and every registered `Service` — from
    // this test or another running concurrently — answers a matching source's
    // `can_process` identically, being stateless. So more than one may be
    // registered by the time this runs, and `find` is free to hand back any
    // of them; what matters, and is unique to *this* service's own
    // registration, is that a matching source finds something and a
    // non-matching one finds nothing.
    let registry = registry();
    let _service = service_in(registry);
    let matching = TestSource::new().with_authentication_method(NAME);
    let other = TestSource::new().with_authentication_method(c"Basic");

    // SAFETY: a live registry with `_service` registered in it, and live
    // sources; a non-NULL result is a new ref this test drops.
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
