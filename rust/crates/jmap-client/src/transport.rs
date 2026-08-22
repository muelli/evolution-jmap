// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP transport abstraction.
//!
//! The client only needs plain request/response HTTP. Keeping it behind a
//! trait lets the Evolution Data Server integration substitute a
//! libsoup-backed transport later without touching protocol logic; the
//! [`CancelFlag`] is the seam that maps to `GCancellable`.

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

/// The flag every request made on this thread is currently checked against,
/// if an operation installed one.
///
/// Public because the layer that bridges a `GCancellable` onto it lives in
/// another crate and its tests have to be able to ask what this thread is
/// observing without making a request to find out.
pub fn observed() -> Option<CancelFlag> {
    OBSERVED.with(|observed| observed.borrow().clone())
}

/// A [`CancelFlag`] installed for the length of one operation, on the thread
/// running it.
///
/// The problem this exists for: a [`Client`] is built once per account and used
/// for every operation on it, but the thing that cancels an operation belongs to
/// the *operation*. Camel and EDS both hand a sync vfunc a `GCancellable` that
/// means "stop this call", and that vfunc has no way to reach into a client
/// built long before it and re-point a flag — nor should it, because the next
/// vfunc along may be running on another thread at the same moment, under a
/// cancellable of its own.
///
/// A thread-local is the exact shape of that: these are *blocking* vfuncs, so
/// the operation being cancelled is the one this thread is inside, and one
/// thread is inside exactly one at a time. What was observed before is restored
/// when the scope drops, so a folder operation that calls into its store leaves
/// the outer operation's cancellation in place.
///
/// A client checks a scope in preference to the flag it was built with — see
/// [`Client::execute`].
///
/// [`Client`]: crate::Client
/// [`Client::execute`]: crate::Client
pub struct CancelScope {
    /// What this thread observed before, put back on drop.
    previous: Option<CancelFlag>,
    /// Not `Send`: the guard restores a thread-local, so it has to be dropped
    /// on the thread that installed it.
    _thread_bound: PhantomData<*const ()>,
}

impl CancelScope {
    /// Makes `flag` the cancellation of every request this thread makes until
    /// the returned scope is dropped.
    ///
    /// Scopes nest, and are expected to be dropped in the reverse of the order
    /// they were installed — which is what holding one in a local of the vfunc
    /// it belongs to guarantees. Dropping them out of order restores an older
    /// observation over a newer one; nothing here can detect that, and no
    /// caller has reason to do it.
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
    /// The most octets of response body this request will accept — exactly
    /// this many are fine, one more is [`TransportError::ResponseTooLarge`].
    ///
    /// It has no default and every caller states it, which is the whole point:
    /// a response is buffered whole, JMAP gives no number to bound one by (see
    /// [`crate::limits`]), and the number in force before this field existed
    /// was a dependency's. A transport must apply it rather than treat it as
    /// advice, and must apply it while *reading* rather than after: a body that
    /// is refused for being too long has to stop being read at the ceiling, or
    /// the memory it was meant to bound has already been taken.
    pub max_response_bytes: u64,
}

pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    /// The URL this response actually came from — [`HttpRequest::url`] unless
    /// a redirect was followed, in which case this is the last one.
    ///
    /// A caller that must not trust a redirect target it never named (a blob
    /// download addressed by the session's own `downloadUrl`, in particular —
    /// see [`crate::Client::download_blob`]) compares this against the URL it
    /// asked for; a transport with nothing to report here because it never
    /// redirects sets it to the request's own URL.
    pub final_url: String,
}

/// Errors a transport can produce; mapped onto [`crate::Error`] by the
/// client.
#[derive(Debug)]
pub enum TransportError {
    Cancelled,
    Failed(String),
    /// The response body passed [`HttpRequest::max_response_bytes`] and was
    /// abandoned there; `limit` is the ceiling it passed.
    ///
    /// Separate from [`Self::Failed`] because the two mean opposite things to
    /// whoever is waiting: a failed request may well work on the next attempt,
    /// while a body that is too large will be too large every time, and the
    /// number is what a layer above needs in order to say so.
    ResponseTooLarge {
        limit: u64,
    },
}

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
        pub fn new(timeout: Duration) -> Self {
            let config = ureq::Agent::config_builder()
                // Non-2xx responses must reach the client as data (the body
                // carries RFC 7807 problem details), not as transport errors.
                .http_status_as_error(false)
                .timeout_global(Some(timeout))
                // ureq's default (`Never`) strips `Authorization` on every
                // redirect, even a same-host one. A server that serves its
                // session document via a same-host redirect (Stalwart's
                // `/.well-known/jmap` -> `/jmap/session`, for one) then sees
                // an unauthenticated request and answers with an anonymous,
                // empty-accounts session — not a 401, so the failure surfaces
                // confusingly downstream as "no primary account" rather than
                // here. Cross-host redirects still get no auth header, which
                // is the safe default RFC 7235 leaves it out for.
                .redirect_auth_headers(ureq::config::RedirectAuthHeaders::SameHost)
                .build();
            Self {
                agent: config.into(),
            }
        }
    }

    impl Default for UreqTransport {
        fn default() -> Self {
            Self::new(Duration::from_secs(30))
        }
    }

    impl Transport for UreqTransport {
        fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            if request.cancel.is_some_and(super::CancelFlag::is_cancelled) {
                return Err(TransportError::Cancelled);
            }

            tracing::trace!(
                method = ?request.method,
                url = request.url,
                "sending HTTP request"
            );

            let result = match request.method {
                HttpMethod::Get => {
                    let mut builder = self.agent.get(request.url);
                    for (name, value) in request.headers {
                        builder = builder.header(name, value);
                    }
                    builder.call()
                }
                HttpMethod::Post => {
                    let mut builder = self.agent.post(request.url);
                    for (name, value) in request.headers {
                        builder = builder.header(name, value);
                    }
                    builder.send(request.body.unwrap_or_default())
                }
            };

            let response = result.map_err(|error| {
                tracing::debug!(url = request.url, %error, "HTTP request failed");
                TransportError::Failed(error.to_string())
            })?;
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            // The URL this response actually came from, which `ureq` tracks
            // regardless of `redirect_auth_headers` — that setting only
            // decides whether `Authorization` follows a redirect, not
            // whether the redirect itself is taken. `get_uri()` is the
            // request's own URL when nothing redirected.
            let final_url = ureq::ResponseExt::get_uri(&response).to_string();
            tracing::trace!(
                status,
                final_url = %final_url,
                redirected = final_url != request.url,
                "received HTTP response"
            );
            // `read_to_vec()` would apply `ureq`'s own `MAX_BODY_SIZE`; the
            // limit is set here so the number in force is the caller's.
            //
            // One more than the ceiling, deliberately. `ureq`'s limiting reader
            // fails on the read that finds nothing left rather than on the
            // octet that overran, so a body of exactly `limit` octets is
            // rejected by it: the last read consumes the allowance, and the
            // read that would have returned end-of-file finds none. Asking for
            // one octet more leaves that read an allowance to return zero
            // against, which makes `max_response_bytes` mean "this many octets
            // are fine" — the only reading a caller sizing a ceiling from a
            // blob's own `size` can use.
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

            Ok(HttpResponse {
                status,
                content_type,
                body,
                final_url,
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::{Arc, Mutex};

        use tracing::field::{Field, Visit};
        use tracing::span::{Attributes, Id, Record};
        use tracing::{Event, Level, Metadata, Subscriber};

        use super::*;

        /// Records every event this module emits (level + fields), so a test
        /// can assert a request attached structured fields rather than only a
        /// free-text message — duplicated from `client.rs`'s own test
        /// harness for the same reason that one gives: this crate depends on
        /// `tracing`, not `tracing-subscriber`, so there is no ready-made
        /// capturing layer to share across modules.
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
            // Nothing listens here: 127.0.0.1:1 is a reserved, unassigned
            // port that refuses the connection immediately rather than
            // timing out, so this is a fast, deterministic transport
            // failure.
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
