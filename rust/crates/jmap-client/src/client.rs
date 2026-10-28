// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The blocking JMAP client.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use jmap_proto::Id;
use jmap_proto::error::{MethodError, RequestError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, SetRequest, SetResponse};
use jmap_proto::request::{Invocation, Request};
use jmap_proto::response::Response;
use jmap_proto::session::{self, Session};
use jmap_proto::state::State;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::Error;
use crate::limits::{self, IdBudget};
use crate::resolver::{NoSrvResolver, Resolver};
use crate::transport::{
    CancelFlag, HttpMethod, HttpRequest, HttpResponse, Transport, TransportError,
};

/// How the client authenticates (RFC 8620 leaves the scheme to HTTP).
#[derive(Clone)]
pub enum Credentials {
    None,
    Basic { user: String, password: String },
    Bearer(String),
}

/// Shows the user name but never the password or the token.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credentials::None => f.write_str("None"),
            Credentials::Basic { user, .. } => f
                .debug_struct("Basic")
                .field("user", user)
                .field("password", &format_args!("<redacted>"))
                .finish(),
            Credentials::Bearer(_) => f
                .debug_tuple("Bearer")
                .field(&format_args!("<redacted>"))
                .finish(),
        }
    }
}

impl Credentials {
    pub fn none() -> Self {
        Credentials::None
    }

    pub fn basic(user: impl Into<String>, password: impl Into<String>) -> Self {
        Credentials::Basic {
            user: user.into(),
            password: password.into(),
        }
    }

    pub fn bearer(token: impl Into<String>) -> Self {
        Credentials::Bearer(token.into())
    }

    fn authorization_header(&self) -> Option<String> {
        use base64::Engine as _;
        match self {
            Credentials::None => None,
            Credentials::Basic { user, password } => {
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
                Some(format!("Basic {encoded}"))
            }
            Credentials::Bearer(token) => Some(format!("Bearer {token}")),
        }
    }
}

/// Read `JMAP_LIVE_SERVER_REBASE_URLS` to configure URL rebasing.
///
/// Returns true when the environment variable is set to `1` or `true`.
pub fn rebase_urls_from_env() -> bool {
    std::env::var("JMAP_LIVE_SERVER_REBASE_URLS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub struct ClientBuilder {
    timeout: Duration,
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    transport: Option<Box<dyn Transport>>,
    cancel: Option<CancelFlag>,
    rebase_urls_to_origin: bool,
    resolver: Box<dyn Resolver>,
    max_rate_limit_retries: u32,
    max_retry_after: Duration,
    id_budget: IdBudget,
}

impl std::fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("timeout", &self.timeout)
            .field("connect_timeout", &self.connect_timeout)
            .field("read_timeout", &self.read_timeout)
            .field("rebase_urls_to_origin", &self.rebase_urls_to_origin)
            .field("max_rate_limit_retries", &self.max_rate_limit_retries)
            .field("max_retry_after", &self.max_retry_after)
            .field("id_budget", &self.id_budget)
            .finish_non_exhaustive()
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            connect_timeout: None,
            read_timeout: None,
            transport: None,
            cancel: None,
            rebase_urls_to_origin: rebase_urls_from_env(),
            resolver: Box::new(NoSrvResolver),
            max_rate_limit_retries: 3,
            max_retry_after: Duration::from_secs(30),
            id_budget: IdBudget::per_sync(),
        }
    }
}

impl ClientBuilder {
    /// Set the connect timeout for establishing HTTP connections.
    /// Defaults to the total timeout if not explicitly set.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }

    /// Set the read timeout for receiving HTTP response headers and bodies.
    /// Defaults to the total timeout if not explicitly set.
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = Some(timeout);
        self
    }

    /// Set the total end-to-end timeout for HTTP operations.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn transport(mut self, transport: impl Transport) -> Self {
        self.transport = Some(Box::new(transport));
        self
    }

    /// Cancelling this flag aborts in-flight and future operations with
    /// [`Error::Cancelled`].
    pub fn cancel_flag(mut self, cancel: CancelFlag) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Rebase session URLs to the connected origin.
    ///
    /// Rewrites scheme and authority for `apiUrl`, `downloadUrl`, `uploadUrl`,
    /// and `eventSourceUrl`. Preserves path and query parameters.
    ///
    /// Disabled by default. Enable when proxy or NAT boundaries prevent
    /// direct access to advertised endpoints.
    pub fn rebase_urls_to_origin(mut self, rebase: bool) -> Self {
        self.rebase_urls_to_origin = rebase;
        self
    }

    /// Set the resolver for DNS SRV lookups (RFC 8620 §2.2).
    ///
    /// Defaults to [`NoSrvResolver`].
    pub fn resolver(mut self, resolver: impl Resolver + 'static) -> Self {
        self.resolver = Box::new(resolver);
        self
    }

    /// Set the maximum number of attempts for idempotent operations on rate limits (HTTP 429).
    ///
    /// Defaults to 3 attempts. Setting to 1 or 0 disables automatic retries.
    pub fn max_rate_limit_retries(mut self, retries: u32) -> Self {
        self.max_rate_limit_retries = retries;
        self
    }

    /// Set the maximum backoff delay for rate limit retries.
    ///
    /// Defaults to 30 seconds.
    pub fn max_retry_after(mut self, max: Duration) -> Self {
        self.max_retry_after = max;
        self
    }

    /// Set the id budget for one sync, for example one `all_changes` call.
    ///
    /// Defaults to [`IdBudget::per_sync`].
    pub fn id_budget(mut self, budget: IdBudget) -> Self {
        self.id_budget = budget;
        self
    }

    /// Fetch the session object from `origin` (scheme + host + port) and
    /// return a ready client.
    pub fn connect(self, origin: &str, credentials: Credentials) -> Result<Client, Error> {
        let transport = match self.transport {
            Some(transport) => transport,
            None => {
                #[cfg(feature = "transport-ureq")]
                {
                    let connect = self.connect_timeout.unwrap_or(self.timeout);
                    let read = self.read_timeout.unwrap_or(self.timeout);
                    Box::new(crate::transport::UreqTransport::with_timeouts(
                        connect,
                        read,
                        self.timeout,
                    ))
                }
                #[cfg(not(feature = "transport-ureq"))]
                {
                    return Err(Error::Protocol(
                        "no transport configured and transport-ureq feature disabled".into(),
                    ));
                }
            }
        };

        let origin = origin.trim_end_matches('/').to_string();
        let session_url = format!("{origin}/.well-known/jmap");
        let mut client = Client {
            transport,
            authorization: RwLock::new(credentials.authorization_header()),
            cancel: self.cancel,
            session_url,
            session: None,
            next_call_id: AtomicU64::new(0),
            rebase_origin: self.rebase_urls_to_origin.then_some(origin),
            rebase_note: None,
            max_rate_limit_retries: self.max_rate_limit_retries,
            max_retry_after: self.max_retry_after,
            id_budget: self.id_budget,
        };
        client.refresh_session()?;
        Ok(client)
    }

    /// Connect to a domain using DNS SRV resolution (RFC 8620 §2.2).
    ///
    /// Queries `_jmap._tcp` records and falls back to `https://<domain>`.
    pub fn connect_domain(self, domain: &str, credentials: Credentials) -> Result<Client, Error> {
        let origin = match self.resolver.lookup_srv(domain) {
            Some(target) if target.port == 443 => format!("https://{}", target.host),
            Some(target) => format!("https://{}:{}", target.host, target.port),
            None => format!("https://{domain}"),
        };
        self.connect(&origin, credentials)
    }
}

pub struct Client {
    transport: Box<dyn Transport>,
    // Synchronizes credential updates across shared client instances.
    authorization: RwLock<Option<String>>,
    cancel: Option<CancelFlag>,
    session_url: String,
    session: Option<Session>,
    next_call_id: AtomicU64,
    rebase_origin: Option<String>,
    // Records download URL origin rewrites.
    rebase_note: Option<String>,
    pub(crate) max_rate_limit_retries: u32,
    pub(crate) max_retry_after: Duration,
    id_budget: IdBudget,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let authenticated = self
            .authorization
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        f.debug_struct("Client")
            .field("session_url", &self.session_url)
            .field("authenticated", &authenticated)
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// A fresh id budget for one sync. Each sync charges its own copy.
    pub fn id_budget(&self) -> IdBudget {
        self.id_budget
    }

    /// Connect with default transport and timeout.
    ///
    /// Checks `JMAP_LIVE_SERVER_REBASE_URLS` to enable URL rebasing.
    pub fn connect(origin: &str, credentials: Credentials) -> Result<Self, Error> {
        Self::builder()
            .rebase_urls_to_origin(rebase_urls_from_env())
            .connect(origin, credentials)
    }

    /// The session object fetched at connect time.
    pub fn session(&self) -> &Session {
        self.session
            .as_ref()
            .expect("session is fetched during connect")
    }

    /// Refresh the session resource (RFC 8620 §2).
    pub fn refresh_session(&mut self) -> Result<(), Error> {
        let response = self.execute_idempotent_within(
            HttpMethod::Get,
            &self.session_url.clone(),
            None,
            None,
            "application/json",
            limits::MAX_API_RESPONSE_BYTES,
        )?;
        // The transport did not send the credentials to another origin, so its session must not direct them.
        if !crate::url::same_origin(&self.session_url, &response.final_url) {
            return Err(Error::RedirectCredentialsWithheld {
                requested: crate::url::origin_of(&self.session_url).into_owned(),
                followed: crate::url::origin_of(&response.final_url).into_owned(),
            });
        }
        let mut session: Session = serde_json::from_slice(&response.body)?;
        if let Some(origin) = &self.rebase_origin {
            let advertised_download_origin =
                crate::url::origin_of(&session.download_url).into_owned();
            session.api_url = crate::url::rebase_origin(&session.api_url, origin);
            session.download_url = crate::url::rebase_origin(&session.download_url, origin);
            session.upload_url = crate::url::rebase_origin(&session.upload_url, origin);
            session.event_source_url = crate::url::rebase_origin(&session.event_source_url, origin);
            self.rebase_note = if crate::url::same_origin(&advertised_download_origin, origin) {
                None
            } else {
                let note = format!(
                    "note: JMAP_LIVE_SERVER_REBASE_URLS is active and rewrote \
                     {advertised_download_origin} to {origin}"
                );
                eprintln!("{note}");
                Some(note)
            };
        }
        for (name, endpoint) in [
            ("apiUrl", &session.api_url),
            ("downloadUrl", &session.download_url),
            ("uploadUrl", &session.upload_url),
            ("eventSourceUrl", &session.event_source_url),
        ] {
            if !crate::url::may_carry_credentials(&self.session_url, endpoint) {
                return Err(Error::Protocol(format!(
                    "the session names {name} {endpoint} without TLS; the client sends \
                     credentials only over TLS, to loopback, or to the configured origin"
                )));
            }
        }
        self.session = Some(session);
        Ok(())
    }

    /// Return the notification message when URL rebasing modified the origin.
    pub fn rebase_note(&self) -> Option<&str> {
        self.rebase_note.as_deref()
    }

    /// Update client credentials for subsequent requests.
    ///
    /// Updates the authorization header without rebuilding the transport.
    /// Use this method to refresh expired OAuth 2.0 tokens.
    pub fn set_credentials(&self, credentials: Credentials) {
        *self
            .authorization
            .write()
            .unwrap_or_else(PoisonError::into_inner) = credentials.authorization_header();
    }

    /// Return the current `Authorization` header value.
    ///
    /// Returns `None` for unauthenticated sessions.
    pub fn authorization_header(&self) -> Option<String> {
        self.authorization
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Check if the session document is anonymous.
    ///
    /// Returns true when the username and account list are empty (RFC 8620 §2 and §8.1).
    pub fn is_anonymous(&self) -> bool {
        let session = self.session();
        session.username.is_empty() && session.accounts.is_empty()
    }

    /// Return the primary account ID for a capability (RFC 8620 §2).
    pub fn primary_account(&self, capability: &str) -> Result<jmap_proto::Id, Error> {
        self.session()
            .resolve_primary_account(capability)
            .cloned()
            .ok_or_else(|| Error::Protocol(format!("no primary account for {capability}")))
    }

    /// Generate a fresh call ID, unique within this client instance.
    pub fn next_call_id(&self) -> String {
        format!("c{}", self.next_call_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Return true if the method name represents an idempotent operation.
    pub fn is_method_idempotent(method: &str) -> bool {
        let name = method.rsplit_once('/').map(|(_, m)| m).unwrap_or(method);
        matches!(
            name,
            "get" | "query" | "changes" | "queryChanges" | "echo" | "getAvailability"
        )
    }

    /// Return true if every method call in the request is idempotent.
    pub fn is_request_idempotent(request: &Request) -> bool {
        !request.method_calls.is_empty()
            && request
                .method_calls
                .iter()
                .all(|call| Self::is_method_idempotent(&call.name))
    }

    /// Compute the backoff delay for an attempt, capped to `max_retry_after`.
    pub(crate) fn compute_backoff(&self, retry_after: Option<Duration>, attempt: u32) -> Duration {
        if let Some(duration) = retry_after {
            return duration.min(self.max_retry_after);
        }
        let factor = 1u64.checked_shl(attempt.saturating_sub(1)).unwrap_or(8);
        let millis = 500 * factor;
        Duration::from_millis(millis).min(self.max_retry_after)
    }

    /// Sleep for `duration` in small intervals while checking the cancellation flag.
    pub(crate) fn sleep_with_cancel(&self, duration: Duration) -> Result<(), Error> {
        let cancel = self.cancel_for_request();
        if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
            return Err(Error::Cancelled);
        }
        if duration.is_zero() {
            return Ok(());
        }
        let start = std::time::Instant::now();
        while start.elapsed() < duration {
            if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
                return Err(Error::Cancelled);
            }
            let remaining = duration.saturating_sub(start.elapsed());
            let chunk = remaining.min(Duration::from_millis(20));
            std::thread::sleep(chunk);
        }
        if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    /// Execute an idempotent HTTP request with automatic rate limit backoff and retry.
    pub(crate) fn execute_idempotent_within(
        &self,
        method: HttpMethod,
        url: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
        accept: &str,
        max_response_bytes: u64,
    ) -> Result<HttpResponse, Error> {
        let max_attempts = self.max_rate_limit_retries.max(1);
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.execute_within(method, url, body, content_type, accept, max_response_bytes) {
                Ok(response) => return Ok(response),
                Err(Error::Http {
                    status: 429,
                    problem,
                }) => {
                    let retry_after = crate::transport::observed_retry_after()
                        .or_else(|| problem.as_ref().and_then(extract_problem_retry_after));
                    if attempt < max_attempts {
                        let backoff = self.compute_backoff(retry_after, attempt);
                        tracing::debug!(attempt, ?backoff, "backing off after rate limit");
                        self.sleep_with_cancel(backoff)?;
                        continue;
                    }
                    return Err(Error::RateLimited {
                        retry_after,
                        method: None,
                        problem,
                    });
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Send a request envelope to the API endpoint (RFC 8620 §3).
    ///
    /// Rejects requests that exceed `maxSizeRequest` with [`Error::RequestTooLarge`].
    /// Automatically retries idempotent calls on HTTP 429 and limit errors.
    pub fn api_call(&self, request: &Request) -> Result<Response, Error> {
        let api_url = self.session().api_url.clone();
        let body = serde_json::to_vec(request)?;
        if let Some(limit) = self.session().max_size_request()
            && body.len() as u64 > limit
        {
            return Err(Error::RequestTooLarge {
                size: body.len() as u64,
                limit,
            });
        }
        tracing::trace!(body = %String::from_utf8_lossy(&body), "JMAP request body");

        let is_idempotent = Self::is_request_idempotent(request);
        let max_attempts = if is_idempotent {
            self.max_rate_limit_retries.max(1)
        } else {
            1
        };

        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.execute(HttpMethod::Post, &api_url, Some(&body)) {
                Ok(response) => {
                    tracing::trace!(body = %String::from_utf8_lossy(&response.body), "JMAP response body");
                    return Ok(serde_json::from_slice(&response.body)?);
                }
                Err(Error::Http {
                    status: 429,
                    problem,
                }) => {
                    let retry_after = crate::transport::observed_retry_after()
                        .or_else(|| problem.as_ref().and_then(extract_problem_retry_after));

                    if is_idempotent && attempt < max_attempts {
                        let backoff = self.compute_backoff(retry_after, attempt);
                        tracing::debug!(attempt, ?backoff, "backing off after rate limit error");
                        self.sleep_with_cancel(backoff)?;
                        continue;
                    }

                    let non_idempotent_method = request
                        .method_calls
                        .iter()
                        .find(|c| !Self::is_method_idempotent(&c.name))
                        .map(|c| c.name.clone());

                    return Err(Error::RateLimited {
                        retry_after,
                        method: non_idempotent_method,
                        problem,
                    });
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Execute a single method call and return the response arguments.
    pub fn single_call(
        &self,
        using: &[&str],
        method: &str,
        arguments: &impl serde::Serialize,
    ) -> Result<Value, Error> {
        self.single_call_with_created_ids(using, method, arguments, &BTreeMap::new())
    }

    /// Send one method call with the request's initial `createdIds` map (RFC 8620 §3.3).
    ///
    /// The server resolves `#creationId` references from the map. An empty map is not sent.
    pub(crate) fn single_call_with_created_ids(
        &self,
        using: &[&str],
        method: &str,
        arguments: &impl serde::Serialize,
        created_ids: &BTreeMap<Id, Id>,
    ) -> Result<Value, Error> {
        let call_id = self.next_call_id();
        tracing::debug!(method, call_id, "sending JMAP method call");
        let result = self.single_call_inner(using, method, arguments, created_ids, &call_id);
        if let Err(error) = &result {
            tracing::warn!(method, call_id, %error, "JMAP method call failed");
        }
        result
    }

    fn single_call_inner(
        &self,
        using: &[&str],
        method: &str,
        arguments: &impl serde::Serialize,
        created_ids: &BTreeMap<Id, Id>,
        call_id: &str,
    ) -> Result<Value, Error> {
        let is_idempotent = Self::is_method_idempotent(method);
        let max_attempts = if is_idempotent {
            self.max_rate_limit_retries.max(1)
        } else {
            1
        };

        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut request =
                Request::new(using.iter().copied()).call(method, arguments, call_id)?;
            if !created_ids.is_empty() {
                request = request.with_created_ids(created_ids.clone());
            }
            let response = self.api_call(&request)?;
            let invocation = response
                .responses_for(call_id)
                .next()
                .ok_or_else(|| Error::Protocol(format!("no response for call id {call_id}")))?;

            if invocation.is_error() {
                let method_error: MethodError = invocation.parse()?;
                if method_error.error_type == "limit" {
                    let retry_after = crate::transport::observed_retry_after()
                        .or_else(|| extract_method_retry_after(&method_error));

                    if is_idempotent && attempt < max_attempts {
                        let backoff = self.compute_backoff(retry_after, attempt);
                        tracing::debug!(attempt, ?backoff, "backing off after method limit error");
                        self.sleep_with_cancel(backoff)?;
                        continue;
                    }

                    return Err(Error::RateLimited {
                        retry_after,
                        method: Some(method.to_string()),
                        problem: None,
                    });
                }
                return Err(Error::Method(method_error));
            }

            if invocation.name != method {
                return Err(Error::Protocol(format!(
                    "expected response to {method}, got {}",
                    invocation.name
                )));
            }
            return Ok(invocation.arguments.clone());
        }
    }

    /// Check if the server permits `calls` method calls in one request.
    ///
    /// Verifies compliance with `maxCallsInRequest` (RFC 8620 §2).
    pub fn takes_calls_in_one_request(&self, calls: u64) -> bool {
        self.session()
            .max_calls_in_request()
            .is_none_or(|limit| calls <= limit)
    }

    /// Execute a `/set` method call, splitting requests across `maxObjectsInSet`.
    pub(crate) fn set_chunked<T>(
        &self,
        using: &[&str],
        method: &str,
        request: &SetRequest<T>,
    ) -> Result<SetResponse<T>, Error>
    where
        T: Clone + serde::Serialize + serde::de::DeserializeOwned,
    {
        let limit = self
            .session()
            .max_objects_in_set()
            .and_then(|l| usize::try_from(l).ok());

        crate::set::execute_set_chunked(request, limit, |chunk_req, created_ids| {
            let arguments =
                self.single_call_with_created_ids(using, method, chunk_req, created_ids)?;
            Ok(serde_json::from_value(arguments)?)
        })
    }

    /// Remove each object whose id is not in `requested` (RFC 8620 §5.1).
    ///
    /// A `/get` answer must hold only the requested objects. Call this for
    /// each chunk, so that extra objects do not stay in memory. The first
    /// object with no id stays, so that the caller can report the violation.
    pub(crate) fn keep_requested<T>(
        list: &mut Vec<T>,
        requested: &[Id],
        id: impl Fn(&T) -> Option<&Id>,
    ) {
        let requested: std::collections::HashSet<&Id> = requested.iter().collect();
        let mut kept_one_without_id = false;
        list.retain(|object| match id(object) {
            Some(id) => requested.contains(id),
            None => !std::mem::replace(&mut kept_one_without_id, true),
        });
    }

    /// Calculate how many identifiers fit into the request size limit.
    ///
    /// Computes the exact byte length of the serialized request envelope.
    /// Returns [`Error::RequestTooLarge`] when even the first identifier does not fit.
    pub(crate) fn ids_that_fit(
        &self,
        using: &[&str],
        method: &str,
        empty_req: &impl Serialize,
        ids: &[Id],
        call_id: &str,
        limit: u64,
    ) -> Result<usize, Error> {
        let empty = Request::new(using.iter().copied()).call(method, empty_req, call_id)?;
        let mut used = serde_json::to_vec(&empty)?.len() as u64;
        if used > limit {
            return Err(Error::RequestTooLarge { size: used, limit });
        }

        for (taken, id) in ids.iter().enumerate() {
            let cost = serde_json::to_string(id)?.len() as u64 + u64::from(taken > 0);
            if used + cost > limit {
                if taken == 0 {
                    return Err(Error::RequestTooLarge {
                        size: used + cost,
                        limit,
                    });
                }
                return Ok(taken);
            }
            used += cost;
        }
        Ok(ids.len())
    }

    /// Execute a chunked `/get` method call with custom request and response shapes.
    ///
    /// Splits requests when identifiers exceed `maxSizeRequest` or `maxObjectsInGet`.
    /// Executes at least one call when `ids` is empty to fetch current state.
    pub(crate) fn chunked_get_custom<Req: Serialize, Resp: DeserializeOwned, Item>(
        &self,
        using: &[&str],
        method: &str,
        ids: &[Id],
        make_req: impl Fn(&[Id]) -> Req,
        extract: impl Fn(Resp) -> (Vec<Item>, Vec<Id>, Option<State>, Id),
        id_of: impl Fn(&Item) -> Option<&Id>,
    ) -> Result<GetResponse<Item>, Error> {
        let empty_req = make_req(&[]);
        let limit = self.session().max_size_request();
        let max_objects = self
            .session()
            .max_objects_in_get()
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|&limit| limit > 0);

        let mut combined: Option<GetResponse<Item>> = None;
        let mut rest = ids;
        loop {
            let call_id = format!("c{}", self.next_call_id.load(Ordering::Relaxed));
            let mut take = match limit {
                Some(limit) => {
                    self.ids_that_fit(using, method, &empty_req, rest, &call_id, limit)?
                }
                None => rest.len(),
            };
            if let Some(max_objects) = max_objects {
                take = take.min(max_objects);
            }
            let (chunk, remaining) = rest.split_at(take);
            let request = make_req(chunk);
            let arguments = self.single_call(using, method, &request)?;
            let response: Resp = serde_json::from_value(arguments)?;
            let (mut list, mut not_found, state, account_id) = extract(response);
            Self::keep_requested(&mut list, chunk, &id_of);
            Self::keep_requested(&mut not_found, chunk, |id| Some(id));

            match &mut combined {
                None => {
                    combined = Some(
                        GetResponse::new(account_id, state.unwrap_or_default(), list)
                            .with_not_found(not_found),
                    );
                }
                Some(combined) => {
                    combined.list.extend(list);
                    combined.not_found.extend(not_found);
                    if let Some(state) = state {
                        combined.state = state;
                    }
                }
            }

            rest = remaining;
            if rest.is_empty() {
                return Ok(combined.expect("the loop always sets combined on its first iteration"));
            }
        }
    }

    /// Execute a standard `Foo/get` method call in chunks.
    ///
    /// Splits requests into chunks when identifiers exceed `maxSizeRequest` or `maxObjectsInGet`.
    /// When `ids` is empty, executes at least one call to read the current state.
    pub fn get_chunked<T: DeserializeOwned + Default>(
        &self,
        using: &[&str],
        method: &str,
        account_id: &Id,
        ids: &[Id],
        properties: Option<&[&str]>,
        id_of: impl Fn(&T) -> Option<&Id>,
    ) -> Result<GetResponse<T>, Error> {
        self.chunked_get_custom(
            using,
            method,
            ids,
            |chunk| Self::get_request(account_id, chunk, properties),
            |resp: GetResponse<T>| (resp.list, resp.not_found, Some(resp.state), resp.account_id),
            id_of,
        )
    }

    /// Build a standard `GetRequest` for `account_id`, `ids`, and optional `properties`.
    fn get_request(account_id: &Id, ids: &[Id], properties: Option<&[&str]>) -> GetRequest {
        let mut request = GetRequest::ids(account_id.clone(), ids.iter().cloned());
        request.properties = properties.map(|props| props.iter().map(|s| s.to_string()).collect());
        request
    }

    /// Extract arguments from an invocation, mapping `error` responses to
    /// [`Error::Method`].
    pub fn unwrap_invocation(invocation: &Invocation, method: &str) -> Result<Value, Error> {
        if invocation.is_error() {
            return Err(Error::Method(invocation.parse()?));
        }
        if invocation.name != method {
            return Err(Error::Protocol(format!(
                "expected response to {method}, got {}",
                invocation.name
            )));
        }
        Ok(invocation.arguments.clone())
    }

    /// `Core/echo` (RFC 8620 §4).
    pub fn echo(&self, value: Value) -> Result<Value, Error> {
        self.single_call(&[session::CAPABILITY_CORE], "Core/echo", &value)
    }

    /// Return the active cancellation flag for the current request.
    ///
    /// Prioritizes thread-local cancel scopes over the client-wide flag.
    fn cancel_for_request(&self) -> Option<CancelFlag> {
        crate::transport::observed().or_else(|| self.cancel.clone())
    }

    /// Execute an HTTP request expecting a JSON response within size limits.
    pub(crate) fn execute(
        &self,
        method: HttpMethod,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, Error> {
        self.execute_with_content_type(method, url, body, body.map(|_| "application/json"))
    }

    /// Execute an HTTP request with explicit content type expecting JSON.
    pub(crate) fn execute_with_content_type(
        &self,
        method: HttpMethod,
        url: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Result<HttpResponse, Error> {
        self.execute_within(
            method,
            url,
            body,
            content_type,
            "application/json",
            limits::MAX_API_RESPONSE_BYTES,
        )
    }

    /// Execute an HTTP request with explicit Accept header and response size limits.
    pub(crate) fn execute_within(
        &self,
        method: HttpMethod,
        url: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
        accept: &str,
        max_response_bytes: u64,
    ) -> Result<HttpResponse, Error> {
        let cancel = self.cancel_for_request();
        if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
            return Err(Error::Cancelled);
        }

        let mut headers: Vec<(String, String)> = vec![("Accept".to_owned(), accept.to_owned())];
        if let Some(content_type) = content_type {
            headers.push(("Content-Type".to_owned(), content_type.to_owned()));
        }
        if let Some(authorization) = &*self
            .authorization
            .read()
            .unwrap_or_else(PoisonError::into_inner)
        {
            headers.push(("Authorization".to_owned(), authorization.clone()));
        }

        crate::transport::set_observed_retry_after(None);

        let response = self
            .transport
            .execute(HttpRequest {
                method,
                url,
                headers: &headers,
                body,
                cancel: cancel.as_ref(),
                max_response_bytes,
            })
            .map_err(|error| match error {
                TransportError::Cancelled => Error::Cancelled,
                TransportError::Failed(message) => Error::Transport(message),
                TransportError::ResponseTooLarge { limit } => Error::ResponseTooLarge { limit },
            })?;

        if response.status == 401
            && headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            && !crate::url::same_origin(url, &response.final_url)
        {
            return Err(Error::RedirectCredentialsWithheld {
                requested: crate::url::origin_of(url).into_owned(),
                followed: crate::url::origin_of(&response.final_url).into_owned(),
            });
        }

        if !(200..300).contains(&response.status) {
            let problem: Option<RequestError> = serde_json::from_slice(&response.body).ok();
            return Err(Error::Http {
                status: response.status,
                problem,
            });
        }
        Ok(response)
    }

    /// Open an EventSource push stream for the specified data types.
    ///
    /// Expands the session's `eventSourceUrl` URI template, attaches the current
    /// authorization header, and connects with default timeouts and a fresh
    /// cancellation token.
    pub fn event_source(
        &self,
        types: &[&str],
    ) -> Result<crate::eventsource::EventSourceSubscription, Error> {
        self.event_source_with_timeouts(types, crate::eventsource::EventSourceTimeouts::default())
    }

    /// Open an EventSource push stream for the specified data types with explicit timeouts.
    pub fn event_source_with_timeouts(
        &self,
        types: &[&str],
        timeouts: crate::eventsource::EventSourceTimeouts,
    ) -> Result<crate::eventsource::EventSourceSubscription, Error> {
        let template = self.session().event_source_url.trim();
        if template.is_empty() {
            return Err(Error::Protocol(
                "session does not advertise an eventSourceUrl".into(),
            ));
        }
        let url = crate::eventsource::expand_url(template, types, false, 0);
        let headers = self
            .authorization_header()
            .map(|auth| vec![("Authorization".to_string(), auth)])
            .unwrap_or_default();
        let cancel = crate::transport::CancelFlag::default();
        Ok(
            crate::eventsource::EventSourceSubscription::start_with_timeouts(
                url,
                crate::eventsource::SharedHeaders::new(headers),
                cancel,
                timeouts,
            ),
        )
    }
}

/// Extract the `Retry-After` duration from a problem error when present.
pub(crate) fn extract_problem_retry_after(problem: &RequestError) -> Option<Duration> {
    problem
        .extra
        .get("retryAfter")
        .or_else(|| problem.extra.get("retry_after"))
        .and_then(|val| {
            val.as_u64()
                .or_else(|| val.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .map(Duration::from_secs)
}

/// Extract the `Retry-After` duration from a method error when present.
pub(crate) fn extract_method_retry_after(method_error: &MethodError) -> Option<Duration> {
    method_error
        .extra
        .get("retryAfter")
        .or_else(|| method_error.extra.get("retry_after"))
        .and_then(|val| {
            val.as_u64()
                .or_else(|| val.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .map(Duration::from_secs)
}

/// Extract the `Retry-After` duration from a set error when present.
pub(crate) fn extract_set_error_retry_after(set_error: &SetError) -> Option<Duration> {
    set_error
        .extra
        .get("retryAfter")
        .or_else(|| set_error.extra.get("retry_after"))
        .and_then(|val| {
            val.as_u64()
                .or_else(|| val.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::*;

    /// Verify that the builder stores the configured timeout.
    ///
    /// The field is private. External callers cannot inspect the stored value directly.
    #[test]
    fn timeout_replaces_the_default() {
        let builder = ClientBuilder::default().timeout(Duration::from_secs(5));
        assert_eq!(builder.timeout, Duration::from_secs(5));
    }

    /// Collects emitted tracing events for test assertions.
    ///
    /// Stores the level and structured fields of every event.
    struct CapturingSubscriber {
        captured: Arc<Mutex<Vec<(Level, String, String)>>>,
    }

    struct Recorder<'a> {
        level: Level,
        sink: &'a Mutex<Vec<(Level, String, String)>>,
    }

    impl Visit for Recorder<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.sink.lock().unwrap().push((
                self.level,
                field.name().to_owned(),
                format!("{value:?}"),
            ));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.sink
                .lock()
                .unwrap()
                .push((self.level, field.name().to_owned(), value.to_owned()));
        }
    }

    impl Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            event.record(&mut Recorder {
                level: *event.metadata().level(),
                sink: &self.captured,
            });
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[test]
    fn single_call_traces_the_method_and_call_id_on_success() {
        let server = jmap_mock::MockServer::builder().start();
        let client = Client::connect(server.origin(), Credentials::none())
            .expect("mock server should accept an anonymous connection");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            captured: captured.clone(),
        };

        tracing::subscriber::with_default(subscriber, || {
            client
                .echo(serde_json::json!({}))
                .expect("Core/echo should succeed");
        });

        let captured = captured.lock().unwrap();
        assert!(
            captured
                .iter()
                .any(|(level, name, value)| *level == Level::DEBUG
                    && name == "method"
                    && value == "Core/echo"),
            "expected a DEBUG method=Core/echo field, got {captured:?}"
        );
        assert!(
            captured
                .iter()
                .any(|(level, name, _)| *level == Level::DEBUG && name == "call_id"),
            "expected a DEBUG call_id field, got {captured:?}"
        );
        assert!(
            captured.iter().all(|(_, name, _)| name != "error"),
            "a successful call should not log an error field, got {captured:?}"
        );
    }

    #[test]
    fn single_call_traces_the_failure_when_the_method_errors() {
        let server = jmap_mock::MockServer::builder().start();
        let client = Client::connect(server.origin(), Credentials::none())
            .expect("mock server should accept an anonymous connection");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            captured: captured.clone(),
        };

        tracing::subscriber::with_default(subscriber, || {
            // The mock server returns an `unknownMethod` invocation error.
            // This test verifies tracing on that failure path.
            let _ = client.single_call(
                &[session::CAPABILITY_CORE],
                "Nonexistent/thing",
                &Value::Null,
            );
        });

        let captured = captured.lock().unwrap();
        assert!(
            captured
                .iter()
                .any(|(level, name, value)| *level == Level::WARN
                    && name == "method"
                    && value == "Nonexistent/thing"),
            "expected a WARN method=Nonexistent/thing field, got {captured:?}"
        );
        assert!(
            captured
                .iter()
                .any(|(level, name, _)| *level == Level::WARN && name == "error"),
            "expected a WARN error field, got {captured:?}"
        );
    }

    #[test]
    fn keep_requested_drops_unrequested_ids_and_keeps_one_object_without_an_id() {
        use jmap_proto::Id as JmapId;
        let requested = [JmapId::new("a"), JmapId::new("b")];
        let mut list = vec![
            Some(JmapId::new("a")),
            Some(JmapId::new("x")),
            None,
            Some(JmapId::new("b")),
            None,
        ];
        Client::keep_requested(&mut list, &requested, Option::as_ref);
        assert_eq!(list, [Some(JmapId::new("a")), None, Some(JmapId::new("b"))]);

        #[derive(Debug, PartialEq, Eq)]
        struct Item {
            id: Option<JmapId>,
            tag: u32,
        }
        let mut items = vec![
            Item { id: None, tag: 1 },
            Item { id: None, tag: 2 },
            Item {
                id: Some(JmapId::new("a")),
                tag: 3,
            },
        ];
        Client::keep_requested(&mut items, &[JmapId::new("a")], |i| i.id.as_ref());
        assert_eq!(
            items,
            [
                Item { id: None, tag: 1 },
                Item {
                    id: Some(JmapId::new("a")),
                    tag: 3,
                }
            ]
        );
    }

    struct TestResolver;
    impl crate::resolver::Resolver for TestResolver {
        fn lookup_srv(&self, _domain: &str) -> Option<crate::resolver::SrvTarget> {
            Some(crate::resolver::SrvTarget {
                host: "srv.test.internal".to_string(),
                port: 9999,
            })
        }
    }

    struct TestTransport;
    impl crate::transport::Transport for TestTransport {
        fn execute(
            &self,
            _request: crate::transport::HttpRequest<'_>,
        ) -> Result<crate::transport::HttpResponse, crate::transport::TransportError> {
            Err(crate::transport::TransportError::Failed("test".into()))
        }
    }

    #[test]
    fn client_builder_methods_and_debug_representation() {
        let flag = CancelFlag::new();
        let builder = ClientBuilder::default()
            .timeout(Duration::from_secs(42))
            .connect_timeout(Duration::from_secs(11))
            .read_timeout(Duration::from_secs(22))
            .transport(TestTransport)
            .cancel_flag(flag.clone())
            .rebase_urls_to_origin(true)
            .resolver(TestResolver)
            .max_rate_limit_retries(7)
            .max_retry_after(Duration::from_secs(55))
            .id_budget(IdBudget::new(1234, 5678));

        assert_eq!(builder.timeout, Duration::from_secs(42));
        assert_eq!(builder.connect_timeout, Some(Duration::from_secs(11)));
        assert_eq!(builder.read_timeout, Some(Duration::from_secs(22)));
        assert!(builder.transport.is_some());
        assert!(builder.cancel.is_some());
        assert!(builder.rebase_urls_to_origin);
        assert_eq!(
            builder.resolver.lookup_srv("anything"),
            Some(crate::resolver::SrvTarget {
                host: "srv.test.internal".to_string(),
                port: 9999,
            })
        );
        assert_eq!(builder.max_rate_limit_retries, 7);
        assert_eq!(builder.max_retry_after, Duration::from_secs(55));
        assert_eq!(builder.id_budget, IdBudget::new(1234, 5678));

        let debug = format!("{builder:?}");
        assert!(debug.contains("timeout: 42s"));
        assert!(debug.contains("connect_timeout: Some(11s)"));
        assert!(debug.contains("read_timeout: Some(22s)"));
        assert!(debug.contains("rebase_urls_to_origin: true"));
        assert!(debug.contains("max_rate_limit_retries: 7"));
        assert!(debug.contains("max_retry_after: 55s"));
    }

    #[test]
    fn client_id_budget_accessor_returns_configured_budget() {
        let server = jmap_mock::MockServer::builder().start();
        let client = Client::builder()
            .id_budget(IdBudget::new(999, 8888))
            .connect(server.origin(), Credentials::none())
            .expect("mock server connection succeeds");
        assert_eq!(client.id_budget(), IdBudget::new(999, 8888));
    }

    #[test]
    fn idempotent_method_and_request_classification() {
        assert!(Client::is_method_idempotent("get"));
        assert!(Client::is_method_idempotent("query"));
        assert!(Client::is_method_idempotent("changes"));
        assert!(Client::is_method_idempotent("queryChanges"));
        assert!(Client::is_method_idempotent("echo"));
        assert!(Client::is_method_idempotent("getAvailability"));
        assert!(Client::is_method_idempotent("Email/get"));
        assert!(Client::is_method_idempotent("CalendarEvent/query"));
        assert!(Client::is_method_idempotent("ContactCard/changes"));

        assert!(!Client::is_method_idempotent("set"));
        assert!(!Client::is_method_idempotent("copy"));
        assert!(!Client::is_method_idempotent("import"));
        assert!(!Client::is_method_idempotent("Email/set"));
        assert!(!Client::is_method_idempotent("CalendarEvent/copy"));
        assert!(!Client::is_method_idempotent("Email/import"));

        let empty_request = Request::new([session::CAPABILITY_CORE]);
        assert!(!Client::is_request_idempotent(&empty_request));

        let idempotent_request = Request::new([session::CAPABILITY_CORE])
            .call("Core/echo", &Value::Null, "c0")
            .unwrap();
        assert!(Client::is_request_idempotent(&idempotent_request));

        let mixed_request = Request::new([session::CAPABILITY_CORE])
            .call("Core/echo", &Value::Null, "c0")
            .unwrap()
            .call("Email/set", &Value::Null, "c1")
            .unwrap();
        assert!(!Client::is_request_idempotent(&mixed_request));
    }

    #[test]
    fn compute_backoff_exponential_factor_and_cap() {
        let server = jmap_mock::MockServer::builder().start();
        let client = Client::builder()
            .max_retry_after(Duration::from_secs(10))
            .connect(server.origin(), Credentials::none())
            .expect("mock connection succeeds");

        assert_eq!(
            client.compute_backoff(Some(Duration::from_secs(4)), 1),
            Duration::from_secs(4)
        );
        assert_eq!(
            client.compute_backoff(Some(Duration::from_secs(25)), 1),
            Duration::from_secs(10)
        );

        assert_eq!(client.compute_backoff(None, 1), Duration::from_millis(500));
        assert_eq!(client.compute_backoff(None, 2), Duration::from_millis(1000));
        assert_eq!(client.compute_backoff(None, 3), Duration::from_millis(2000));
        assert_eq!(client.compute_backoff(None, 4), Duration::from_millis(4000));
        assert_eq!(client.compute_backoff(None, 5), Duration::from_millis(8000));
        assert_eq!(client.compute_backoff(None, 6), Duration::from_secs(10));
    }

    #[test]
    fn sleep_with_cancel_zero_duration_and_cancellation() {
        let server = jmap_mock::MockServer::builder().start();
        let client_flag = CancelFlag::new();
        let client = Client::builder()
            .cancel_flag(client_flag.clone())
            .connect(server.origin(), Credentials::none())
            .expect("mock connection succeeds");

        assert!(client.sleep_with_cancel(Duration::ZERO).is_ok());

        let t0 = std::time::Instant::now();
        assert!(client.sleep_with_cancel(Duration::from_millis(30)).is_ok());
        assert!(t0.elapsed() >= Duration::from_millis(25));

        client_flag.cancel();
        let err = client.sleep_with_cancel(Duration::from_millis(50));
        assert!(matches!(err, Err(Error::Cancelled)));

        let cancel_mid = CancelFlag::new();
        let cancel_clone = cancel_mid.clone();
        let client_mid = Client::builder()
            .cancel_flag(cancel_mid)
            .connect(server.origin(), Credentials::none())
            .expect("mock connection succeeds");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            cancel_clone.cancel();
        });
        let err = client_mid.sleep_with_cancel(Duration::from_secs(5));
        assert!(matches!(err, Err(Error::Cancelled)));
    }

    #[test]
    fn extract_retry_after_from_problem_method_and_set_error() {
        use jmap_proto::error::{MethodError, RequestError, SetError};

        let mut prob = RequestError::new("urn:ietf:params:jmap:error:limit");
        prob.extra
            .insert("retryAfter".into(), serde_json::json!(18));
        assert_eq!(
            extract_problem_retry_after(&prob),
            Some(Duration::from_secs(18))
        );

        let mut prob_str = RequestError::new("urn:ietf:params:jmap:error:limit");
        prob_str
            .extra
            .insert("retryAfter".into(), serde_json::json!("24"));
        assert_eq!(
            extract_problem_retry_after(&prob_str),
            Some(Duration::from_secs(24))
        );

        let mut prob_snake = RequestError::new("urn:ietf:params:jmap:error:limit");
        prob_snake
            .extra
            .insert("retry_after".into(), serde_json::json!(32));
        assert_eq!(
            extract_problem_retry_after(&prob_snake),
            Some(Duration::from_secs(32))
        );

        let empty_prob = RequestError::new("urn:ietf:params:jmap:error:limit");
        assert_eq!(extract_problem_retry_after(&empty_prob), None);

        let mut meth = MethodError::new("limit");
        meth.extra
            .insert("retryAfter".into(), serde_json::json!(42));
        assert_eq!(
            extract_method_retry_after(&meth),
            Some(Duration::from_secs(42))
        );

        let mut meth_str = MethodError::new("limit");
        meth_str
            .extra
            .insert("retryAfter".into(), serde_json::json!("55"));
        assert_eq!(
            extract_method_retry_after(&meth_str),
            Some(Duration::from_secs(55))
        );

        let empty_meth = MethodError::new("limit");
        assert_eq!(extract_method_retry_after(&empty_meth), None);

        let mut set_err = SetError::new("limit");
        set_err
            .extra
            .insert("retryAfter".into(), serde_json::json!(64));
        assert_eq!(
            extract_set_error_retry_after(&set_err),
            Some(Duration::from_secs(64))
        );

        let mut set_err_snake = SetError::new("limit");
        set_err_snake
            .extra
            .insert("retry_after".into(), serde_json::json!("77"));
        assert_eq!(
            extract_set_error_retry_after(&set_err_snake),
            Some(Duration::from_secs(77))
        );

        let empty_set = SetError::new("limit");
        assert_eq!(extract_set_error_retry_after(&empty_set), None);
    }

    #[test]
    fn credentials_constructors_and_debug_representation() {
        let none = Credentials::none();
        assert_eq!(none.authorization_header(), None);
        assert_eq!(format!("{none:?}"), "None");

        let basic = Credentials::basic("admin", "secret_pass");
        assert_eq!(
            basic.authorization_header(),
            Some("Basic YWRtaW46c2VjcmV0X3Bhc3M=".to_string())
        );
        let basic_debug = format!("{basic:?}");
        assert!(basic_debug.contains("admin"));
        assert!(!basic_debug.contains("secret_pass"));
        assert!(basic_debug.contains("<redacted>"));

        let bearer = Credentials::bearer("token_xyz");
        assert_eq!(
            bearer.authorization_header(),
            Some("Bearer token_xyz".to_string())
        );
        let bearer_debug = format!("{bearer:?}");
        assert!(!bearer_debug.contains("token_xyz"));
        assert!(bearer_debug.contains("<redacted>"));
    }

    struct MockFnTransport<F>(F);
    impl<F> crate::transport::Transport for MockFnTransport<F>
    where
        F: Fn(
                crate::transport::HttpRequest<'_>,
            )
                -> Result<crate::transport::HttpResponse, crate::transport::TransportError>
            + Send
            + Sync
            + 'static,
    {
        fn execute(
            &self,
            request: crate::transport::HttpRequest<'_>,
        ) -> Result<crate::transport::HttpResponse, crate::transport::TransportError> {
            (self.0)(request)
        }
    }

    fn make_test_session_json(max_objects: Option<u64>, max_size: Option<u64>) -> Vec<u8> {
        let mut core = serde_json::json!({
            "maxSizeUpload": 50000000,
            "maxConcurrentUpload": 4,
            "maxConcurrentRequests": 4,
            "maxObjectsInSet": 500,
            "collationAlgorithms": ["i;ascii-casemap", "i;octet"]
        });
        if let Some(objs) = max_objects {
            core.as_object_mut()
                .unwrap()
                .insert("maxObjectsInGet".into(), serde_json::json!(objs));
        }
        if let Some(size) = max_size {
            core.as_object_mut()
                .unwrap()
                .insert("maxSizeRequest".into(), serde_json::json!(size));
        }
        serde_json::to_vec(&serde_json::json!({
            "username": "user",
            "apiUrl": "http://127.0.0.1:8080/api",
            "downloadUrl": "http://127.0.0.1:8080/download",
            "uploadUrl": "http://127.0.0.1:8080/upload",
            "eventSourceUrl": "http://127.0.0.1:8080/events",
            "state": "0",
            "accounts": {
                "acc1": {
                    "name": "acc1",
                    "isPersonal": true,
                    "isReadOnly": false,
                    "accountCapabilities": {}
                }
            },
            "primaryAccounts": {
                "urn:ietf:params:jmap:core": "acc1"
            },
            "capabilities": {
                "urn:ietf:params:jmap:core": core
            }
        }))
        .unwrap()
    }

    #[test]
    fn ids_that_fit_boundaries_and_error_reporting() {
        use jmap_proto::Id as JmapId;

        let server = jmap_mock::MockServer::builder().start();
        let client = Client::connect(server.origin(), Credentials::none())
            .expect("mock connection succeeds");

        let empty_req = serde_json::json!({ "accountId": "acc1" });
        let ids = [
            JmapId::new("id_one"),
            JmapId::new("id_two"),
            JmapId::new("id_three"),
        ];

        let empty = Request::new([session::CAPABILITY_CORE])
            .call("Foo/get", &empty_req, "c0")
            .unwrap();
        let base_size = serde_json::to_vec(&empty).unwrap().len() as u64;

        let err = client
            .ids_that_fit(
                &[session::CAPABILITY_CORE],
                "Foo/get",
                &empty_req,
                &ids,
                "c0",
                base_size - 1,
            )
            .unwrap_err();
        match err {
            Error::RequestTooLarge { size, limit } => {
                assert_eq!(size, base_size);
                assert_eq!(limit, base_size - 1);
            }
            other => panic!("expected RequestTooLarge, got {other:?}"),
        }

        let fit_empty = client
            .ids_that_fit(
                &[session::CAPABILITY_CORE],
                "Foo/get",
                &empty_req,
                &[],
                "c0",
                base_size,
            )
            .unwrap();
        assert_eq!(fit_empty, 0);

        let cost0 = serde_json::to_string(&ids[0]).unwrap().len() as u64;
        let cost1 = serde_json::to_string(&ids[1]).unwrap().len() as u64 + 1;
        let cost2 = serde_json::to_string(&ids[2]).unwrap().len() as u64 + 1;

        let err0 = client
            .ids_that_fit(
                &[session::CAPABILITY_CORE],
                "Foo/get",
                &empty_req,
                &ids,
                "c0",
                base_size + cost0 - 1,
            )
            .unwrap_err();
        match err0 {
            Error::RequestTooLarge { size, limit } => {
                assert_eq!(size, base_size + cost0);
                assert_eq!(limit, base_size + cost0 - 1);
            }
            other => panic!("expected RequestTooLarge, got {other:?}"),
        }

        let fit1 = client
            .ids_that_fit(
                &[session::CAPABILITY_CORE],
                "Foo/get",
                &empty_req,
                &ids,
                "c0",
                base_size + cost0,
            )
            .unwrap();
        assert_eq!(fit1, 1);

        let fit1_again = client
            .ids_that_fit(
                &[session::CAPABILITY_CORE],
                "Foo/get",
                &empty_req,
                &ids,
                "c0",
                base_size + cost0 + cost1 - 1,
            )
            .unwrap();
        assert_eq!(fit1_again, 1);

        let fit2 = client
            .ids_that_fit(
                &[session::CAPABILITY_CORE],
                "Foo/get",
                &empty_req,
                &ids,
                "c0",
                base_size + cost0 + cost1,
            )
            .unwrap();
        assert_eq!(fit2, 2);

        let fit3 = client
            .ids_that_fit(
                &[session::CAPABILITY_CORE],
                "Foo/get",
                &empty_req,
                &ids,
                "c0",
                base_size + cost0 + cost1 + cost2,
            )
            .unwrap();
        assert_eq!(fit3, 3);
    }

    #[test]
    fn get_request_builds_request_with_account_id_ids_and_properties() {
        use jmap_proto::Id as JmapId;
        let account_id = JmapId::new("acc_test");
        let ids = [JmapId::new("id1"), JmapId::new("id2")];
        let properties = ["propA", "propB"];

        let req = Client::get_request(&account_id, &ids, Some(&properties));
        assert_eq!(req.account_id, account_id);
        assert_eq!(req.ids, Some(vec![JmapId::new("id1"), JmapId::new("id2")]));
        assert_eq!(
            req.properties,
            Some(vec!["propA".to_string(), "propB".to_string()])
        );

        let req_no_props = Client::get_request(&account_id, &ids, None);
        assert_eq!(req_no_props.properties, None);
    }

    #[test]
    fn get_chunked_combines_chunks_updates_state_and_filters_unrequested() {
        use jmap_proto::Id as JmapId;
        use jmap_proto::State as JmapState;
        use std::sync::atomic::AtomicUsize;

        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
        struct TestItem {
            id: Option<JmapId>,
            name: String,
        }

        let requests_count = Arc::new(AtomicUsize::new(0));
        let requests_count_clone = requests_count.clone();

        let transport = MockFnTransport(move |req: crate::transport::HttpRequest<'_>| {
            if req.url.ends_with("/.well-known/jmap") {
                return Ok(crate::transport::HttpResponse {
                    status: 200,
                    content_type: Some("application/json".into()),
                    body: make_test_session_json(Some(2), None),
                    final_url: req.url.to_string(),
                });
            }

            let call_idx = requests_count_clone.fetch_add(1, Ordering::SeqCst);
            let parsed_req: Request =
                serde_json::from_slice(req.body.expect("request body present")).unwrap();
            let inv = &parsed_req.method_calls[0];
            let call_id = &inv.call_id;
            let get_args: GetRequest = serde_json::from_value(inv.arguments.clone()).unwrap();

            let (list, not_found, state) = match call_idx {
                0 => {
                    assert_eq!(
                        get_args.ids,
                        Some(vec![JmapId::new("id1"), JmapId::new("id2")])
                    );
                    (
                        vec![
                            TestItem {
                                id: Some(JmapId::new("id1")),
                                name: "first".into(),
                            },
                            TestItem {
                                id: Some(JmapId::new("unrequested_extra")),
                                name: "intruder".into(),
                            },
                        ],
                        vec![JmapId::new("id2"), JmapId::new("unrequested_nf")],
                        "state_chunk1",
                    )
                }
                1 => {
                    assert_eq!(get_args.ids, Some(vec![JmapId::new("id3")]));
                    (
                        vec![TestItem {
                            id: Some(JmapId::new("id3")),
                            name: "third".into(),
                        }],
                        vec![],
                        "state_chunk2",
                    )
                }
                _ => panic!("unexpected call index {call_idx}"),
            };

            let response_body = serde_json::json!({
                "sessionState": "s0",
                "methodResponses": [
                    [
                        "TestItem/get",
                        {
                            "accountId": "acc1",
                            "state": state,
                            "list": list,
                            "notFound": not_found
                        },
                        call_id
                    ]
                ]
            });

            Ok(crate::transport::HttpResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body: serde_json::to_vec(&response_body).unwrap(),
                final_url: req.url.to_string(),
            })
        });

        let client = Client::builder()
            .transport(transport)
            .connect("http://127.0.0.1:8080", Credentials::none())
            .unwrap();

        let ids = [JmapId::new("id1"), JmapId::new("id2"), JmapId::new("id3")];
        let resp = client
            .get_chunked(
                &[session::CAPABILITY_CORE],
                "TestItem/get",
                &JmapId::new("acc1"),
                &ids,
                Some(&["name"]),
                |item: &TestItem| item.id.as_ref(),
            )
            .unwrap();

        assert_eq!(requests_count.load(Ordering::SeqCst), 2);
        assert_eq!(
            resp.list,
            [
                TestItem {
                    id: Some(JmapId::new("id1")),
                    name: "first".into(),
                },
                TestItem {
                    id: Some(JmapId::new("id3")),
                    name: "third".into(),
                }
            ]
        );
        assert_eq!(resp.not_found, [JmapId::new("id2")]);
        assert_eq!(resp.state, JmapState::new("state_chunk2"));
        assert_eq!(resp.account_id, JmapId::new("acc1"));
    }

    #[test]
    fn get_chunked_ignores_zero_max_objects_in_get_and_handles_empty_ids() {
        use jmap_proto::Id as JmapId;
        use jmap_proto::State as JmapState;
        use std::sync::atomic::AtomicUsize;

        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
        struct TestItem {
            id: Option<JmapId>,
            name: String,
        }

        let requests_count = Arc::new(AtomicUsize::new(0));
        let requests_count_clone = requests_count.clone();

        let transport = MockFnTransport(move |req: crate::transport::HttpRequest<'_>| {
            if req.url.ends_with("/.well-known/jmap") {
                return Ok(crate::transport::HttpResponse {
                    status: 200,
                    content_type: Some("application/json".into()),
                    body: make_test_session_json(Some(0), None),
                    final_url: req.url.to_string(),
                });
            }

            requests_count_clone.fetch_add(1, Ordering::SeqCst);
            let parsed_req: Request =
                serde_json::from_slice(req.body.expect("request body present")).unwrap();
            let inv = &parsed_req.method_calls[0];
            let call_id = &inv.call_id;
            let get_args: GetRequest = serde_json::from_value(inv.arguments.clone()).unwrap();

            let response_body = serde_json::json!({
                "sessionState": "s0",
                "methodResponses": [
                    [
                        "TestItem/get",
                        {
                            "accountId": "acc1",
                            "state": "state_active",
                            "list": get_args.ids.clone().unwrap_or_default().into_iter().map(|id| {
                                TestItem { id: Some(id), name: "item".into() }
                            }).collect::<Vec<_>>(),
                            "notFound": []
                        },
                        call_id
                    ]
                ]
            });

            Ok(crate::transport::HttpResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body: serde_json::to_vec(&response_body).unwrap(),
                final_url: req.url.to_string(),
            })
        });

        let client = Client::builder()
            .transport(transport)
            .connect("http://127.0.0.1:8080", Credentials::none())
            .unwrap();

        let ids = [JmapId::new("id1"), JmapId::new("id2"), JmapId::new("id3")];
        let resp = client
            .get_chunked(
                &[session::CAPABILITY_CORE],
                "TestItem/get",
                &JmapId::new("acc1"),
                &ids,
                None,
                |item: &TestItem| item.id.as_ref(),
            )
            .unwrap();
        assert_eq!(requests_count.load(Ordering::SeqCst), 1);
        assert_eq!(resp.list.len(), 3);
        assert_eq!(resp.state, JmapState::new("state_active"));

        requests_count.store(0, Ordering::SeqCst);
        let resp_empty = client
            .get_chunked(
                &[session::CAPABILITY_CORE],
                "TestItem/get",
                &JmapId::new("acc1"),
                &[],
                None,
                |item: &TestItem| item.id.as_ref(),
            )
            .unwrap();
        assert_eq!(requests_count.load(Ordering::SeqCst), 1);
        assert!(resp_empty.list.is_empty());
        assert_eq!(resp_empty.state, JmapState::new("state_active"));
    }
}
