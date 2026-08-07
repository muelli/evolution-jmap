// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP transport abstraction.
//!
//! The client only needs plain request/response HTTP. Keeping it behind a
//! trait lets the Evolution Data Server integration substitute a
//! libsoup-backed transport later without touching protocol logic; the
//! [`CancelFlag`] is the seam that will map to `GCancellable`.

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
}

pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Errors a transport can produce; mapped onto [`crate::Error`] by the
/// client.
#[derive(Debug)]
pub enum TransportError {
    Cancelled,
    Failed(String),
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

            let response = result.map_err(|error| TransportError::Failed(error.to_string()))?;
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body = response
                .into_body()
                .read_to_vec()
                .map_err(|error| TransportError::Failed(error.to_string()))?;

            Ok(HttpResponse {
                status,
                content_type,
                body,
            })
        }
    }
}
