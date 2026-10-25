// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Targeted mutation test coverage suite for jmap-client/src/client.rs.
//!
//! Pins Credentials authorization header generation, rebase_urls_from_env parsing,
//! ClientBuilder configuration and Debug formatting, Client lifecycle, session caching,
//! URL rebasing notes, is_anonymous evaluation, primary account resolution, call id
//! generation, request size boundaries, takes_calls_in_one_request limits, invocation
//! unwrapping, cancellation precedence, and HTTP status code dispatching.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jmap_client::resolver::{Resolver, SrvTarget};
use jmap_client::transport::{
    CancelFlag, CancelScope, HttpRequest, HttpResponse, Transport, TransportError,
};
use jmap_client::{Client, Credentials, Error, rebase_urls_from_env};
use jmap_mock::MockServer;
use jmap_proto::request::{Invocation, Request};
use serde_json::{Value, json};

static ENV_MUTEX: Mutex<()> = Mutex::new(());

type TransportHandler =
    Box<dyn Fn(HttpRequest<'_>) -> Result<HttpResponse, TransportError> + Send + Sync>;

struct MockTransport {
    handler: TransportHandler,
}

impl MockTransport {
    fn new<F>(f: F) -> Self
    where
        F: Fn(HttpRequest<'_>) -> Result<HttpResponse, TransportError> + Send + Sync + 'static,
    {
        Self {
            handler: Box::new(f),
        }
    }
}

impl Transport for MockTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        (self.handler)(request)
    }
}

struct SessionConfig<'a> {
    api_url: &'a str,
    download_url: &'a str,
    username: &'a str,
    accounts: Value,
    primary_accounts: Value,
    max_size_request: Option<u64>,
    max_calls_in_request: Option<u64>,
    event_source_url: &'a str,
}

impl Default for SessionConfig<'_> {
    fn default() -> Self {
        Self {
            api_url: "http://127.0.0.1:8080/api",
            download_url: "http://127.0.0.1:8080/download",
            username: "u",
            accounts: json!({}),
            primary_accounts: json!({}),
            max_size_request: None,
            max_calls_in_request: None,
            event_source_url: "http://127.0.0.1:8080/events",
        }
    }
}

fn make_session_json(cfg: SessionConfig<'_>) -> Vec<u8> {
    let mut core = json!({
        "maxSizeUpload": 50000000,
        "maxConcurrentUpload": 4,
        "maxConcurrentRequests": 4,
        "maxObjectsInGet": 500,
        "maxObjectsInSet": 500,
        "collationAlgorithms": ["i;ascii-casemap", "i;octet"]
    });
    if let Some(limit) = cfg.max_size_request {
        core.as_object_mut()
            .unwrap()
            .insert("maxSizeRequest".into(), json!(limit));
    }
    if let Some(calls) = cfg.max_calls_in_request {
        core.as_object_mut()
            .unwrap()
            .insert("maxCallsInRequest".into(), json!(calls));
    }

    let api_url = cfg.api_url;
    serde_json::to_vec(&json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": core,
            "urn:ietf:params:jmap:mail": {}
        },
        "accounts": cfg.accounts,
        "primaryAccounts": cfg.primary_accounts,
        "username": cfg.username,
        "apiUrl": api_url,
        "downloadUrl": cfg.download_url,
        "uploadUrl": format!("{api_url}/upload"),
        "eventSourceUrl": cfg.event_source_url,
        "state": "s1"
    }))
    .unwrap()
}

fn make_ok_api_response() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "methodResponses": [["Core/echo", {"pong": true}, "c0"]],
        "sessionState": "s1"
    }))
    .unwrap()
}

#[test]
fn credentials_authorization_headers_and_client_updates() {
    let none = Credentials::none();
    assert_eq!(
        Client::builder()
            .transport(MockTransport::new(|req| {
                Ok(HttpResponse {
                    status: 200,
                    content_type: Some("application/json".into()),
                    body: make_session_json(SessionConfig {
                        username: "testuser",
                        accounts: json!({"acc1": {"name": "test@example.com", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {}}}),
                        ..Default::default()
                    }),
                    final_url: req.url.to_string(),
                })
            }))
            .connect("http://127.0.0.1:8080", none)
            .unwrap()
            .authorization_header(),
        None
    );

    let basic = Credentials::basic("aladdin", "opensesame");
    assert_eq!(
        Client::builder()
            .transport(MockTransport::new(|req| {
                Ok(HttpResponse {
                    status: 200,
                    content_type: Some("application/json".into()),
                    body: make_session_json(SessionConfig {
                        username: "testuser",
                        accounts: json!({"acc1": {"name": "test@example.com", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {}}}),
                        ..Default::default()
                    }),
                    final_url: req.url.to_string(),
                })
            }))
            .connect("http://127.0.0.1:8080", basic)
            .unwrap()
            .authorization_header(),
        Some("Basic YWxhZGRpbjpvcGVuc2VzYW1l".to_string())
    );

    let bearer = Credentials::bearer("secret-token-xyz");
    let client = Client::builder()
        .transport(MockTransport::new(|req| {
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body: make_session_json(SessionConfig {
                    username: "testuser",
                    accounts: json!({"acc1": {"name": "test@example.com", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {}}}),
                    ..Default::default()
                }),
                final_url: req.url.to_string(),
            })
        }))
        .connect("http://127.0.0.1:8080", bearer)
        .expect("connect succeeds");

    assert_eq!(
        client.authorization_header(),
        Some("Bearer secret-token-xyz".to_string())
    );

    client.set_credentials(Credentials::bearer("updated-token-123"));
    assert_eq!(
        client.authorization_header(),
        Some("Bearer updated-token-123".to_string())
    );

    client.set_credentials(Credentials::basic("user2", "pass2"));
    assert_eq!(
        client.authorization_header(),
        Some("Basic dXNlcjI6cGFzczI=".to_string())
    );
}

#[test]
fn rebase_urls_from_env_evaluation() {
    let _lock = ENV_MUTEX.lock().unwrap();

    unsafe {
        std::env::remove_var("JMAP_LIVE_SERVER_REBASE_URLS");
    }
    assert!(!rebase_urls_from_env());

    unsafe {
        std::env::set_var("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    }
    assert!(rebase_urls_from_env());

    unsafe {
        std::env::set_var("JMAP_LIVE_SERVER_REBASE_URLS", "true");
    }
    assert!(rebase_urls_from_env());

    unsafe {
        std::env::set_var("JMAP_LIVE_SERVER_REBASE_URLS", "TRUE");
    }
    assert!(rebase_urls_from_env());

    unsafe {
        std::env::set_var("JMAP_LIVE_SERVER_REBASE_URLS", "0");
    }
    assert!(!rebase_urls_from_env());

    unsafe {
        std::env::set_var("JMAP_LIVE_SERVER_REBASE_URLS", "false");
    }
    assert!(!rebase_urls_from_env());

    unsafe {
        std::env::set_var("JMAP_LIVE_SERVER_REBASE_URLS", "xyzzy");
    }
    assert!(!rebase_urls_from_env());

    unsafe {
        std::env::remove_var("JMAP_LIVE_SERVER_REBASE_URLS");
    }
}

#[test]
fn client_builder_configuration_and_debug_formatting() {
    let builder = Client::builder()
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(12))
        .read_timeout(Duration::from_secs(18))
        .rebase_urls_to_origin(true);

    let debug_repr = format!("{builder:?}");
    assert!(debug_repr.starts_with("ClientBuilder"));
    assert!(debug_repr.contains("timeout: 45s"));
    assert!(debug_repr.contains("connect_timeout: Some(12s)"));
    assert!(debug_repr.contains("read_timeout: Some(18s)"));
    assert!(debug_repr.contains("rebase_urls_to_origin: true"));
}

struct DomainResolver {
    target: Option<SrvTarget>,
}

impl Resolver for DomainResolver {
    fn lookup_srv(&self, domain: &str) -> Option<SrvTarget> {
        if domain == "example.org" {
            self.target.clone()
        } else {
            None
        }
    }
}

#[test]
fn client_builder_resolver_and_connect_domain() {
    let captured_urls = Arc::new(Mutex::new(Vec::new()));
    let urls_clone = captured_urls.clone();

    let transport = MockTransport::new(move |req| {
        urls_clone.lock().unwrap().push(req.url.to_string());
        let body = make_session_json(SessionConfig {
            api_url: "https://srv.example.org:8443/api",
            download_url: "https://srv.example.org:8443/download",
            username: "user",
            accounts: json!({"a": {"name": "u", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {}}}),
            event_source_url: "https://srv.example.org:8443/events",
            ..Default::default()
        });
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let resolver = DomainResolver {
        target: Some(SrvTarget {
            host: "srv.example.org".to_string(),
            port: 8443,
        }),
    };

    let client = Client::builder()
        .transport(transport)
        .resolver(resolver)
        .connect_domain("example.org", Credentials::none())
        .expect("connect_domain succeeds");

    assert_eq!(
        captured_urls.lock().unwrap().first().cloned(),
        Some("https://srv.example.org:8443/.well-known/jmap".to_string())
    );

    let debug_str = format!("{client:?}");
    assert!(debug_str.starts_with("Client"));
    assert!(debug_str.contains("authenticated: false"));
}

#[test]
fn client_debug_formatting_authenticated() {
    let transport = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig {
            api_url: "https://example.com/api",
            download_url: "https://example.com/download",
            username: "alice",
            accounts: json!({"a1": {"name": "alice", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {}}}),
            event_source_url: "https://example.com/events",
            ..Default::default()
        });
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client = Client::builder()
        .transport(transport)
        .connect("https://example.com", Credentials::bearer("tok123"))
        .unwrap();

    let debug_repr = format!("{client:?}");
    assert!(debug_repr.contains("authenticated: true"));
    assert!(debug_repr.contains("https://example.com/.well-known/jmap"));
}

#[test]
fn client_session_caching_and_refresh() {
    let session_version = Arc::new(Mutex::new(1));
    let version_clone = session_version.clone();

    let transport = MockTransport::new(move |req| {
        let ver = *version_clone.lock().unwrap();
        let api_url = format!("http://127.0.0.1:8080/api/v{ver}");
        let body = make_session_json(SessionConfig {
            api_url: &api_url,
            username: "charlie",
            accounts: json!({"acc": {"name": "c@example.com", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {}}}),
            ..Default::default()
        });
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let mut client = Client::builder()
        .transport(transport)
        .connect("http://127.0.0.1:8080", Credentials::none())
        .unwrap();

    assert_eq!(client.session().api_url, "http://127.0.0.1:8080/api/v1");
    assert_eq!(client.session().username, "charlie");

    *session_version.lock().unwrap() = 2;
    client.refresh_session().expect("session refresh succeeds");
    assert_eq!(client.session().api_url, "http://127.0.0.1:8080/api/v2");
}

#[test]
fn client_rebase_note_presence_and_absence() {
    let transport_different = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig {
            api_url: "http://internal.cluster.local:8080/api",
            download_url: "http://internal.cluster.local:8080/download",
            username: "dan",
            event_source_url: "http://internal.cluster.local:8080/events",
            ..Default::default()
        });
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client_rebased = Client::builder()
        .transport(transport_different)
        .rebase_urls_to_origin(true)
        .connect("http://public.proxy:9000", Credentials::none())
        .unwrap();

    assert_eq!(
        client_rebased.rebase_note(),
        Some(
            "note: JMAP_LIVE_SERVER_REBASE_URLS is active and rewrote http://internal.cluster.local:8080 to http://public.proxy:9000"
        )
    );

    let transport_same = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig {
            api_url: "http://public.proxy:9000/api",
            download_url: "http://public.proxy:9000/download",
            username: "dan",
            event_source_url: "http://public.proxy:9000/events",
            ..Default::default()
        });
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client_same = Client::builder()
        .transport(transport_same)
        .rebase_urls_to_origin(true)
        .connect("http://public.proxy:9000", Credentials::none())
        .unwrap();

    assert_eq!(client_same.rebase_note(), None);
}

#[test]
fn client_is_anonymous_truth_table() {
    let make_client = |username: &'static str, accounts: Value| {
        let transport = MockTransport::new(move |req| {
            let body = make_session_json(SessionConfig {
                username,
                accounts: accounts.clone(),
                ..Default::default()
            });
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body,
                final_url: req.url.to_string(),
            })
        });

        Client::builder()
            .transport(transport)
            .connect("http://127.0.0.1:8080", Credentials::none())
            .unwrap()
    };

    let anon_client = make_client("", json!({}));
    assert!(anon_client.is_anonymous());

    let named_empty_accounts = make_client("bob", json!({}));
    assert!(!named_empty_accounts.is_anonymous());

    let unnamed_with_accounts = make_client(
        "",
        json!({"a1": {"name": "acc1", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {}}}),
    );
    assert!(!unnamed_with_accounts.is_anonymous());

    let named_with_accounts = make_client(
        "bob",
        json!({"a1": {"name": "acc1", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {}}}),
    );
    assert!(!named_with_accounts.is_anonymous());
}

#[test]
fn client_primary_account_resolution() {
    let transport = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig {
            username: "testuser",
            accounts: json!({"mail-acc": {"name": "m", "isPersonal": true, "isReadOnly": false, "accountCapabilities": {"urn:ietf:params:jmap:mail": {}}}}),
            primary_accounts: json!({"urn:ietf:params:jmap:mail": "mail-acc"}),
            ..Default::default()
        });
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client = Client::builder()
        .transport(transport)
        .connect("http://127.0.0.1:8080", Credentials::none())
        .unwrap();

    let account_id = client
        .primary_account("urn:ietf:params:jmap:mail")
        .expect("primary account found");
    assert_eq!(account_id.as_str(), "mail-acc");

    let err = client
        .primary_account("urn:ietf:params:jmap:contacts")
        .unwrap_err();
    match err {
        Error::Protocol(msg) => {
            assert!(msg.contains("no primary account for urn:ietf:params:jmap:contacts"));
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}

#[test]
fn client_next_call_id_exact_progression() {
    let transport = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig::default());
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client = Client::builder()
        .transport(transport)
        .connect("http://127.0.0.1:8080", Credentials::none())
        .unwrap();

    assert_eq!(client.next_call_id(), "c0");
    assert_eq!(client.next_call_id(), "c1");
    assert_eq!(client.next_call_id(), "c2");
}

#[test]
fn client_api_call_request_size_boundaries() {
    let exact_req = Request::new(["urn:ietf:params:jmap:core"])
        .call("Core/echo", &json!({}), "c0")
        .unwrap();
    let exact_len = serde_json::to_vec(&exact_req).unwrap().len() as u64;

    let transport = MockTransport::new(move |req| {
        let body = make_session_json(SessionConfig {
            max_size_request: Some(exact_len),
            ..Default::default()
        });
        if req.url.ends_with("/.well-known/jmap") {
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body,
                final_url: req.url.to_string(),
            })
        } else {
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body: make_ok_api_response(),
                final_url: req.url.to_string(),
            })
        }
    });

    let client = Client::builder()
        .transport(transport)
        .connect("http://127.0.0.1:8080", Credentials::none())
        .unwrap();

    // At exact limit: body.len() == limit. Must succeed with > (limit > limit is false).
    // Replacing > with >= causes this to fail with RequestTooLarge.
    let exact_res = client.api_call(&exact_req);
    assert!(exact_res.is_ok());

    let padding = "x".repeat(50);
    let large_req = Request::new(["urn:ietf:params:jmap:core"])
        .call("Core/echo", &json!({"pad": padding}), "c0")
        .unwrap();
    let large_serialized_len = serde_json::to_vec(&large_req).unwrap().len() as u64;
    assert!(large_serialized_len > exact_len);

    let large_res = client.api_call(&large_req);
    match large_res {
        Err(Error::RequestTooLarge { size, limit }) => {
            assert_eq!(size, large_serialized_len);
            assert_eq!(limit, exact_len);
        }
        other => panic!("expected Error::RequestTooLarge, got {other:?}"),
    }
}

#[test]
fn client_takes_calls_in_one_request_limits() {
    let transport_with_limit = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig {
            max_calls_in_request: Some(3),
            ..Default::default()
        });
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client = Client::builder()
        .transport(transport_with_limit)
        .connect("http://127.0.0.1:8080", Credentials::none())
        .unwrap();

    assert!(client.takes_calls_in_one_request(2));
    assert!(client.takes_calls_in_one_request(3));
    assert!(!client.takes_calls_in_one_request(4));

    let transport_without_limit = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig::default());
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client_unlimited = Client::builder()
        .transport(transport_without_limit)
        .connect("http://127.0.0.1:8080", Credentials::none())
        .unwrap();

    assert!(client_unlimited.takes_calls_in_one_request(500));
}

#[test]
fn client_unwrap_invocation_variants() {
    let ok_invocation = Invocation::from_value("Core/echo", json!({"key": "val"}), "c0");
    let unwrapped = Client::unwrap_invocation(&ok_invocation, "Core/echo").unwrap();
    assert_eq!(unwrapped, json!({"key": "val"}));

    let mismatched_invocation = Invocation::from_value("Something/other", json!({}), "c0");
    let mismatch_res = Client::unwrap_invocation(&mismatched_invocation, "Core/echo");
    match mismatch_res {
        Err(Error::Protocol(msg)) => {
            assert_eq!(msg, "expected response to Core/echo, got Something/other");
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }

    let error_invocation = Invocation::from_value(
        "error",
        json!({"type": "accountNotFound", "description": "no such account"}),
        "c0",
    );
    let err_res = Client::unwrap_invocation(&error_invocation, "Core/echo");
    match err_res {
        Err(Error::Method(method_err)) => {
            assert_eq!(method_err.error_type, "accountNotFound");
        }
        other => panic!("expected Error::Method, got {other:?}"),
    }
}

#[test]
fn client_echo_and_single_call_execution() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let echo_res = client.echo(json!({"ping": "pong"})).unwrap();
    assert_eq!(echo_res, json!({"ping": "pong"}));
}

#[test]
fn client_cancellation_precedence() {
    let transport = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig::default());
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client_flag = CancelFlag::new();

    let client = Client::builder()
        .transport(transport)
        .cancel_flag(client_flag.clone())
        .connect("http://127.0.0.1:8080", Credentials::none())
        .unwrap();

    let req = Request::new(["urn:ietf:params:jmap:core"])
        .call("Core/echo", &json!({}), "c0")
        .unwrap();

    client_flag.cancel();
    let err = client.api_call(&req).unwrap_err();
    assert!(matches!(err, Error::Cancelled));

    let uncancelled_scope_flag = CancelFlag::new();
    let scope = CancelScope::install(&uncancelled_scope_flag);

    let res = client.api_call(&req);
    assert!(res.is_ok());

    drop(scope);

    let err2 = client.api_call(&req).unwrap_err();
    assert!(matches!(err2, Error::Cancelled));
}

#[test]
fn client_execute_within_status_and_header_handling() {
    let mock_transport = |status: u16, final_url: &'static str, body: Vec<u8>| {
        MockTransport::new(move |req| {
            if req.url.ends_with("/.well-known/jmap") {
                Ok(HttpResponse {
                    status: 200,
                    content_type: Some("application/json".into()),
                    body: make_session_json(SessionConfig::default()),
                    final_url: req.url.to_string(),
                })
            } else {
                Ok(HttpResponse {
                    status,
                    content_type: Some("application/json".into()),
                    body: body.clone(),
                    final_url: final_url.to_string(),
                })
            }
        })
    };

    let client_with_auth = |status: u16, final_url: &'static str, body: Vec<u8>| {
        Client::builder()
            .transport(mock_transport(status, final_url, body))
            .connect("http://127.0.0.1:8080", Credentials::bearer("token"))
            .unwrap()
    };

    let client_no_auth = |status: u16, final_url: &'static str, body: Vec<u8>| {
        Client::builder()
            .transport(mock_transport(status, final_url, body))
            .connect("http://127.0.0.1:8080", Credentials::none())
            .unwrap()
    };

    let req = Request::new(["urn:ietf:params:jmap:core"])
        .call("Core/echo", &json!({}), "c0")
        .unwrap();

    let cross_401 = client_with_auth(401, "http://other-host.org:9000/api", b"".to_vec());
    let err = cross_401.api_call(&req).unwrap_err();
    match err {
        Error::RedirectCredentialsWithheld {
            requested,
            followed,
        } => {
            assert_eq!(requested, "http://127.0.0.1:8080");
            assert_eq!(followed, "http://other-host.org:9000");
        }
        other => panic!("expected Error::RedirectCredentialsWithheld, got {other:?}"),
    }

    let cross_401_no_auth = client_no_auth(401, "http://other-host.org:9000/api", b"".to_vec());
    let err = cross_401_no_auth.api_call(&req).unwrap_err();
    match err {
        Error::Http { status, problem } => {
            assert_eq!(status, 401);
            assert!(problem.is_none());
        }
        other => panic!("expected Error::Http, got {other:?}"),
    }

    let same_401 = client_with_auth(401, "http://127.0.0.1:8080/api", b"".to_vec());
    let err = same_401.api_call(&req).unwrap_err();
    match err {
        Error::Http { status, problem } => {
            assert_eq!(status, 401);
            assert!(problem.is_none());
        }
        other => panic!("expected Error::Http, got {other:?}"),
    }

    let cross_200 = client_with_auth(
        200,
        "http://other-host.org:9000/api",
        make_ok_api_response(),
    );
    let res = cross_200.api_call(&req);
    assert!(res.is_ok());

    let problem_body = json!({
        "type": "urn:ietf:params:jmap:error:unknownCapability",
        "status": 400,
        "detail": "requested capability unknown"
    });
    let err_400 = client_with_auth(
        400,
        "http://127.0.0.1:8080/api",
        serde_json::to_vec(&problem_body).unwrap(),
    );
    let err = err_400.api_call(&req).unwrap_err();
    match err {
        Error::Http { status, problem } => {
            assert_eq!(status, 400);
            assert_eq!(
                problem.unwrap().error_type,
                "urn:ietf:params:jmap:error:unknownCapability"
            );
        }
        other => panic!("expected Error::Http with problem, got {other:?}"),
    }

    let err_404 = client_with_auth(404, "http://127.0.0.1:8080/api", b"not found".to_vec());
    let err = err_404.api_call(&req).unwrap_err();
    assert!(matches!(
        err,
        Error::Http {
            status: 404,
            problem: None
        }
    ));
}

#[test]
fn client_event_source_validation() {
    let transport = MockTransport::new(|req| {
        let body = make_session_json(SessionConfig {
            event_source_url: "",
            ..Default::default()
        });
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body,
            final_url: req.url.to_string(),
        })
    });

    let client = Client::builder()
        .transport(transport)
        .connect("http://127.0.0.1:8080", Credentials::none())
        .unwrap();

    let err = client.event_source(&["Mailbox"]).unwrap_err();
    match err {
        Error::Protocol(msg) => {
            assert!(msg.contains("session does not advertise an eventSourceUrl"));
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}
