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

pub struct ClientBuilder {
    timeout: Duration,
    transport: Option<Box<dyn Transport>>,
    cancel: Option<CancelFlag>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            transport: None,
            cancel: None,
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

        let session_url = format!("{}/.well-known/jmap", origin.trim_end_matches('/'));
        let mut client = Client {
            transport,
            authorization: credentials.authorization_header(),
            cancel: self.cancel,
            session_url,
            session: None,
            next_call_id: AtomicU64::new(0),
        };
        client.refresh_session()?;
        Ok(client)
    }
}

pub struct Client {
    transport: Box<dyn Transport>,
    authorization: Option<String>,
    cancel: Option<CancelFlag>,
    session_url: String,
    session: Option<Session>,
    next_call_id: AtomicU64,
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
    pub fn connect(origin: &str, credentials: Credentials) -> Result<Self, Error> {
        Self::builder().connect(origin, credentials)
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
        let session: Session = serde_json::from_slice(&response.body)?;
        self.session = Some(session);
        Ok(())
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
        let request = Request::new(using.iter().copied()).call(method, arguments, &call_id)?;
        let response = self.api_call(&request)?;
        let invocation = response
            .responses_for(&call_id)
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
    pub(crate) fn execute_within(
        &self,
        method: HttpMethod,
        url: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
        max_response_bytes: u64,
    ) -> Result<HttpResponse, Error> {
        let cancel = self.cancel_for_request();
        if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
            return Err(Error::Cancelled);
        }

        let mut headers: Vec<(String, String)> =
            vec![("Accept".to_owned(), "application/json".to_owned())];
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
