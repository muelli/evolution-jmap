// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP routes, session documents, and server lifecycle.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jmap_proto::blob::BlobCapability;
use jmap_proto::session::{
    Account, CAPABILITY_BLOB, CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE,
    CAPABILITY_CYRUS_MAIL, CAPABILITY_MAIL, CAPABILITY_PRINCIPALS, CAPABILITY_PRINCIPALS_OWNER,
    CAPABILITY_QUOTA, CAPABILITY_SIEVE, CAPABILITY_SUBMISSION, CAPABILITY_VACATION_RESPONSE,
    Session,
};
use jmap_proto::sieve::SieveCapability;
use serde_json::{Value, json};

use crate::auth::AuthConfig;
use crate::dispatch;
use crate::state::ServerState;

pub const DEFAULT_ACCOUNT_ID: &str = "A1";
pub const DEFAULT_ACCOUNT_NAME: &str = "alice@example.com";

/// Default for `maxObjectsInGet` in session output and request checks.
pub const DEFAULT_OBJECTS_IN_GET: u64 = 256;

/// Default for `maxSizeUpload` in session output and request checks.
pub const DEFAULT_SIZE_UPLOAD: u64 = 50_000_000;

/// Default for `maxCallsInRequest` in session output and request checks.
pub const DEFAULT_CALLS_IN_REQUEST: u64 = 16;

/// Default for `maxSizeRequest` in session output and request checks.
pub const DEFAULT_SIZE_REQUEST: u64 = 10_000_000;

/// Build RFC 8414 metadata from the server origin URL.
type MetadataFn = dyn Fn(&str) -> Value + Send + Sync;

/// Build an RFC 7591 registration response from the request JSON body.
type RegistrationFn = dyn Fn(&Value) -> (u16, Value) + Send + Sync;

/// Build an RFC 6749 token response from parsed form fields.
type TokenFn = dyn Fn(&BTreeMap<String, String>) -> (u16, Value) + Send + Sync;

/// Handlers for optional OAuth endpoints.
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
    changes_never_terminates: bool,
    stalwart_shaped_unrecognized_changes_state: bool,
    objects_in_get: Option<u64>,
    email_get_includes_unrequested: bool,
    contact_photo_uses_file_uri: bool,
    query_page_size: Option<u64>,
    query_never_terminates: bool,
    query_hides_limit: bool,
    size_request: Option<u64>,
    size_upload: Option<u64>,
    session_via_redirect: bool,
    well_known_redirect_to: Option<String>,
    advertise_origin: Option<String>,
    shared_account_name_copies_user: bool,
    download_via_redirect_to: Option<String>,
    reject_download_accept_json: bool,
    terse_submission_create: bool,
    terse_contact_create: bool,
    max_contact_size: Option<u64>,
    new_collections_default_unsubscribed: bool,
    terse_calendar_event_create: bool,
    without_scheduling: bool,
    terse_collection_create: bool,
    id_omitting_collection_create: bool,
    reject_collection_create: bool,
    max_delayed_send: Option<u64>,
    snooze_extension: bool,
}

impl MockServerBuilder {
    /// Allow HTTP Basic auth. Any configured credential makes auth mandatory.
    pub fn basic_auth(mut self, user: &str, password: &str) -> Self {
        self.auth.allow_basic(user, password);
        self
    }

    pub fn bearer_token(mut self, token: &str) -> Self {
        self.auth.allow_bearer(token);
        self
    }

    /// Accept a bearer token and treat the caller as `principal`.
    pub fn bearer_token_as(mut self, token: &str, principal: jmap_proto::Id) -> Self {
        self.auth.allow_bearer_as(token, principal);
        self
    }

    /// Omit one capability from `capabilities` and `primaryAccounts`.
    pub fn without_capability(mut self, capability: &str) -> Self {
        self.omitted_capabilities.insert(capability.to_owned());
        self
    }

    /// Omit `primaryAccounts` while keeping per-capability account lists.
    pub fn without_primary_accounts(mut self) -> Self {
        self.omit_primary_accounts = true;
        self
    }

    pub fn calls_in_request(mut self, calls: u64) -> Self {
        self.calls_in_request = Some(calls);
        self
    }

    /// Omit `maxCallsInRequest` and accept any call count.
    pub fn no_calls_in_request(mut self) -> Self {
        self.calls_in_request = None;
        self
    }

    /// Cap each `/changes` page to at most `ids` ids.
    pub fn changes_page_size(mut self, ids: u64) -> Self {
        self.changes_page_size = Some(ids);
        self
    }

    /// Keep paginated `/changes` responses non-terminating.
    pub fn changes_never_terminates(mut self) -> Self {
        self.changes_never_terminates = true;
        self
    }

    /// Return Stalwart style invalid `sinceState` errors on `/changes`.
    pub fn stalwart_shaped_unrecognized_changes_state(mut self) -> Self {
        self.stalwart_shaped_unrecognized_changes_state = true;
        self
    }

    pub fn objects_in_get(mut self, ids: u64) -> Self {
        self.objects_in_get = Some(ids);
        self
    }

    /// Include unrequested objects in `Email/get` responses.
    pub fn email_get_includes_unrequested(mut self) -> Self {
        self.email_get_includes_unrequested = true;
        self
    }

    /// Rewrite `ContactCard/get` photo media URIs to a local `file:` URI.
    pub fn contact_photo_uses_file_uri(mut self) -> Self {
        self.contact_photo_uses_file_uri = true;
        self
    }

    /// Cap each `Email/query`, `ContactCard/query` and `CalendarEvent/query` page to at most
    /// `ids` ids.
    pub fn query_page_size(mut self, ids: u64) -> Self {
        self.query_page_size = Some(ids);
        self
    }

    /// Keep paginated `Email/query` responses non-terminating.
    pub fn query_never_terminates(mut self) -> Self {
        self.query_never_terminates = true;
        self
    }

    /// Omit `limit` from a capped contact or event query page, against RFC 8620 section 5.5.
    pub fn query_hides_limit(mut self) -> Self {
        self.query_hides_limit = true;
        self
    }

    pub fn size_request(mut self, bytes: u64) -> Self {
        self.size_request = Some(bytes);
        self
    }

    /// Omit `maxSizeRequest` and accept any request size.
    pub fn no_size_request(mut self) -> Self {
        self.size_request = None;
        self
    }

    pub fn size_upload(mut self, bytes: u64) -> Self {
        self.size_upload = Some(bytes);
        self
    }

    /// Omit `maxSizeUpload` and accept any upload size.
    pub fn no_size_upload(mut self) -> Self {
        self.size_upload = None;
        self
    }

    /// Serve OAuth metadata on `/.well-known/oauth-authorization-server`.
    pub fn oauth_authorization_server(
        mut self,
        metadata: impl Fn(&str) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.oauth_metadata = Some(Arc::new(metadata));
        self
    }

    /// Serve `POST /oauth/register` using `handler`.
    pub fn oauth_client_registration(
        mut self,
        handler: impl Fn(&Value) -> (u16, Value) + Send + Sync + 'static,
    ) -> Self {
        self.oauth_registration = Some(Arc::new(handler));
        self
    }

    /// Serve `POST /oauth/token` using `handler`.
    pub fn oauth_token(
        mut self,
        handler: impl Fn(&BTreeMap<String, String>) -> (u16, Value) + Send + Sync + 'static,
    ) -> Self {
        self.oauth_token = Some(Arc::new(handler));
        self
    }

    /// Bind to a fixed localhost port instead of an ephemeral port.
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Serve discovery through a redirect flow.
    pub fn session_via_redirect(mut self) -> Self {
        self.session_via_redirect = true;
        self
    }

    /// Redirect `GET /.well-known/jmap` to `origin` with path `/jmap/session`.
    pub fn well_known_redirect_to(mut self, origin: impl Into<String>) -> Self {
        self.well_known_redirect_to = Some(origin.into());
        self
    }

    /// Advertise API URLs on `origin` instead of the listener address.
    pub fn advertise_origin(mut self, origin: impl Into<String>) -> Self {
        self.advertise_origin = Some(origin.into());
        self
    }

    /// Make shared-account session names copy the user account name.
    pub fn shared_account_name_copies_user(mut self) -> Self {
        self.shared_account_name_copies_user = true;
        self
    }

    /// Redirect `GET /download/...` to the same path on `origin`.
    pub fn download_via_redirect_to(mut self, origin: impl Into<String>) -> Self {
        self.download_via_redirect_to = Some(origin.into());
        self
    }

    /// Return `406` for download requests with `Accept: application/json`.
    pub fn reject_download_accept_json(mut self) -> Self {
        self.reject_download_accept_json = true;
        self
    }

    /// Return terse `EmailSubmission/set` creates without `identityId` or `emailId`.
    pub fn terse_submission_create(mut self) -> Self {
        self.terse_submission_create = true;
        self
    }

    /// Return terse `ContactCard/set` creates with only `id` in `created`.
    pub fn terse_contact_create(mut self) -> Self {
        self.terse_contact_create = true;
        self
    }

    /// Reject a `ContactCard/set` create larger than `bytes` JSON octets as
    /// `tooLarge`, like a server with a per-card size limit.
    pub fn max_contact_size(mut self, bytes: u64) -> Self {
        self.max_contact_size = Some(bytes);
        self
    }

    /// Default new `AddressBook` and `Calendar` objects to `isSubscribed: false`.
    pub fn new_collections_default_unsubscribed(mut self) -> Self {
        self.new_collections_default_unsubscribed = true;
        self
    }

    /// Return terse `CalendarEvent/set` creates with only `id` in `created`.
    pub fn terse_calendar_event_create(mut self) -> Self {
        self.terse_calendar_event_create = true;
        self
    }

    /// Simulate a server that sends no scheduling messages.
    /// Reject `CalendarEvent/set` with `sendSchedulingMessages` as
    /// `invalidArguments` (RFC 8620 section 3.9), and apply nothing.
    pub fn without_scheduling(mut self) -> Self {
        self.without_scheduling = true;
        self
    }

    /// Omit `name` from created `AddressBook` and `Calendar` responses.
    pub fn terse_collection_create(mut self) -> Self {
        self.terse_collection_create = true;
        self
    }

    /// Omit `id` from created collection responses.
    pub fn id_omitting_collection_create(mut self) -> Self {
        self.id_omitting_collection_create = true;
        self
    }

    /// Reject `AddressBook` and `Calendar` creates as `forbidden`.
    pub fn reject_collection_create(mut self) -> Self {
        self.reject_collection_create = true;
        self
    }

    /// Advertise FUTURERELEASE support and set `maxDelayedSend`.
    pub fn max_delayed_send(mut self, seconds: u64) -> Self {
        self.max_delayed_send = Some(seconds);
        self
    }

    /// Advertise the Cyrus mail extension and accept `Email.snoozed`.
    pub fn snooze_extension(mut self) -> Self {
        self.snooze_extension = true;
        self
    }

    /// Bind localhost and serve requests in a background thread.
    pub fn start(self) -> MockServer {
        let mut state = ServerState::new();
        state.add_account(DEFAULT_ACCOUNT_ID, DEFAULT_ACCOUNT_NAME);
        state.omitted_capabilities = self.omitted_capabilities.clone();
        state.omit_primary_accounts = self.omit_primary_accounts;
        state.calls_in_request = self.calls_in_request;
        state.changes_page_size = self.changes_page_size;
        state.changes_never_terminates = self.changes_never_terminates;
        state.stalwart_shaped_unrecognized_changes_state =
            self.stalwart_shaped_unrecognized_changes_state;
        state.objects_in_get = self.objects_in_get;
        state.email_get_includes_unrequested = self.email_get_includes_unrequested;
        state.contact_photo_uses_file_uri = self.contact_photo_uses_file_uri;
        state.query_page_size = self.query_page_size;
        state.query_never_terminates = self.query_never_terminates;
        state.query_hides_limit = self.query_hides_limit;
        state.size_request = self.size_request;
        state.size_upload = self.size_upload;
        state.session_via_redirect = self.session_via_redirect;
        state.well_known_redirect_to = self.well_known_redirect_to.clone();
        state.advertise_origin = self.advertise_origin.clone();
        state.shared_account_name_copies_user = self.shared_account_name_copies_user;
        state.download_via_redirect_to = self.download_via_redirect_to.clone();
        state.reject_download_accept_json = self.reject_download_accept_json;
        state.terse_submission_create = self.terse_submission_create;
        state.terse_contact_create = self.terse_contact_create;
        state.max_contact_size = self.max_contact_size;
        state.new_collections_default_unsubscribed = self.new_collections_default_unsubscribed;
        state.terse_calendar_event_create = self.terse_calendar_event_create;
        state.without_scheduling = self.without_scheduling;
        state.terse_collection_create = self.terse_collection_create;
        state.id_omitting_collection_create = self.id_omitting_collection_create;
        state.reject_collection_create = self.reject_collection_create;
        state.max_delayed_send = self.max_delayed_send;
        state.snooze_extension = self.snooze_extension;
        let state = Arc::new(Mutex::new(state));

        let server = tiny_http::Server::http(format!("127.0.0.1:{}", self.port))
            .expect("bind mock server to localhost");
        let port = server
            .server_addr()
            .to_ip()
            .expect("mock server has an IP address")
            .port();
        let origin = format!("http://127.0.0.1:{port}");

        let auth = Arc::new(Mutex::new(self.auth));

        let stop = Arc::new(AtomicBool::new(false));
        let oauth = OAuthHandlers {
            metadata: self.oauth_metadata.clone(),
            registration: self.oauth_registration.clone(),
            token: self.oauth_token.clone(),
        };
        let handle = std::thread::spawn({
            let state = Arc::clone(&state);
            let auth = Arc::clone(&auth);
            let stop = Arc::clone(&stop);
            let origin = origin.clone();
            move || serve(server, state, auth, oauth, origin, stop)
        });

        MockServer {
            origin,
            state,
            auth,
            stop,
            handle: Some(handle),
        }
    }
}

/// Run a mock server on an ephemeral localhost port.
pub struct MockServer {
    origin: String,
    state: Arc<Mutex<ServerState>>,
    auth: Arc<Mutex<AuthConfig>>,
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
            changes_never_terminates: false,
            stalwart_shaped_unrecognized_changes_state: false,
            objects_in_get: None,
            email_get_includes_unrequested: false,
            contact_photo_uses_file_uri: false,
            query_page_size: None,
            query_never_terminates: false,
            query_hides_limit: false,
            size_request: Some(DEFAULT_SIZE_REQUEST),
            size_upload: Some(DEFAULT_SIZE_UPLOAD),
            session_via_redirect: false,
            well_known_redirect_to: None,
            advertise_origin: None,
            shared_account_name_copies_user: false,
            download_via_redirect_to: None,
            reject_download_accept_json: false,
            terse_submission_create: false,
            terse_contact_create: false,
            max_contact_size: None,
            new_collections_default_unsubscribed: false,
            terse_calendar_event_create: false,
            without_scheduling: false,
            terse_collection_create: false,
            id_omitting_collection_create: false,
            reject_collection_create: false,
            max_delayed_send: None,
            snooze_extension: false,
        }
    }

    /// Origin URL (`http://127.0.0.1:<port>`) for `Client::connect`.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Default account id.
    pub fn account_id(&self) -> jmap_proto::Id {
        jmap_proto::Id::new(DEFAULT_ACCOUNT_ID)
    }

    /// Return shared state for fixture setup and white-box assertions.
    pub fn state(&self) -> Arc<Mutex<ServerState>> {
        Arc::clone(&self.state)
    }

    /// Replace accepted bearer tokens with only `token`.
    pub fn set_bearer_token(&self, token: &str) {
        self.auth
            .lock()
            .expect("mock auth lock")
            .replace_bearer(token);
    }

    /// Accept the current bearer token for `remaining` more requests.
    pub fn expire_bearer_token_after(&self, remaining: usize) {
        self.auth
            .lock()
            .expect("mock auth lock")
            .expire_after(remaining);
    }

    /// Return method call names in request order.
    pub fn method_calls(&self) -> Vec<String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.method_calls.clone()
    }

    /// Return the count from [`ServerState::api_requests`].
    pub fn api_requests(&self) -> usize {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.api_requests
    }

    /// Return the count from [`ServerState::unauthorized_responses`].
    pub fn unauthorized_responses(&self) -> usize {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.unauthorized_responses
    }

    /// Push `change` to every connected `/eventsource` client.
    pub fn push_state_change(&self, change: &jmap_proto::push::StateChange) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.event_source.broadcast(change);
    }

    /// Wait for one `/eventsource` subscriber or panic at timeout.
    pub fn wait_for_event_source_subscriber(&self, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let connected = {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.event_source.subscriber_count() > 0
            };
            if connected {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("no /eventsource subscriber connected within {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
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
    auth: Arc<Mutex<AuthConfig>>,
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
    auth: &Mutex<AuthConfig>,
    oauth: &OAuthHandlers,
    origin: &str,
) {
    // RFC 8414 metadata is public. Serve this route without auth.
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

    // Serve client registration without auth.
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

    // Serve token requests without auth for public client flows.
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

    // In redirect mode, serve both discovery routes before the 401 gate.
    let (session_via_redirect, well_known_redirect_to) = {
        let state = state.lock().expect("mock state lock");
        (
            state.session_via_redirect,
            state.well_known_redirect_to.clone(),
        )
    };
    if session_via_redirect || well_known_redirect_to.is_some() {
        let path_only = request.url().split('?').next().unwrap_or_default();
        if request.method() == &tiny_http::Method::Get && path_only == "/.well-known/jmap" {
            let target_origin = well_known_redirect_to.as_deref().unwrap_or(origin);
            let location = format!("{target_origin}/jmap/session");
            let response = tiny_http::Response::empty(307).with_header(
                tiny_http::Header::from_bytes(&b"Location"[..], location.as_bytes())
                    .expect("location header"),
            );
            let _ = request.respond(response);
            return;
        }
        if request.method() == &tiny_http::Method::Get && path_only == "/jmap/session" {
            let authorization = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("Authorization"))
                .map(|header| header.value.as_str().to_owned());
            let authorized = auth
                .lock()
                .expect("mock auth lock")
                .authorized(authorization.as_deref());
            let state = state.lock().expect("mock state lock");
            let session = session_document(&state, origin, authorized);
            respond_json(
                request,
                200,
                &serde_json::to_value(&session).expect("session serializes"),
            );
            return;
        }
    }

    let authorization = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .map(|header| header.value.as_str().to_owned());
    let mut auth_guard = auth.lock().expect("mock auth lock");
    let authorized = auth_guard.authorized(authorization.as_deref());
    let caller = auth_guard.identity_for(authorization.as_deref());
    drop(auth_guard);
    if !authorized {
        state
            .lock()
            .expect("mock state lock")
            .unauthorized_responses += 1;
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
            let session = session_document(&state, origin, true);
            respond_json(
                request,
                200,
                &serde_json::to_value(&session).expect("session serializes"),
            );
        }
        (tiny_http::Method::Get, "/eventsource") => {
            let query = url.split_once('?').map(|(_, query)| query).unwrap_or("");
            crate::eventsource::spawn_and_respond(request, query, state);
        }
        (tiny_http::Method::Post, "/jmap") => {
            let mut body = Vec::new();
            if request.as_reader().read_to_end(&mut body).is_err() {
                respond_json(request, 400, &json!({"detail": "unreadable body"}));
                return;
            }
            let mut state = state.lock().expect("mock state lock");
            let (status, response) = dispatch::handle_api(&mut state, &body, caller.as_ref());
            drop(state);
            respond_json(request, status, &response);
        }
        (tiny_http::Method::Post, _) if path.starts_with("/upload/") => {
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
            // RFC 8620 §6.1: oversize uploads return a request-level limit error.
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
            let (redirect_to, reject_accept_json) = {
                let state = state.lock().expect("mock state lock");
                (
                    state.download_via_redirect_to.clone(),
                    state.reject_download_accept_json,
                )
            };
            if reject_accept_json {
                let accept = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("Accept"))
                    .map(|header| header.value.as_str());
                if accept == Some("application/json") {
                    respond_json(
                        request,
                        406,
                        &json!({"status": 406, "detail": "blob download does not answer JSON"}),
                    );
                    return;
                }
            }
            if let Some(redirect_to) = redirect_to {
                let location = format!("{redirect_to}{url}");
                let response = tiny_http::Response::empty(302).with_header(
                    tiny_http::Header::from_bytes(&b"Location"[..], location.as_bytes())
                        .expect("location header"),
                );
                let _ = request.respond(response);
                return;
            }
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

/// Split a request path and decode each segment.
/// Keep `%2F` as data inside an id.
fn path_segments(path: &str) -> Vec<String> {
    path.trim_start_matches('/')
        .split('/')
        .map(percent_decode)
        .collect()
}

/// Parse an OAuth token body.
/// Treat `+` as space before percent decoding.
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

/// Decode percent escapes in one path segment.
fn percent_decode(segment: &str) -> String {
    percent_encoding::percent_decode_str(segment)
        .decode_utf8_lossy()
        .into_owned()
}

fn respond_json(request: tiny_http::Request, status: u16, body: &Value) {
    let payload = serde_json::to_string(body).expect("JSON body serializes");
    let response = tiny_http::Response::from_string(payload)
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("static header"),
        );
    // Ignore response write errors. The client can close first.
    let _ = request.respond(response);
}

/// List capabilities each account offers unless a test omits one.
const ACCOUNT_CAPABILITIES: &[&str] = &[
    CAPABILITY_MAIL,
    CAPABILITY_SUBMISSION,
    CAPABILITY_CONTACTS,
    CAPABILITY_CALENDARS,
    CAPABILITY_VACATION_RESPONSE,
    CAPABILITY_QUOTA,
    CAPABILITY_SIEVE,
    CAPABILITY_BLOB,
];

/// Build a capability object for `capability`.
/// Return `{}` for capabilities with no extra fields.
fn capability_value(capability: &str) -> Value {
    if capability == CAPABILITY_SIEVE {
        serde_json::to_value(SieveCapability::new(1_048_576).with_max_number_scripts(32))
            .expect("SieveCapability serializes")
    } else if capability == CAPABILITY_BLOB {
        serde_json::to_value(
            BlobCapability::new()
                .with_max_size_blob_set(DEFAULT_SIZE_UPLOAD)
                .with_max_data_sources(16)
                .with_supported_type_names(["Email", "Thread", "SieveScript"])
                .with_supported_digest_algorithms(["sha-256", "sha-512"]),
        )
        .expect("BlobCapability serializes")
    } else {
        json!({})
    }
}

/// Build the RFC 8620 section 2 session document from `state`.
fn session_document(state: &ServerState, origin: &str, authorized: bool) -> Session {
    let mut accounts = std::collections::BTreeMap::new();
    let owner_name = if authorized {
        state
            .accounts
            .iter()
            .find_map(|(_, account)| account.is_personal.then(|| account.name.clone()))
            .or_else(|| {
                state
                    .accounts
                    .iter()
                    .next()
                    .map(|(_, account)| account.name.clone())
            })
    } else {
        None
    };
    if authorized {
        for (id, account) in &state.accounts {
            let mut account_capabilities: std::collections::BTreeMap<String, Value> =
                ACCOUNT_CAPABILITIES
                    .iter()
                    .filter(|capability| !state.omitted_capabilities.contains(**capability))
                    .map(|capability| ((*capability).to_owned(), capability_value(capability)))
                    .collect();
            // Add `submission` account data only when `maxDelayedSend` is set.
            if let Some(seconds) = state.max_delayed_send
                && !state.omitted_capabilities.contains(CAPABILITY_SUBMISSION)
            {
                account_capabilities.insert(
                    CAPABILITY_SUBMISSION.to_owned(),
                    json!({
                        "maxDelayedSend": seconds,
                        "submissionExtensions": {"FUTURERELEASE": []},
                    }),
                );
            }
            if state.snooze_extension {
                account_capabilities.insert(CAPABILITY_CYRUS_MAIL.to_owned(), json!({}));
            }
            if !state.omitted_capabilities.contains(CAPABILITY_PRINCIPALS) {
                account_capabilities.insert(
                    CAPABILITY_PRINCIPALS.to_owned(),
                    json!({"currentUserPrincipalId": account.current_user_principal_id}),
                );
                if let Some(principal_id) = &account.current_user_principal_id {
                    account_capabilities.insert(
                        CAPABILITY_PRINCIPALS_OWNER.to_owned(),
                        json!({
                            "accountIdForPrincipal": id,
                            "principalId": principal_id,
                        }),
                    );
                }
            }
            accounts.insert(
                id.clone(),
                Account {
                    name: if state.shared_account_name_copies_user && !account.is_personal {
                        owner_name.clone().unwrap_or_else(|| account.name.clone())
                    } else {
                        account.name.clone()
                    },
                    is_personal: account.is_personal,
                    is_read_only: false,
                    account_capabilities,
                    extra: Default::default(),
                },
            );
        }
    }

    let first_account = if authorized {
        state.accounts.iter().next()
    } else {
        None
    };
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

    // Build `core`. Omit optional limits when unset.
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

    let advertised = state.advertise_origin.as_deref().unwrap_or(origin);

    let mut capabilities: std::collections::BTreeMap<String, Value> =
        [(CAPABILITY_CORE.to_owned(), core)]
            .into_iter()
            .chain(
                ACCOUNT_CAPABILITIES
                    .iter()
                    .chain([&CAPABILITY_PRINCIPALS, &CAPABILITY_PRINCIPALS_OWNER])
                    .filter(|capability| !state.omitted_capabilities.contains(**capability))
                    .map(|capability| ((*capability).to_owned(), capability_value(capability))),
            )
            .collect();
    if state.snooze_extension {
        capabilities.insert(CAPABILITY_CYRUS_MAIL.to_owned(), json!({}));
    }

    Session {
        capabilities,
        accounts,
        primary_accounts,
        username: first_account
            .map(|(_, account)| account.name.clone())
            .unwrap_or_default(),
        api_url: format!("{advertised}/jmap"),
        download_url: format!("{advertised}/download/{{accountId}}/{{blobId}}/{{name}}"),
        upload_url: format!("{advertised}/upload/{{accountId}}"),
        event_source_url: format!("{advertised}/eventsource"),
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
