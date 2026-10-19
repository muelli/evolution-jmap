// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP transport abstraction.
//!
//! Trait definitions for unary HTTP requests and responses.
//! [`CancelFlag`] provides cooperative cancellation.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancellation flag shared between an operation and its caller.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

thread_local! {
    /// The flag this thread's current operation is cancelled through, if it
    /// installed one. See [`CancelScope`].
    static OBSERVED: RefCell<Option<CancelFlag>> = const { RefCell::new(None) };
}

/// Return the cancellation flag active on this thread.
///
/// Returns `None` if no [`CancelScope`] is installed.
pub fn observed() -> Option<CancelFlag> {
    OBSERVED.with(|observed| observed.borrow().clone())
}

/// RAII guard setting a thread-local cancellation flag for an operation.
///
/// Operations can set a flag for blocking execution on the current thread.
/// Dropping the scope restores the previous flag.
/// Clients check this scoped flag before falling back to default flags.
pub struct CancelScope {
    /// What this thread observed before, put back on drop.
    previous: Option<CancelFlag>,
    /// Not `Send`: the guard restores a thread-local, so it has to be dropped
    /// on the thread that installed it.
    _thread_bound: PhantomData<*const ()>,
}

impl CancelScope {
    /// Install `flag` on the current thread until the returned scope drops.
    ///
    /// Scopes support nesting. Dropping a child scope restores the parent flag.
    pub fn install(flag: &CancelFlag) -> Self {
        let previous = OBSERVED.with(|observed| observed.replace(Some(flag.clone())));
        Self {
            previous,
            _thread_bound: PhantomData,
        }
    }
}

impl Drop for CancelScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        OBSERVED.with(|observed| observed.replace(previous));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

/// A request as handed to the transport. Headers already include
/// authorization and content type.
pub struct HttpRequest<'a> {
    pub method: HttpMethod,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
    pub cancel: Option<&'a CancelFlag>,
    /// Maximum response body size in bytes.
    ///
    /// Responses exceeding this size return [`TransportError::ResponseTooLarge`].
    /// Transports must enforce this limit while streaming data to bound memory consumption.
    pub max_response_bytes: u64,
}

pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    /// Final URL after following redirects.
    ///
    /// Defaults to [`HttpRequest::url`] if no redirects occurred.
    /// Callers use this value to detect cross-origin redirects.
    pub final_url: String,
}

/// Errors a transport can produce; mapped onto [`crate::Error`] by the
/// client.
#[derive(Debug)]
pub enum TransportError {
    Cancelled,
    Failed(String),
    /// Response exceeded the configured byte limit.
    ///
    /// `limit` records the maximum allowed size from [`HttpRequest::max_response_bytes`].
    ResponseTooLarge {
        limit: u64,
    },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Cancelled => write!(f, "transport request cancelled"),
            TransportError::Failed(message) => write!(f, "transport error: {message}"),
            TransportError::ResponseTooLarge { limit } => {
                write!(f, "response body exceeded maximum limit of {limit} bytes")
            }
        }
    }
}

impl std::error::Error for TransportError {}

pub trait Transport: Send + Sync + 'static {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError>;
}

#[cfg(feature = "transport-ureq")]
pub use ureq_transport::UreqTransport;

#[cfg(feature = "transport-ureq")]
mod ureq_transport {
    use std::time::Duration;

    use super::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};

    /// Default [`Transport`] built on `ureq` (blocking, rustls).
    pub struct UreqTransport {
        agent: ureq::Agent,
    }

    impl UreqTransport {
        /// Build a transport with explicit timeouts for connection, response read,
        /// and total call duration.
        pub fn with_timeouts(connect: Duration, read: Duration, total: Duration) -> Self {
            let config = ureq::Agent::config_builder()
                // Non-2xx responses must reach the client as data (the body
                // carries RFC 7807 problem details), not as transport errors.
                .http_status_as_error(false)
                .timeout_connect(Some(connect))
                .timeout_recv_response(Some(read))
                .timeout_recv_body(Some(read))
                .timeout_global(Some(total))
                // Redirects are handled directly in execute() to ensure
                // trailing dot hosts (e.g. api.fastmail.com.) are recognized as
                // the same origin as dotless hosts, keeping Authorization headers.
                .max_redirects(0)
                .build();
            Self {
                agent: config.into(),
            }
        }

        pub fn new(timeout: Duration) -> Self {
            Self::with_timeouts(timeout, timeout, timeout)
        }
    }

    impl Default for UreqTransport {
        fn default() -> Self {
            Self::new(Duration::from_secs(30))
        }
    }

    impl Transport for UreqTransport {
        fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            let mut current_url = request.url.to_string();
            let mut current_method = request.method;
            let mut current_headers: Vec<(String, String)> = request.headers.to_vec();
            let mut redirect_count = 0;

            loop {
                if request.cancel.is_some_and(super::CancelFlag::is_cancelled) {
                    return Err(TransportError::Cancelled);
                }

                tracing::trace!(
                    method = ?current_method,
                    url = %current_url,
                    redirect_count,
                    "sending HTTP request"
                );

                let result = match current_method {
                    HttpMethod::Get => {
                        let mut builder = self.agent.get(&current_url);
                        for (name, value) in &current_headers {
                            builder = builder.header(name, value);
                        }
                        builder.call()
                    }
                    HttpMethod::Post => {
                        let mut builder = self.agent.post(&current_url);
                        for (name, value) in &current_headers {
                            builder = builder.header(name, value);
                        }
                        builder.send(request.body.unwrap_or_default())
                    }
                };

                let response = result.map_err(|error| {
                    tracing::debug!(url = %current_url, %error, "HTTP request failed");
                    TransportError::Failed(error.to_string())
                })?;

                let status = response.status().as_u16();

                if matches!(status, 301..=303 | 307 | 308)
                    && redirect_count < 10
                    && let Some(location) = response
                        .headers()
                        .get("location")
                        .and_then(|value| value.to_str().ok())
                {
                    let next_url = crate::url::resolve_redirect(&current_url, location)
                        .map_err(TransportError::Failed)?;

                    let same_origin = crate::url::same_origin(&current_url, &next_url);
                    if !same_origin {
                        // Cross-origin redirect: strip Authorization per RFC 7235
                        current_headers
                            .retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
                    }

                    if matches!(status, 301..=303) && current_method == HttpMethod::Post {
                        current_method = HttpMethod::Get;
                        current_headers.retain(|(name, _)| {
                            !name.eq_ignore_ascii_case("content-type")
                                && !name.eq_ignore_ascii_case("content-length")
                        });
                    }

                    drop(response);
                    current_url = next_url;
                    redirect_count += 1;
                    continue;
                }

                if matches!(status, 301..=303 | 307 | 308) && redirect_count >= 10 {
                    return Err(TransportError::Failed("too many redirects".to_string()));
                }

                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);

                tracing::trace!(
                    status,
                    final_url = %current_url,
                    redirected = current_url != request.url,
                    "received HTTP response"
                );

                let body = response
                    .into_body()
                    .with_config()
                    .limit(request.max_response_bytes.saturating_add(1))
                    .read_to_vec()
                    .map_err(|error| match error {
                        ureq::Error::BodyExceedsLimit(_) => TransportError::ResponseTooLarge {
                            limit: request.max_response_bytes,
                        },
                        error => TransportError::Failed(error.to_string()),
                    })?;

                return Ok(HttpResponse {
                    status,
                    content_type,
                    body,
                    final_url: current_url,
                });
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::{Arc, Mutex};

        use tracing::field::{Field, Visit};
        use tracing::span::{Attributes, Id, Record};
        use tracing::{Event, Level, Metadata, Subscriber};

        use super::*;

        /// Collects tracing events for test assertions.
        ///
        /// Records log levels and structured fields.
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
                self.sink.lock().unwrap().push((
                    self.level,
                    field.name().to_owned(),
                    value.to_owned(),
                ));
            }

            fn record_bool(&mut self, field: &Field, value: bool) {
                self.sink.lock().unwrap().push((
                    self.level,
                    field.name().to_owned(),
                    value.to_string(),
                ));
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

        type CapturedEvents = Vec<(Level, String, String)>;

        fn run_captured(
            request: HttpRequest<'_>,
        ) -> (Result<HttpResponse, TransportError>, CapturedEvents) {
            let captured = Arc::new(Mutex::new(Vec::new()));
            let subscriber = CapturingSubscriber {
                captured: captured.clone(),
            };
            let transport = UreqTransport::default();
            let result =
                tracing::subscriber::with_default(subscriber, || transport.execute(request));
            let captured = captured.lock().unwrap().clone();
            (result, captured)
        }

        #[test]
        fn a_plain_request_traces_its_url_and_the_response_status() {
            let server = jmap_mock::MockServer::builder().start();
            let url = format!("{}/.well-known/jmap", server.origin());

            let (result, captured) = run_captured(HttpRequest {
                method: HttpMethod::Get,
                url: &url,
                headers: &[],
                body: None,
                cancel: None,
                max_response_bytes: 1_000_000,
            });

            result.expect("the mock server answers the well-known path");
            assert!(
                captured
                    .iter()
                    .any(|(level, name, value)| *level == Level::TRACE
                        && name == "url"
                        && *value == url),
                "expected a TRACE url={url} field before the request, got {captured:?}"
            );
            assert!(
                captured
                    .iter()
                    .any(|(level, name, _)| *level == Level::TRACE && name == "status"),
                "expected a TRACE status field after the response, got {captured:?}"
            );
            assert!(
                captured
                    .iter()
                    .any(|(level, name, value)| *level == Level::TRACE
                        && name == "redirected"
                        && value == "false"),
                "a same-URL response should trace redirected=false, got {captured:?}"
            );
        }

        #[test]
        fn a_redirected_request_traces_redirected_true_and_the_final_url() {
            let server = jmap_mock::MockServer::builder()
                .session_via_redirect()
                .start();
            let url = format!("{}/.well-known/jmap", server.origin());

            let (result, captured) = run_captured(HttpRequest {
                method: HttpMethod::Get,
                url: &url,
                headers: &[],
                body: None,
                cancel: None,
                max_response_bytes: 1_000_000,
            });

            let response = result.expect("the redirect target answers the request");
            assert_ne!(
                response.final_url, url,
                "session_via_redirect should actually redirect to a different path"
            );
            assert!(
                captured
                    .iter()
                    .any(|(level, name, value)| *level == Level::TRACE
                        && name == "redirected"
                        && value == "true"),
                "a redirected response should trace redirected=true, got {captured:?}"
            );
            assert!(
                captured
                    .iter()
                    .any(|(level, name, value)| *level == Level::TRACE
                        && name == "final_url"
                        && *value == response.final_url),
                "expected a TRACE final_url={} field, got {captured:?}",
                response.final_url
            );
        }

        #[test]
        fn a_failed_request_traces_the_error_at_debug() {
            // Port 1 refuses the connection immediately, which produces a fast test failure.
            let (result, captured) = run_captured(HttpRequest {
                method: HttpMethod::Get,
                url: "http://127.0.0.1:1/",
                headers: &[],
                body: None,
                cancel: None,
                max_response_bytes: 1_000_000,
            });

            assert!(
                matches!(result, Err(TransportError::Failed(_))),
                "connecting to a port nothing listens on should fail the transport"
            );
            assert!(
                captured
                    .iter()
                    .any(|(level, name, _)| *level == Level::DEBUG && name == "error"),
                "expected a DEBUG error field on transport failure, got {captured:?}"
            );
        }
    }
}
