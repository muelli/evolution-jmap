// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The blocking JMAP client.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use jmap_proto::Id;
use jmap_proto::error::{MethodError, RequestError, SetError};
use jmap_proto::methods::{SetRequest, SetResponse};
use jmap_proto::request::{Invocation, Request};
use jmap_proto::response::Response;
use jmap_proto::session::{self, Session};
use serde_json::Value;

use crate::error::Error;
use crate::limits::{self, IdBudget};
use crate::resolver::{NoSrvResolver, Resolver};
use crate::transport::{
    CancelFlag, HttpMethod, HttpRequest, HttpResponse, Transport, TransportError,
};

/// How the client authenticates (RFC 8620 leaves the scheme to HTTP).
#[derive(Debug, Clone)]
pub enum Credentials {
    None,
    Basic { user: String, password: String },
    Bearer(String),
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
    /// each chunk, so that extra objects do not stay in memory.
    pub(crate) fn keep_requested<T>(
        list: &mut Vec<T>,
        requested: &[Id],
        id: impl Fn(&T) -> Option<&Id>,
    ) {
        let requested: std::collections::HashSet<&Id> = requested.iter().collect();
        list.retain(|object| id(object).is_some_and(|id| requested.contains(id)));
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
}
