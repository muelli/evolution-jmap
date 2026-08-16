// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP surface: routing, session document, server lifecycle.

use std::collections::{BTreeMap, BTreeSet};
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

/// The `maxObjectsInGet` the mock advertises and enforces unless a test asks
/// for a different one. RFC 8620 §2 requires a server to name a limit, and one
/// this size is out of the way of every test that is not about it.
pub const DEFAULT_OBJECTS_IN_GET: u64 = 256;

/// The `maxSizeUpload` the mock advertises and enforces unless a test asks for
/// a different one — 50 MB, which is what a real server of this kind offers and
/// is out of the way of every test that is not about it.
pub const DEFAULT_SIZE_UPLOAD: u64 = 50_000_000;

/// The `maxCallsInRequest` the mock advertises and enforces unless a test asks
/// for a different one. Comfortably above the longest chain this client builds
/// (two calls), so only a test about the limit meets it.
pub const DEFAULT_CALLS_IN_REQUEST: u64 = 16;

/// The `maxSizeRequest` the mock advertises and enforces unless a test asks for
/// a different one — 10 MB, the number RFC 8620 §2's own example carries, and
/// far above the longest id list any test builds.
pub const DEFAULT_SIZE_REQUEST: u64 = 10_000_000;

/// Builds the RFC 8414 authorization-server metadata document a mock
/// deployment publishes, given the origin it ended up bound to.
///
/// A closure rather than a `Value` because the port is ephemeral: the
/// endpoints a real document names are on the deployment's own origin, which
/// nothing knows until the listener exists.
type MetadataFn = dyn Fn(&str) -> Value + Send + Sync;

/// Builds an RFC 7591 dynamic client registration response, given the
/// request body the client sent.
///
/// A closure rather than a fixed `Value`, unlike [`MetadataFn`] there is no
/// ephemeral origin to close over here — instead a test typically wants to
/// assert on what the client asked to register as (`redirect_uris`,
/// `client_name`) before answering, which only a closure over the request
/// lets it do. Returns the HTTP status alongside the body so a test can drive
/// RFC 7591 §3.2.2's error responses too.
type RegistrationFn = dyn Fn(&Value) -> (u16, Value) + Send + Sync;

/// Answers an RFC 6749 §5 token-endpoint request, given the
/// `application/x-www-form-urlencoded` fields the client sent
/// (`grant_type`, `code`/`refresh_token`, `client_id`, `code_verifier`, …).
///
/// A closure over the parsed fields, like [`RegistrationFn`], so a test can
/// assert on exactly what a client sent (the PKCE verifier a code exchange
/// carries, in particular) before deciding the status and JSON body to
/// answer with.
type TokenFn = dyn Fn(&BTreeMap<String, String>) -> (u16, Value) + Send + Sync;

/// The three OAuth 2.0 endpoint handlers a test may configure, bundled so
/// [`serve`]/[`handle_request`] take one argument for all of them rather than
/// three.
#[derive(Clone, Default)]
struct OAuthHandlers {
    metadata: Option<Arc<MetadataFn>>,
    registration: Option<Arc<RegistrationFn>>,
    token: Option<Arc<TokenFn>>,
}

pub struct MockServerBuilder {
    auth: AuthConfig,
    port: u16,
    oauth_metadata: Option<Arc<MetadataFn>>,
    oauth_registration: Option<Arc<RegistrationFn>>,
    oauth_token: Option<Arc<TokenFn>>,
    omitted_capabilities: BTreeSet<String>,
    omit_primary_accounts: bool,
    calls_in_request: Option<u64>,
    changes_page_size: Option<u64>,
    objects_in_get: Option<u64>,
    query_page_size: Option<u64>,
    size_request: Option<u64>,
    size_upload: Option<u64>,
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

    /// Leave a capability URN out of the session document entirely — the
    /// account will not list it, and no primary account resolves under it.
    ///
    /// A JMAP server need not offer all of mail, contacts and calendars, and a
    /// client that looks its account up under the wrong capability would
    /// otherwise be indistinguishable from one that looks it up under the
    /// right one, because every account here answers to all four.
    pub fn without_capability(mut self, capability: &str) -> Self {
        self.omitted_capabilities.insert(capability.to_owned());
        self
    }

    /// Leave `primaryAccounts` out of the session document entirely, while
    /// every capability still lists its accounts as usual.
    ///
    /// RFC 8620 §2 permits this outright ("a server that does not support
    /// this concept MUST omit this property") — distinct from
    /// [`Self::without_capability`], which removes the capability itself. A
    /// client that only ever reads `primaryAccounts` cannot find an account
    /// on a server shaped this way, however unambiguous the account is.
    pub fn without_primary_accounts(mut self) -> Self {
        self.omit_primary_accounts = true;
        self
    }

    /// Advertise — and enforce — `calls` as `maxCallsInRequest`, as a server
    /// that takes only short requests does.
    ///
    /// A request with more calls than this is refused whole, with RFC 8620
    /// §3.2's `urn:ietf:params:jmap:error:limit`: none of its calls run, not
    /// even the ones that would have fit. That is what makes it worth
    /// configuring — a client that chains two calls through a back-reference
    /// without reading the session document loses both here, rather than
    /// passing because the mock was permissive.
    pub fn calls_in_request(mut self, calls: u64) -> Self {
        self.calls_in_request = Some(calls);
        self
    }

    /// Leave `maxCallsInRequest` out of the session document entirely, and take
    /// a request of any length.
    ///
    /// RFC 8620 §2 requires the property, so this is a server out of spec — and
    /// it exists for the reason [`Self::no_size_upload`] does: a client has to
    /// be pinned on what it does when the number it would check against is not
    /// there.
    pub fn no_calls_in_request(mut self) -> Self {
        self.calls_in_request = None;
        self
    }

    /// Answer `/changes` in pages of at most `ids` identifiers, as a busy
    /// server does with a long backlog.
    ///
    /// RFC 8620 §5.2 lets a server split a `/changes` answer whether or not the
    /// client asked it to, so a client that stops at the first page silently
    /// misses changes. Without this there is no way to make that happen on
    /// purpose: the mock otherwise answers everything at once, which is the
    /// case that hides the bug.
    pub fn changes_page_size(mut self, ids: u64) -> Self {
        self.changes_page_size = Some(ids);
        self
    }

    /// Advertise — and enforce — `ids` as `maxObjectsInGet`, as a server with a
    /// small appetite does.
    ///
    /// An `Email/get` naming more than this is answered with
    /// `requestTooLarge`, which is what makes a client that fetches a large
    /// mailbox in one call fail rather than work by luck. The default is far
    /// above what any test seeds, so only a test about the limit sees it.
    pub fn objects_in_get(mut self, ids: u64) -> Self {
        self.objects_in_get = Some(ids);
        self
    }

    /// Answer `Email/query` with at most `ids` identifiers per response, as a
    /// server that caps a result set does.
    ///
    /// RFC 8620 §5.5 lets a server impose its own limit whether or not the
    /// client sent one, and requires it to report the limit it applied. Without
    /// this the mock always answers a query in full, which is the case that
    /// hides a client stopping at the first page.
    pub fn query_page_size(mut self, ids: u64) -> Self {
        self.query_page_size = Some(ids);
        self
    }

    /// Advertise — and enforce — `bytes` as `maxSizeRequest`, as a server that
    /// takes only small requests does.
    ///
    /// A request whose body is longer than this is refused whole, with RFC 8620
    /// §3.2's `urn:ietf:params:jmap:error:limit`, before anything in it is
    /// parsed — so a client that names a long list of ids in one `Email/get`
    /// without reading the session document gets none of them back, rather than
    /// passing because the mock was permissive.
    pub fn size_request(mut self, bytes: u64) -> Self {
        self.size_request = Some(bytes);
        self
    }

    /// Leave `maxSizeRequest` out of the session document entirely, and take a
    /// request of any size.
    ///
    /// RFC 8620 §2 requires the property, so this is a server out of spec — and
    /// it exists for the reason [`Self::no_size_upload`] does: a client has to
    /// be pinned on what it does when the number it would check against is not
    /// there.
    pub fn no_size_request(mut self) -> Self {
        self.size_request = None;
        self
    }

    /// Advertise — and enforce — `bytes` as `maxSizeUpload`, as a server with a
    /// modest appetite does.
    ///
    /// The two being one number is the point, as it is for
    /// [`Self::objects_in_get`]: an upload larger than this is answered with
    /// RFC 8620 §6.1's `urn:ietf:params:jmap:error:limit`, so a client that
    /// never reads the session document fails here rather than passing because
    /// the mock was permissive.
    pub fn size_upload(mut self, bytes: u64) -> Self {
        self.size_upload = Some(bytes);
        self
    }

    /// Leave `maxSizeUpload` out of the session document entirely, and take an
    /// upload of any size.
    ///
    /// RFC 8620 §2 requires the property, so this is a server out of spec — and
    /// it exists because a client has to be pinned on what it does when the
    /// number it would check against is not there.
    pub fn no_size_upload(mut self) -> Self {
        self.size_upload = None;
        self
    }

    /// Publish an RFC 8414 authorization-server metadata document at
    /// `/.well-known/oauth-authorization-server`, built from the origin this
    /// server binds to.
    ///
    /// Off by default, because a JMAP server need not do OAuth 2.0 at all and
    /// a client has to be able to tell that it does not. Configuring it makes
    /// the mock a deployment that *advertises* the flow; it does not make it
    /// an identity provider — nothing here issues or accepts a token, and the
    /// endpoints the document names are strings, not routes.
    pub fn oauth_authorization_server(
        mut self,
        metadata: impl Fn(&str) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.oauth_metadata = Some(Arc::new(metadata));
        self
    }

    /// Answer `POST /oauth/register` — the path this crate's own
    /// [`Self::oauth_authorization_server`] test fixtures name as
    /// `registration_endpoint` — as an RFC 7591 dynamic client registration
    /// endpoint, with `handler` deciding the status and body from the
    /// request body the client sent.
    ///
    /// Off by default: a deployment need not offer registration even if it
    /// does OAuth 2.0 at all (a self-hosted server might hand out a fixed
    /// `client_id` instead), and a client has to be able to tell "not here"
    /// from a network failure.
    pub fn oauth_client_registration(
        mut self,
        handler: impl Fn(&Value) -> (u16, Value) + Send + Sync + 'static,
    ) -> Self {
        self.oauth_registration = Some(Arc::new(handler));
        self
    }

    /// Answer `POST /oauth/token` — the path this crate's own
    /// [`Self::oauth_authorization_server`] test fixtures name as
    /// `token_endpoint` — as an RFC 6749 §4.1.3/§6 token endpoint, with
    /// `handler` deciding the status and body from the form fields the
    /// client sent.
    ///
    /// Off by default, matching [`Self::oauth_client_registration`]: a
    /// deployment need not answer requests here at all until a test asks it
    /// to.
    pub fn oauth_token(
        mut self,
        handler: impl Fn(&BTreeMap<String, String>) -> (u16, Value) + Send + Sync + 'static,
    ) -> Self {
        self.oauth_token = Some(Arc::new(handler));
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
        state.omitted_capabilities = self.omitted_capabilities.clone();
        state.omit_primary_accounts = self.omit_primary_accounts;
        state.calls_in_request = self.calls_in_request;
        state.changes_page_size = self.changes_page_size;
        state.objects_in_get = self.objects_in_get;
        state.query_page_size = self.query_page_size;
        state.size_request = self.size_request;
        state.size_upload = self.size_upload;
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
        let oauth = OAuthHandlers {
            metadata: self.oauth_metadata.clone(),
            registration: self.oauth_registration.clone(),
            token: self.oauth_token.clone(),
        };
        let handle = std::thread::spawn({
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            let origin = origin.clone();
            move || serve(server, state, self.auth, oauth, origin, stop)
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
            oauth_metadata: None,
            oauth_registration: None,
            oauth_token: None,
            omitted_capabilities: BTreeSet::new(),
            omit_primary_accounts: false,
            calls_in_request: Some(DEFAULT_CALLS_IN_REQUEST),
            changes_page_size: None,
            objects_in_get: None,
            query_page_size: None,
            size_request: Some(DEFAULT_SIZE_REQUEST),
            size_upload: Some(DEFAULT_SIZE_UPLOAD),
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

    /// The names of the method calls this server has answered so far, in
    /// order — see [`ServerState::method_calls`].
    ///
    /// A copy, because the alternative is a test holding the server's lock
    /// while it asserts, and the server needs that lock to answer the next
    /// request.
    pub fn method_calls(&self) -> Vec<String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.method_calls.clone()
    }

    /// How many requests this server has taken at the API endpoint — see
    /// [`ServerState::api_requests`].
    pub fn api_requests(&self) -> usize {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.api_requests
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
    oauth: OAuthHandlers,
    origin: String,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_millis(20)) {
            Ok(Some(request)) => handle_request(request, &state, &auth, &oauth, &origin),
            Ok(None) => {}
            Err(_) => break,
        }
    }
}

fn handle_request(
    mut request: tiny_http::Request,
    state: &Mutex<ServerState>,
    auth: &AuthConfig,
    oauth: &OAuthHandlers,
    origin: &str,
) {
    // Answered before the credential check, and only ever with 200 or 404.
    // RFC 8414 §3 has the metadata document publicly readable, which is not a
    // detail: it is what a client reads in order to find out where to *get*
    // credentials, so a deployment that demanded them here would have closed
    // the loop on itself. A server with no OAuth 2.0 must equally answer "not
    // here" rather than "who are you".
    if request.method() == &tiny_http::Method::Get
        && request.url().split('?').next().unwrap_or_default()
            == "/.well-known/oauth-authorization-server"
    {
        match oauth.metadata.as_deref() {
            Some(metadata) => respond_json(request, 200, &metadata(origin)),
            None => respond_json(
                request,
                404,
                &json!({"status": 404, "detail": "this deployment publishes no OAuth 2.0 metadata"}),
            ),
        }
        return;
    }

    // Also answered before the credential check, for the reason `discover`'s
    // module doc gives: registration is how a client obtains an identity, so
    // it cannot be gated on already having one.
    if request.method() == &tiny_http::Method::Post
        && request.url().split('?').next().unwrap_or_default() == "/oauth/register"
    {
        let mut body = Vec::new();
        if request.as_reader().read_to_end(&mut body).is_err() {
            respond_json(request, 400, &json!({"detail": "unreadable body"}));
            return;
        }
        let Ok(parsed) = serde_json::from_slice::<Value>(&body) else {
            respond_json(request, 400, &json!({"detail": "invalid JSON"}));
            return;
        };
        match oauth.registration.as_deref() {
            Some(handler) => {
                let (status, response) = handler(&parsed);
                respond_json(request, status, &response);
            }
            None => respond_json(
                request,
                404,
                &json!({"status": 404, "detail": "this deployment offers no client registration"}),
            ),
        }
        return;
    }

    // Also answered before the credential check: this client always
    // registers as a public, PKCE-only client (RFC 8252 §8.4), so it sends no
    // separate client authentication to this endpoint either — the
    // `client_id` in the form body, and the PKCE verifier or refresh token
    // proving possession, are all the identification RFC 6749 §3.2.1 asks a
    // public client for.
    if request.method() == &tiny_http::Method::Post
        && request.url().split('?').next().unwrap_or_default() == "/oauth/token"
    {
        let mut body = Vec::new();
        if request.as_reader().read_to_end(&mut body).is_err() {
            respond_json(request, 400, &json!({"error": "invalid_request"}));
            return;
        }
        let fields = parse_form_body(&body);
        match oauth.token.as_deref() {
            Some(handler) => {
                let (status, response) = handler(&fields);
                respond_json(request, status, &response);
            }
            None => respond_json(
                request,
                404,
                &json!({"status": 404, "detail": "this deployment offers no token endpoint"}),
            ),
        }
        return;
    }

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
            let decoded = path_segments(&path);
            let segments: Vec<&str> = decoded.iter().map(String::as_str).collect();
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
            let size = data.len() as u64;
            // RFC 8620 §6.1: too big is a request-level error naming the limit
            // it broke, not a stored blob and not a generic 400.
            if state.size_upload.is_some_and(|limit| size > limit) {
                drop(state);
                respond_json(
                    request,
                    400,
                    &json!({
                        "type": "urn:ietf:params:jmap:error:limit",
                        "limit": "maxSizeUpload",
                        "status": 400,
                        "detail": "the upload is larger than maxSizeUpload",
                    }),
                );
                return;
            }
            let Some(account) = state.account_mut(&account_id) else {
                drop(state);
                respond_json(
                    request,
                    404,
                    &json!({"status": 404, "detail": "no such account"}),
                );
                return;
            };
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
            let decoded = path_segments(&path);
            let segments: Vec<&str> = decoded.iter().map(String::as_str).collect();
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

/// Split a request path into its segments, each percent-decoded.
///
/// The blob endpoints are the only routes here whose path holds values a
/// client chose, and RFC 8620 §6.1/§6.2 tell that client to URI-encode each
/// one before it substitutes it into the template this server published. So
/// the escapes have to come back off, and — this is the part that matters —
/// the split has to happen *first*: an `%2F` inside an id is a character of
/// the id, not a separator, and decoding before splitting would let a
/// server-chosen value invent a path segment. That is precisely the confusion
/// the encoding exists to prevent.
fn path_segments(path: &str) -> Vec<String> {
    path.trim_start_matches('/')
        .split('/')
        .map(percent_decode)
        .collect()
}

/// Parse an `application/x-www-form-urlencoded` body (RFC 6749 §4.1.3's
/// request shape for the token endpoint) into its key/value pairs.
///
/// A literal `+` is decoded as a space before the percent-escapes are undone,
/// which is this media type's own historical convention for it — even though
/// this crate's own client never sends one (it percent-encodes a space as
/// `%20`, which the fallthrough case below already handles).
fn parse_form_body(body: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8_lossy(body)
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| {
            (
                percent_decode(&key.replace('+', " ")),
                percent_decode(&value.replace('+', " ")),
            )
        })
        .collect()
}

/// Undo percent-encoding in one path segment.
///
/// A `%` that is not followed by two hex digits is kept as itself rather than
/// rejected: this is a test server, and a malformed escape should surface as
/// the 404 of an id that matches nothing, not as a parse error that hides
/// which id was asked for. Decoded octets that are not UTF-8 go through
/// [`String::from_utf8_lossy`] for the same reason — a `jmap_proto::Id` is a
/// `String`, so bytes that spell no string can only miss the lookup.
fn percent_decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match (bytes[index], bytes.get(index + 1), bytes.get(index + 2)) {
            (b'%', Some(high), Some(low)) => match (hex_value(*high), hex_value(*low)) {
                (Some(high), Some(low)) => {
                    decoded.push((high << 4) | low);
                    index += 3;
                }
                _ => {
                    decoded.push(b'%');
                    index += 1;
                }
            },
            _ => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// One hex digit's value, in either case, or `None` if it is not one.
fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
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

/// The capabilities every account here offers, unless a test asked for one to
/// be left out.
const ACCOUNT_CAPABILITIES: &[&str] = &[
    CAPABILITY_MAIL,
    CAPABILITY_SUBMISSION,
    CAPABILITY_CONTACTS,
    CAPABILITY_CALENDARS,
];

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
                account_capabilities: ACCOUNT_CAPABILITIES
                    .iter()
                    .filter(|capability| !state.omitted_capabilities.contains(**capability))
                    .map(|capability| ((*capability).to_owned(), json!({})))
                    .collect(),
                extra: Default::default(),
            },
        );
    }

    let first_account = state.accounts.iter().next();
    let primary_accounts = if state.omit_primary_accounts {
        std::collections::BTreeMap::new()
    } else {
        first_account
            .map(|(id, _)| {
                ACCOUNT_CAPABILITIES
                    .iter()
                    .filter(|capability| !state.omitted_capabilities.contains(**capability))
                    .map(|capability| ((*capability).to_owned(), id.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };

    // Built as an object rather than written out whole because one of its
    // properties may be absent: a server that names no `maxSizeUpload` is what
    // `no_size_upload` asks for, and a `null` would not be that server — it
    // would be one naming a limit of nothing.
    let mut core = json!({
        "maxConcurrentUpload": 4,
        "maxConcurrentRequests": 4,
        "maxObjectsInGet": state.objects_in_get(),
        "maxObjectsInSet": 128,
        "collationAlgorithms": ["i;ascii-casemap"],
    });
    if let Some(size_request) = state.size_request {
        core["maxSizeRequest"] = json!(size_request);
    }
    if let Some(size_upload) = state.size_upload {
        core["maxSizeUpload"] = json!(size_upload);
    }
    if let Some(calls_in_request) = state.calls_in_request {
        core["maxCallsInRequest"] = json!(calls_in_request);
    }

    Session {
        capabilities: [(CAPABILITY_CORE.to_owned(), core)]
            .into_iter()
            .chain(
                ACCOUNT_CAPABILITIES
                    .iter()
                    .filter(|capability| !state.omitted_capabilities.contains(**capability))
                    .map(|capability| ((*capability).to_owned(), json!({}))),
            )
            .collect(),
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

#[cfg(test)]
mod tests {
    use super::{path_segments, percent_decode};

    #[test]
    fn an_escaped_separator_stays_inside_its_segment() {
        assert_eq!(
            path_segments("/download/A1/b%2F1/name.bin"),
            ["download", "A1", "b/1", "name.bin"],
            "%2F is a character of the id, not a path separator"
        );
    }

    #[test]
    fn the_escapes_a_client_writes_come_back_off() {
        assert_eq!(percent_decode("b%231%3F2%2F3%204%255"), "b#1?2/3 4%5");
        assert_eq!(percent_decode("%C3%A4"), "ä");
        assert_eq!(percent_decode("%c3%a4"), "ä", "hex digits in either case");
    }

    #[test]
    fn a_segment_without_escapes_is_itself() {
        assert_eq!(percent_decode("B1"), "B1");
        assert_eq!(percent_decode(""), "");
    }

    #[test]
    fn a_malformed_escape_is_kept_rather_than_swallowed() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zz"), "a%zz");
        assert_eq!(percent_decode("%2"), "%2");
    }
}
