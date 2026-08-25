// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structured tracing tests for `jmap-config` — Track B1 follow-up.
//!
//! Asserts structured fields attached to `tracing` events during OAuth 2.0
//! discovery, client registration, protected-resource indicator probing,
//! PKCE query / token form preparation, and JMAP config lookup.

use std::ffi::CString;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eds_sys::{
    EOAuth2Services, e_oauth2_service_prepare_authentication_uri_query,
    e_oauth2_service_prepare_get_token_form, e_oauth2_service_prepare_refresh_token_form,
    e_oauth2_services_new, e_source_new_with_uid,
};
use glib_sys::{g_free, g_hash_table_destroy, g_hash_table_new_full, g_str_equal, g_str_hash};
use jmap_backend_core::subclass::register_static;
use jmap_client::resolver::{Resolver, SrvTarget};
use jmap_client::transport::UreqTransport;
use jmap_config::config_lookup::probe_host;
use jmap_config::oauth2::{self, Config};
use jmap_config::oauth2_service::Service;
use jmap_config::oauth2_setup::discover_and_register;
use jmap_mock::MockServer;
use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

const REDIRECT_URI: &str = "https://client.example.org/callback";

struct CapturingSubscriber {
    captured: Arc<Mutex<Vec<(Level, String, String)>>>,
}

struct Recorder<'a> {
    level: Level,
    sink: &'a Mutex<Vec<(Level, String, String)>>,
}

impl Visit for Recorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_owned()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }
}

impl Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> SpanId {
        SpanId::from_u64(1)
    }

    fn record(&self, _span: &SpanId, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &SpanId, _follows: &SpanId) {}

    fn event(&self, event: &Event<'_>) {
        event.record(&mut Recorder {
            level: *event.metadata().level(),
            sink: &self.captured,
        });
    }

    fn enter(&self, _span: &SpanId) {}

    fn exit(&self, _span: &SpanId) {}
}

static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

fn capture(run: impl FnOnce()) -> Vec<(Level, String, String)> {
    let _serialize = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        captured: captured.clone(),
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        run();
    });
    std::mem::take(&mut *captured.lock().unwrap())
}

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

fn metadata(origin: &str) -> serde_json::Value {
    json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/oauth/authorize"),
        "token_endpoint": format!("{origin}/oauth/token"),
        "registration_endpoint": format!("{origin}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
    })
}

fn host_and_port(server: &MockServer) -> (&str, u16) {
    let (host, port) = server
        .origin()
        .trim_start_matches("http://")
        .split_once(':')
        .expect("the mock's origin always names a port");
    (host, port.parse().expect("a numeric port"))
}

#[test]
fn discover_and_register_traces_issuer_and_client_id_on_success() {
    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .oauth_client_registration(|_request| (201, json!({"client_id": "client-123"})))
        .start();
    let (host, port) = host_and_port(&server);

    let captured = capture(|| {
        let _ = discover_and_register(
            &UreqTransport::default(),
            host,
            port,
            false,
            REDIRECT_URI,
            None,
        );
    });

    assert!(
        has(&captured, Level::DEBUG, "issuer", server.origin()),
        "expected DEBUG issuer field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "client_id", "client-123"),
        "expected DEBUG client_id field, got {captured:?}"
    );
}

#[test]
fn discover_and_register_traces_warning_on_unsupported_grant() {
    let server = MockServer::builder()
        .oauth_authorization_server(|origin| {
            let mut doc = metadata(origin);
            doc["grant_types_supported"] = json!(["implicit"]);
            doc
        })
        .start();
    let (host, port) = host_and_port(&server);

    let captured = capture(|| {
        let _ = discover_and_register(
            &UreqTransport::default(),
            host,
            port,
            false,
            REDIRECT_URI,
            None,
        );
    });

    assert!(
        has(&captured, Level::WARN, "issuer", server.origin()),
        "expected WARN issuer field on unsupported grant, got {captured:?}"
    );
}

#[test]
fn discover_and_register_traces_warning_on_missing_registration_endpoint() {
    let server = MockServer::builder()
        .oauth_authorization_server(|origin| {
            let mut doc = metadata(origin);
            doc.as_object_mut().unwrap().remove("registration_endpoint");
            doc
        })
        .start();
    let (host, port) = host_and_port(&server);

    let captured = capture(|| {
        let _ = discover_and_register(
            &UreqTransport::default(),
            host,
            port,
            false,
            REDIRECT_URI,
            None,
        );
    });

    assert!(
        has(&captured, Level::WARN, "issuer", server.origin()),
        "expected WARN issuer field on missing registration endpoint, got {captured:?}"
    );
}

#[test]
fn probe_host_traces_domain_and_srv_record() {
    struct MockResolver;
    impl Resolver for MockResolver {
        fn lookup_srv(&self, _domain: &str) -> Option<SrvTarget> {
            Some(SrvTarget {
                host: "jmap.example.com".to_owned(),
                port: 8443,
            })
        }
    }

    let captured = capture(|| {
        let result = probe_host("user@example.com", None, &MockResolver);
        assert_eq!(result.as_deref(), Some("jmap.example.com:8443"));
    });

    assert!(
        has(&captured, Level::DEBUG, "domain", "example.com"),
        "expected DEBUG domain field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "target_host", "jmap.example.com"),
        "expected DEBUG target_host field, got {captured:?}"
    );
}

struct TestSource(*mut eds_sys::ESource);

impl TestSource {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let uid = format!(
            "jmap-tracing-writes-test-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let uid = CString::new(uid).expect("no NUL in a generated uid");
        let mut error = ptr::null_mut();
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn written(self, config: &Config) -> Self {
        unsafe { oauth2::apply(self.0, config) };
        self
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        unsafe { gobject_sys::g_object_unref(self.0.cast()) };
    }
}

use gobject_sys::{
    G_TYPE_OBJECT, GValue, g_object_new_with_properties, g_value_init, g_value_set_object,
    g_value_unset,
};

fn service_in(registry: *mut EOAuth2Services) -> *mut eds_sys::EOAuth2Service {
    let gtype = register_static::<Service>();
    let name = c"extensible";

    unsafe {
        let mut value: GValue = std::mem::zeroed();
        g_value_init(&mut value, G_TYPE_OBJECT);
        g_value_set_object(&mut value, registry.cast());
        let mut names = [name.as_ptr()];
        let service = g_object_new_with_properties(gtype, 1, names.as_mut_ptr(), &value)
            .cast::<eds_sys::EOAuth2Service>();
        g_value_unset(&mut value);
        assert!(!service.is_null(), "g_object_new_with_properties failed");
        service
    }
}

fn test_config() -> Config {
    Config {
        client_id: Some("client-abc123".to_owned()),
        client_secret: Some("s3cret".to_owned()),
        authorization_endpoint: Some("https://jmap.example.com/authorize".to_owned()),
        token_endpoint: Some("https://jmap.example.com/token".to_owned()),
        redirect_uri: Some(REDIRECT_URI.to_owned()),
        scope: Some("urn:ietf:params:oauth:scope:mail offline_access".to_owned()),
        resource: Some("https://jmap.example.com/session".to_owned()),
    }
}

#[test]
fn prepare_authentication_uri_query_and_token_form_trace_structured_fields() {
    let registry = unsafe { e_oauth2_services_new() };
    let service = service_in(registry);
    let source = TestSource::new().written(&test_config());

    unsafe {
        let query = g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        );

        let captured_query = capture(|| {
            e_oauth2_service_prepare_authentication_uri_query(service, source.0, query);
        });

        assert!(
            has(&captured_query, Level::DEBUG, "has_scope", "true"),
            "expected has_scope=true in query preparation, got {captured_query:?}"
        );
        assert!(
            has(
                &captured_query,
                Level::DEBUG,
                "pkce_challenge_method",
                "S256"
            ),
            "expected pkce_challenge_method=S256, got {captured_query:?}"
        );

        let form = g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        );

        let captured_form = capture(|| {
            e_oauth2_service_prepare_get_token_form(service, source.0, c"auth-code".as_ptr(), form);
        });

        assert!(
            has(&captured_form, Level::DEBUG, "has_pkce", "true"),
            "expected has_pkce=true in token form preparation, got {captured_form:?}"
        );

        let refresh_form = g_hash_table_new_full(
            Some(g_str_hash),
            Some(g_str_equal),
            Some(g_free),
            Some(g_free),
        );

        let captured_refresh = capture(|| {
            e_oauth2_service_prepare_refresh_token_form(
                service,
                source.0,
                c"refresh-token".as_ptr(),
                refresh_form,
            );
        });

        assert!(
            captured_refresh
                .iter()
                .any(|(lvl, name, _)| *lvl == Level::DEBUG && name == "account_uid"),
            "expected account_uid in refresh token form preparation, got {captured_refresh:?}"
        );

        g_hash_table_destroy(query);
        g_hash_table_destroy(form);
        g_hash_table_destroy(refresh_form);
    }
}
