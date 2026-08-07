// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP surface: routing, session document, server lifecycle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jmap_proto::session::{
    Account, CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
    CAPABILITY_SUBMISSION, Session,
};
use serde_json::{Value, json};

use crate::auth::AuthConfig;
use crate::dispatch;
use crate::state::ServerState;

pub const DEFAULT_ACCOUNT_ID: &str = "A1";
pub const DEFAULT_ACCOUNT_NAME: &str = "alice@example.com";

pub struct MockServerBuilder {
    auth: AuthConfig,
    port: u16,
}

impl MockServerBuilder {
    /// Accept HTTP Basic credentials. May be combined with a bearer token;
    /// configuring any credential makes authentication mandatory.
    pub fn basic_auth(mut self, user: &str, password: &str) -> Self {
        self.auth.allow_basic(user, password);
        self
    }

    /// Accept a Bearer token.
    pub fn bearer_token(mut self, token: &str) -> Self {
        self.auth.allow_bearer(token);
        self
    }

    /// Bind to a fixed localhost port instead of an ephemeral one.
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Bind to localhost and start serving on a background thread. The
    /// server stops when the returned handle is dropped.
    pub fn start(self) -> MockServer {
        let mut state = ServerState::new();
        state.add_account(DEFAULT_ACCOUNT_ID, DEFAULT_ACCOUNT_NAME);
        let state = Arc::new(Mutex::new(state));

        let server = tiny_http::Server::http(format!("127.0.0.1:{}", self.port))
            .expect("bind mock server to localhost");
        let port = server
            .server_addr()
            .to_ip()
            .expect("mock server has an IP address")
            .port();
        let origin = format!("http://127.0.0.1:{port}");

        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn({
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            let origin = origin.clone();
            move || serve(server, state, self.auth, origin, stop)
        });

        MockServer {
            origin,
            state,
            stop,
            handle: Some(handle),
        }
    }
}

/// A running mock server bound to an ephemeral localhost port.
pub struct MockServer {
    origin: String,
    state: Arc<Mutex<ServerState>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    pub fn builder() -> MockServerBuilder {
        MockServerBuilder {
            auth: AuthConfig::default(),
            port: 0,
        }
    }

    /// `http://127.0.0.1:<port>` — pass to `Client::connect`.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The default account's id.
    pub fn account_id(&self) -> jmap_proto::Id {
        jmap_proto::Id::new(DEFAULT_ACCOUNT_ID)
    }

    /// Shared state handle for seeding and white-box assertions.
    pub fn state(&self) -> Arc<Mutex<ServerState>> {
        Arc::clone(&self.state)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(
    server: tiny_http::Server,
    state: Arc<Mutex<ServerState>>,
    auth: AuthConfig,
    origin: String,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_millis(20)) {
            Ok(Some(request)) => handle_request(request, &state, &auth, &origin),
            Ok(None) => {}
            Err(_) => break,
        }
    }
}

fn handle_request(
    mut request: tiny_http::Request,
    state: &Mutex<ServerState>,
    auth: &AuthConfig,
    origin: &str,
) {
    let authorization = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .map(|header| header.value.as_str().to_owned());
    if !auth.authorized(authorization.as_deref()) {
        respond_json(
            request,
            401,
            &json!({
                "type": "urn:ietf:params:jmap:error:unauthorized",
                "status": 401,
                "detail": "authentication required",
            }),
        );
        return;
    }

    let url = request.url().to_owned();
    let path = url.split('?').next().unwrap_or(&url).to_owned();
    let method = request.method().clone();

    match (method, path.as_str()) {
        (tiny_http::Method::Get, "/.well-known/jmap") => {
            let state = state.lock().expect("mock state lock");
            let session = session_document(&state, origin);
            respond_json(
                request,
                200,
                &serde_json::to_value(&session).expect("session serializes"),
            );
        }
        (tiny_http::Method::Post, "/jmap") => {
            let mut body = Vec::new();
            if request.as_reader().read_to_end(&mut body).is_err() {
                respond_json(request, 400, &json!({"detail": "unreadable body"}));
                return;
            }
            let mut state = state.lock().expect("mock state lock");
            let (status, response) = dispatch::handle_api(&mut state, &body);
            drop(state);
            respond_json(request, status, &response);
        }
        (tiny_http::Method::Post, _) if path.starts_with("/upload/") => {
            // /upload/{accountId} (RFC 8620 §6.1)
            let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            let ["upload", account_id] = segments.as_slice() else {
                respond_json(
                    request,
                    404,
                    &json!({"status": 404, "detail": "bad upload path"}),
                );
                return;
            };
            let account_id = jmap_proto::Id::new(*account_id);
            let content_type = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("Content-Type"))
                .map(|header| header.value.as_str().to_owned())
                .unwrap_or_else(|| "application/octet-stream".to_owned());
            let mut data = Vec::new();
            if request.as_reader().read_to_end(&mut data).is_err() {
                respond_json(request, 400, &json!({"detail": "unreadable body"}));
                return;
            }
            let mut state = state.lock().expect("mock state lock");
            let Some(account) = state.account_mut(&account_id) else {
                drop(state);
                respond_json(
                    request,
                    404,
                    &json!({"status": 404, "detail": "no such account"}),
                );
                return;
            };
            let size = data.len() as u64;
            let blob_id = account.add_blob(content_type.clone(), data);
            drop(state);
            respond_json(
                request,
                201,
                &json!({
                    "accountId": account_id,
                    "blobId": blob_id,
                    "type": content_type,
                    "size": size,
                }),
            );
        }
        (tiny_http::Method::Get, _) if path.starts_with("/download/") => {
            // /download/{accountId}/{blobId}/{name}
            let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            let ["download", account_id, blob_id, _name] = segments.as_slice() else {
                respond_json(
                    request,
                    404,
                    &json!({"status": 404, "detail": "bad download path"}),
                );
                return;
            };
            let state = state.lock().expect("mock state lock");
            let blob = state
                .account(&jmap_proto::Id::new(*account_id))
                .and_then(|account| account.blobs.get(&jmap_proto::Id::new(*blob_id)));
            match blob {
                Some(blob) => {
                    let response = tiny_http::Response::from_data(blob.data.clone())
                        .with_status_code(200)
                        .with_header(
                            tiny_http::Header::from_bytes(
                                &b"Content-Type"[..],
                                blob.content_type.as_bytes(),
                            )
                            .expect("content type header"),
                        );
                    drop(state);
                    let _ = request.respond(response);
                }
                None => {
                    drop(state);
                    respond_json(
                        request,
                        404,
                        &json!({"status": 404, "detail": "no such blob"}),
                    );
                }
            }
        }
        _ => respond_json(
            request,
            404,
            &json!({"status": 404, "detail": format!("no route for {path}")}),
        ),
    }
}

fn respond_json(request: tiny_http::Request, status: u16, body: &Value) {
    let payload = serde_json::to_string(body).expect("JSON body serializes");
    let response = tiny_http::Response::from_string(payload)
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("static header"),
        );
    // The client may already have hung up; nothing useful to do about it.
    let _ = request.respond(response);
}

/// Build the RFC 8620 §2 session document from current state.
fn session_document(state: &ServerState, origin: &str) -> Session {
    let mut accounts = std::collections::BTreeMap::new();
    for (id, account) in &state.accounts {
        accounts.insert(
            id.clone(),
            Account {
                name: account.name.clone(),
                is_personal: true,
                is_read_only: false,
                account_capabilities: [
                    (CAPABILITY_MAIL.to_owned(), json!({})),
                    (CAPABILITY_SUBMISSION.to_owned(), json!({})),
                    (CAPABILITY_CONTACTS.to_owned(), json!({})),
                    (CAPABILITY_CALENDARS.to_owned(), json!({})),
                ]
                .into(),
                extra: Default::default(),
            },
        );
    }

    let first_account = state.accounts.iter().next();
    let primary_accounts = first_account
        .map(|(id, _)| {
            [
                CAPABILITY_MAIL,
                CAPABILITY_SUBMISSION,
                CAPABILITY_CONTACTS,
                CAPABILITY_CALENDARS,
            ]
            .into_iter()
            .map(|capability| (capability.to_owned(), id.clone()))
            .collect()
        })
        .unwrap_or_default();

    Session {
        capabilities: [
            (
                CAPABILITY_CORE.to_owned(),
                json!({
                    "maxSizeUpload": 50_000_000u64,
                    "maxConcurrentUpload": 4,
                    "maxSizeRequest": 10_000_000u64,
                    "maxConcurrentRequests": 4,
                    "maxCallsInRequest": 16,
                    "maxObjectsInGet": 256,
                    "maxObjectsInSet": 128,
                    "collationAlgorithms": ["i;ascii-casemap"],
                }),
            ),
            (CAPABILITY_MAIL.to_owned(), json!({})),
            (CAPABILITY_SUBMISSION.to_owned(), json!({})),
            (CAPABILITY_CONTACTS.to_owned(), json!({})),
            (CAPABILITY_CALENDARS.to_owned(), json!({})),
        ]
        .into(),
        accounts,
        primary_accounts,
        username: first_account
            .map(|(_, account)| account.name.clone())
            .unwrap_or_default(),
        api_url: format!("{origin}/jmap"),
        download_url: format!("{origin}/download/{{accountId}}/{{blobId}}/{{name}}"),
        upload_url: format!("{origin}/upload/{{accountId}}"),
        event_source_url: format!("{origin}/eventsource"),
        state: state.session_state(),
        extra: Default::default(),
    }
}
