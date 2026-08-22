// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The blocking JMAP client.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use jmap_proto::error::RequestError;
use jmap_proto::request::{Invocation, Request};
use jmap_proto::response::Response;
use jmap_proto::session::{self, Session};
use serde_json::Value;

use crate::error::Error;
use crate::limits;
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

/// Whether `JMAP_LIVE_SERVER_REBASE_URLS` asks for
/// [`ClientBuilder::rebase_urls_to_origin`]. Shared by every top-level
/// `connect*` convenience so the env var switches them all identically,
/// rather than each one growing its own copy of the same two comparisons.
pub fn rebase_urls_from_env() -> bool {
    std::env::var("JMAP_LIVE_SERVER_REBASE_URLS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub struct ClientBuilder {
    timeout: Duration,
    transport: Option<Box<dyn Transport>>,
    cancel: Option<CancelFlag>,
    rebase_urls_to_origin: bool,
    resolver: Box<dyn Resolver>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            transport: None,
            cancel: None,
            rebase_urls_to_origin: false,
            resolver: Box::new(NoSrvResolver),
        }
    }
}

impl ClientBuilder {
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

    /// After fetching the session, rewrite the scheme and authority of every
    /// URL it names (`apiUrl`, `downloadUrl`, `uploadUrl`, `eventSourceUrl`)
    /// to the origin this client connected to, keeping each URL's path and
    /// query as the server stated them.
    ///
    /// Off by default: RFC 8620 states these are the server's own URLs, and a
    /// deployment reachable at the address it names never needs this. It
    /// exists for a server whose session document names a scheme/host the
    /// client cannot route to even though the document itself came from a
    /// reachable address — a reverse proxy, a NAT boundary, or (the case that
    /// motivated this) a configured public hostname advertised over `https`
    /// when only a plain-`http` listener on a different address is actually
    /// reachable. Turning this on trusts that the origin already reached is
    /// the same deployment the session names; it is not a substitute for
    /// verifying that out of band.
    pub fn rebase_urls_to_origin(mut self, rebase: bool) -> Self {
        self.rebase_urls_to_origin = rebase;
        self
    }

    /// The [`Resolver`] [`connect_domain`](Self::connect_domain) consults for
    /// a `_jmap._tcp` SRV record before falling back to the bare domain.
    /// Defaults to [`NoSrvResolver`], which never finds one.
    pub fn resolver(mut self, resolver: impl Resolver + 'static) -> Self {
        self.resolver = Box::new(resolver);
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
                    Box::new(crate::transport::UreqTransport::new(self.timeout))
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
            authorization: credentials.authorization_header(),
            cancel: self.cancel,
            session_url,
            session: None,
            next_call_id: AtomicU64::new(0),
            rebase_origin: self.rebase_urls_to_origin.then_some(origin),
            rebase_note: None,
        };
        client.refresh_session()?;
        Ok(client)
    }

    /// Fetch the session object for an email domain, trying a `_jmap._tcp`
    /// SRV target via [`resolver`](Self::resolver) first (RFC 8620 §2.2) and
    /// falling back to `https://<domain>/.well-known/jmap` when the resolver
    /// finds no record — [`NoSrvResolver`]'s permanent answer, and so what
    /// happens when [`resolver`](Self::resolver) is never called.
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
    authorization: Option<String>,
    cancel: Option<CancelFlag>,
    session_url: String,
    session: Option<Session>,
    next_call_id: AtomicU64,
    rebase_origin: Option<String>,
    /// Set once `refresh_session` actually rewrites `downloadUrl`'s origin —
    /// distinct from `rebase_origin.is_some()`, which is true whenever the
    /// opt-in is on even if the advertised and connected origins already
    /// happen to match. See [`Error::CrossOriginRedirect`].
    rebase_note: Option<String>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("session_url", &self.session_url)
            .field("authenticated", &self.authorization.is_some())
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Connect with default transport and timeout.
    ///
    /// Honours `JMAP_LIVE_SERVER_REBASE_URLS` as an opt-in escape hatch for
    /// reaching a deployment through an address it does not advertise — a
    /// `socat`/localhost forward or a reverse proxy — by rebasing the session's
    /// URLs onto the origin actually connected through (see
    /// [`ClientBuilder::rebase_urls_to_origin`]). Off by default (RFC-strict:
    /// follow `apiUrl` as given); set the variable to `1`/`true` to enable it.
    /// Every EDS backend reaches a server through this method, so one env var
    /// switches them all.
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

    /// Re-fetch the session object (RFC 8620 §2: clients refresh when
    /// `sessionState` changes).
    pub fn refresh_session(&mut self) -> Result<(), Error> {
        let response = self.execute(HttpMethod::Get, &self.session_url.clone(), None)?;
        let mut session: Session = serde_json::from_slice(&response.body)?;
        if let Some(origin) = &self.rebase_origin {
            let advertised_download_origin =
                crate::url::origin_of(&session.download_url).to_owned();
            session.api_url = crate::url::rebase_origin(&session.api_url, origin);
            session.download_url = crate::url::rebase_origin(&session.download_url, origin);
            session.upload_url = crate::url::rebase_origin(&session.upload_url, origin);
            session.event_source_url = crate::url::rebase_origin(&session.event_source_url, origin);
            self.rebase_note = if advertised_download_origin == *origin {
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
        self.session = Some(session);
        Ok(())
    }

    /// The note explaining a `downloadUrl` rewrite, if the rebase opt-in
    /// actually changed its origin — see [`Error::CrossOriginRedirect`].
    pub(crate) fn rebase_note(&self) -> Option<&str> {
        self.rebase_note.as_deref()
    }

    /// The account id serving a capability URN — `primaryAccounts` where the
    /// server states one, else the sole personal account offering it (see
    /// [`jmap_proto::session::Session::resolve_primary_account`]).
    pub fn primary_account(&self, capability: &str) -> Result<jmap_proto::Id, Error> {
        self.session()
            .resolve_primary_account(capability)
            .cloned()
            .ok_or_else(|| Error::Protocol(format!("no primary account for {capability}")))
    }

    /// A fresh call id, unique within this client.
    pub fn next_call_id(&self) -> String {
        format!("c{}", self.next_call_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Send a full request envelope to the API endpoint.
    ///
    /// A request longer than the session's `maxSizeRequest` is refused here,
    /// without being sent: RFC 8620 §2 has the server refuse it on its octets
    /// with a request-level error, so sending it costs a round trip and returns
    /// nothing. What comes back instead is [`Error::RequestTooLarge`] with both
    /// numbers, which is what a caller that could split its request needs in
    /// order to decide to.
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
        let response = self.execute(HttpMethod::Post, &api_url, Some(&body))?;
        Ok(serde_json::from_slice(&response.body)?)
    }

    /// Send a single method call and return the arguments of its (single)
    /// response, converting method-level errors into [`Error::Method`].
    pub fn single_call(
        &self,
        using: &[&str],
        method: &str,
        arguments: &impl serde::Serialize,
    ) -> Result<Value, Error> {
        let call_id = self.next_call_id();
        tracing::debug!(method, call_id, "sending JMAP method call");
        let result = self.single_call_inner(using, method, arguments, &call_id);
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
        call_id: &str,
    ) -> Result<Value, Error> {
        let request = Request::new(using.iter().copied()).call(method, arguments, call_id)?;
        let response = self.api_call(&request)?;
        let invocation = response
            .responses_for(call_id)
            .next()
            .ok_or_else(|| Error::Protocol(format!("no response for call id {call_id}")))?;
        Self::unwrap_invocation(invocation, method)
    }

    /// Whether this server has said it will take a request carrying `calls`
    /// method calls (RFC 8620 §2's `maxCallsInRequest`).
    ///
    /// Asked before a chain is built, because over the limit the server refuses
    /// the *request* — `urn:ietf:params:jmap:error:limit`, RFC 8620 §3.2 — and
    /// not merely the call that went over it. A client that chains two calls to
    /// save a round trip and gets neither answered has spent the round trip it
    /// was saving and has no data.
    ///
    /// A server naming no limit is taken at its word and sent the chain: see
    /// [`Session::max_calls_in_request`] for why nothing is invented here.
    ///
    /// [`Session::max_calls_in_request`]: jmap_proto::session::Session::max_calls_in_request
    pub(crate) fn takes_calls_in_one_request(&self, calls: u64) -> bool {
        self.session()
            .max_calls_in_request()
            .is_none_or(|limit| calls <= limit)
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

    /// What the request about to be made is cancelled through.
    ///
    /// The operation's own scope if the thread running it installed one (see
    /// [`CancelScope`]), and otherwise the flag this client was built with.
    ///
    /// The order is the point. A client-wide flag can only ever be set once —
    /// there is no way to unset one — so a client whose flag was cancelled at
    /// connect time would refuse every operation the account ever performed
    /// afterwards, including the one the user is waiting on. An operation that
    /// says what its own cancellation is has said something more specific, and
    /// it is what gets honoured.
    ///
    /// [`CancelScope`]: crate::transport::CancelScope
    fn cancel_for_request(&self) -> Option<CancelFlag> {
        crate::transport::observed().or_else(|| self.cancel.clone())
    }

    /// A request whose answer is JSON this client parses, held to
    /// [`limits::MAX_API_RESPONSE_BYTES`].
    pub(crate) fn execute(
        &self,
        method: HttpMethod,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, Error> {
        self.execute_with_content_type(method, url, body, body.map(|_| "application/json"))
    }

    /// As [`Self::execute`], for a request whose body is not JSON. The
    /// *response* still is — an upload answers with a blob descriptor — so the
    /// ceiling is the same one.
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

    /// The one that actually makes the request, and the only place a ceiling
    /// is put on an answer.
    ///
    /// `max_response_bytes` is a caller's number rather than a default because
    /// the two kinds of answer this client reads have nothing in common: a JSON
    /// response is bounded by the question that was asked, and a blob download
    /// is bounded by what the account said the blob weighs. See
    /// [`crate::limits`].
    ///
    /// `accept` is a caller's choice rather than a constant for the same
    /// reason: every JMAP API call answers with JSON, but a blob download
    /// does not, and RFC 8620 §6.2 gives it no reason to declare
    /// `application/json` acceptable — a server doing content negotiation on
    /// that header could legitimately refuse or redirect a download that
    /// claims to accept only JSON for a response that never is.
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
        if let Some(authorization) = &self.authorization {
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

        if !(200..300).contains(&response.status) {
            let problem: Option<RequestError> = serde_json::from_slice(&response.body).ok();
            return Err(Error::Http {
                status: response.status,
                problem,
            });
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::*;

    /// White-box: `timeout`'s field is private, so only an in-crate test can
    /// see that the builder kept the value passed rather than falling back to
    /// a fresh default (both look identical from outside the crate until a
    /// connection is slow enough to time out).
    #[test]
    fn timeout_replaces_the_default() {
        let builder = ClientBuilder::default().timeout(Duration::from_secs(5));
        assert_eq!(builder.timeout, Duration::from_secs(5));
    }

    /// Records every event this crate emits (level + fields), so a test can
    /// assert a call attached structured fields rather than only a free-text
    /// message — the same minimal harness `jmap-backend-core::trampoline`'s
    /// own tests use, duplicated here because this crate depends on
    /// `tracing`, not `tracing-subscriber`.
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
            // A method name the mock never registered: it answers with an
            // `unknownMethod` error invocation rather than a transport
            // failure, which is the failure path this test targets.
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
