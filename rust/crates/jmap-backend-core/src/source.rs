// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading the JMAP account out of an `ESource`.
//!
//! An EDS backend is handed one `ESource` and nothing else; it is the whole
//! description of the account. This module turns it into the two pieces a
//! JMAP client needs — the origin to fetch `/.well-known/jmap` from, and the
//! user name to authenticate as — plus the identifier of the address book the
//! source stands for.
//!
//! Which extension holds what is a decision, not a lookup, because there is
//! no JMAP extension in EDS yet (M6 introduces one along with the collection
//! backend). Until then the standard extensions carry it:
//!
//! - `Authentication` — `host`, `port`, `user`. The same three keys every
//!   remote EDS backend uses, so an account written by hand looks like a
//!   CalDAV or IMAP one.
//! - `Security` — `secure`. Selects the scheme. In a keyfile that property is
//!   spelled `Method=tls` or `Method=none`, *not* `Secure=`: `secure` is a
//!   boolean over the `Method` string and only the string is stored. A
//!   keyfile that says `Secure=true` is not rejected, it is ignored, and what
//!   is left reads back as `none`.
//! - `Resource` — `identity`. The JMAP address book id. "Resource identity"
//!   is exactly what it is, and it is the extension EDS's own backends use
//!   for a server-side object identifier.
//!
//! The password is deliberately *not* here. It arrives at `connect_sync` as
//! an `ENamedParameters` that EDS filled from libsecret, and a JMAP account
//! must never read a credential from a `.source` keyfile.
//!
//! Which makes a hand-written source — the manual test recipe M3 asks for,
//! dropped in `~/.config/evolution/sources/jmap-test.source` — this:
//!
//! ```text
//! [Data Source]
//! DisplayName=JMAP test
//! Enabled=true
//!
//! [Address Book]
//! BackendName=jmap
//!
//! [Authentication]
//! Host=jmap.example.com
//! User=vera@example.com
//! Method=plain/password
//!
//! [Security]
//! Method=tls
//!
//! [Resource]
//! Identity=Ab1
//! ```
//!
//! `[Security]` may be left out entirely — it defaults to TLS here, not to
//! `ESourceSecurity:secure`'s own FALSE — and `[Resource]` may be left out to
//! get the account's default address book. Against `jmap-mockd`, `Host` is
//! `127.0.0.1`, `Port` is the mock's port and `[Security] Method` is `none`.
//!
//! The keyfile that runs against the mock is a file rather than a doc
//! comment: `docs/examples/jmap-mock.source`, with the recipe around it in
//! `docs/manual-test-book-backend.md` and both checked by
//! `jmap-backend-book`'s `recipe` test.
//!
//! ## Why the host is validated
//!
//! The origin is assembled by concatenation, so a host is not just data: a
//! `.source` file — which is a plain file in the user's home, and on a shared
//! machine not necessarily written by the user — carrying `evil.example.com`
//! in a field that is meant to be a bare host name could otherwise aim the
//! client at a different server, or slip a plaintext endpoint past the TLS
//! check below. Only a bare host name or an IP literal is accepted.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use eds_sys::{
    E_CLIENT_ERROR_INVALID_ARG, E_CLIENT_ERROR_TLS_NOT_AVAILABLE,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_RESOURCE, E_SOURCE_EXTENSION_SECURITY,
    EClientError, ESource, ESourceAuthentication, ESourceResource, ESourceSecurity,
    e_client_error_create, e_source_authentication_get_host, e_source_authentication_get_port,
    e_source_authentication_get_type, e_source_authentication_get_user, e_source_get_extension,
    e_source_has_extension, e_source_resource_get_identity, e_source_resource_get_type,
    e_source_security_get_secure, e_source_security_get_type,
};
use glib_sys::GError;

use crate::error::cstring_lossy;
use crate::marshal::read_string;

/// What a backend needs from its `ESource` in order to build a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceConfig {
    /// Scheme, host and — if the source names one — port, with no trailing
    /// slash: what `jmap_client::Client::connect` calls the origin.
    pub origin: String,
    /// The user name to authenticate as, if the source names one.
    pub user: Option<String>,
    /// The JMAP address book this source stands for. Absent means "the
    /// account's default", which the backend resolves at connect time.
    pub address_book_id: Option<String>,
}

/// A source that cannot be turned into a connection.
///
/// These are configuration faults, not server faults: retrying will not help
/// and the meta backend's offline cache is not the right answer either, so
/// they are reported to Evolution as their own `E_CLIENT_ERROR` codes rather
/// than folded into the client error mapping in [`crate::error`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The `Authentication` extension names no host.
    MissingHost,
    /// The host is not a bare host name or IP literal.
    InvalidHost(String),
    /// TLS is switched off for a host that is not loopback.
    InsecureTransport(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHost => f.write_str("the account does not name a JMAP server"),
            Self::InvalidHost(host) => {
                write!(f, "\"{host}\" is not a valid server host name")
            }
            Self::InsecureTransport(host) => write!(
                f,
                "refusing to contact {host} without TLS: credentials and contacts \
                 would go over the network in the clear"
            ),
        }
    }
}

impl std::error::Error for SourceError {}

impl SourceError {
    fn client_error_code(&self) -> EClientError {
        match self {
            // Not "other error": Evolution renders this one as a message
            // about a secure connection, which is the actionable thing to
            // tell someone whose account is configured for plain HTTP.
            Self::InsecureTransport(_) => E_CLIENT_ERROR_TLS_NOT_AVAILABLE,
            Self::MissingHost | Self::InvalidHost(_) => E_CLIENT_ERROR_INVALID_ARG,
        }
    }

    /// Allocates a `GError` describing this failure. Ownership passes to the
    /// caller, as with [`crate::error::to_gerror`].
    pub fn to_gerror(&self) -> *mut GError {
        let message = cstring_lossy(&self.to_string());
        // SAFETY: the code is one of the enum's own values and the message is
        // copied by the call.
        unsafe { e_client_error_create(self.client_error_code(), message.as_ptr()) }
    }
}

impl SourceConfig {
    /// Reads `source`'s JMAP account settings.
    ///
    /// # Safety
    ///
    /// `source` must be a valid `ESource`, i.e. the one EDS handed the
    /// backend. It is only read from, and nothing outlives the call.
    pub unsafe fn from_source(source: *mut ESource) -> Result<Self, SourceError> {
        // `e_source_get_extension` finds an extension by walking the
        // registered children of `E_TYPE_SOURCE_EXTENSION`, so a type nothing
        // has referenced yet is a type it cannot find. In a backend the EDS
        // factory has long since pulled these in, but in a test binary — or
        // any process that links libedataserver without using it — the first
        // lookup would return NULL. Referencing the GType registers it.
        unsafe {
            e_source_authentication_get_type();
            e_source_security_get_type();
            e_source_resource_get_type();
        }

        // Asked before any `e_source_get_extension`, which *creates* the
        // extension it cannot find. `ESourceSecurity:secure` defaults to
        // FALSE, so an unconditional read cannot tell "the keyfile has no
        // [Security] group" from "the user turned TLS off" — and answering
        // the first case with plain HTTP would quietly downgrade every
        // hand-written account. Absent means secure; present means what it
        // says.
        // SAFETY: the source is valid for the whole call and the name is a
        // header constant.
        let secure =
            match unsafe { e_source_has_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr()) } {
                0 => true,
                _ => {
                    // SAFETY: as above; the extension exists and is owned by the
                    // source, which outlives this call.
                    let security = unsafe {
                        e_source_get_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr())
                            .cast::<ESourceSecurity>()
                    };
                    !security.is_null() && unsafe { e_source_security_get_secure(security) } != 0
                }
            };

        // SAFETY: as above; the returned extensions are owned by the source
        // and live as long as it does.
        let (auth, resource) = unsafe {
            (
                e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr())
                    .cast::<ESourceAuthentication>(),
                e_source_get_extension(source, E_SOURCE_EXTENSION_RESOURCE.as_ptr())
                    .cast::<ESourceResource>(),
            )
        };

        // SAFETY: each pointer is either NULL — handled by the readers — or a
        // live extension of the type the name selects.
        let host = unsafe { read_string(e_source_authentication_get_host(auth)) };
        let user = unsafe { read_string(e_source_authentication_get_user(auth)) };
        let port = unsafe { e_source_authentication_get_port(auth) };
        let address_book_id = unsafe { read_string(e_source_resource_get_identity(resource)) };

        let host = host.ok_or(SourceError::MissingHost)?;
        let authority = authority(&host)?;
        if !secure && !is_loopback(&host) {
            return Err(SourceError::InsecureTransport(host));
        }

        let scheme = if secure { "https" } else { "http" };
        let origin = match port {
            // The keyfile writes 0 for "not set"; leaving it out lets the
            // scheme's default apply instead of asking for port 0.
            0 => format!("{scheme}://{authority}"),
            port => format!("{scheme}://{authority}:{port}"),
        };

        Ok(Self {
            origin,
            user,
            address_book_id,
        })
    }
}

/// The host as it appears in a URL: an IPv6 literal has to be bracketed, or
/// the colons in it would be read as the port separator.
fn authority(host: &str) -> Result<String, SourceError> {
    if host.parse::<Ipv6Addr>().is_ok() {
        return Ok(format!("[{host}]"));
    }
    if is_bare_host_name(host) {
        return Ok(host.to_owned());
    }
    Err(SourceError::InvalidHost(host.to_owned()))
}

/// A host name and nothing else — no scheme, no port, no path, no userinfo.
///
/// Deliberately stricter than RFC 1123: internationalised names reach EDS
/// already punycoded, and anything outside this set in a field that is
/// concatenated into a URL is a misconfiguration at best.
fn is_bare_host_name(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('.')
        && !host.starts_with('-')
        && !host.contains("..")
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Whether plaintext to this host stays on the machine. Everything the mock
/// server and a local development instance are reached by, and nothing else.
fn is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("localhost.localdomain")
    {
        return true;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return v4.is_loopback();
    }
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        return v6.is_loopback();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipv6_literal_is_bracketed_and_a_name_is_not() {
        assert_eq!(authority("::1").as_deref(), Ok("[::1]"));
        assert_eq!(
            authority("jmap.example.com").as_deref(),
            Ok("jmap.example.com")
        );
    }

    #[test]
    fn only_loopback_addresses_count_as_local() {
        for host in ["localhost", "LocalHost", "127.0.0.1", "127.9.9.9", "::1"] {
            assert!(is_loopback(host), "{host} should be loopback");
        }
        // The near-misses that a plausible-looking name could exploit.
        for host in [
            "localhost.example.com",
            "notlocalhost",
            "10.0.0.1",
            "0.0.0.0",
            "127.0.0.1.example.com",
            "::2",
        ] {
            assert!(!is_loopback(host), "{host} should not be loopback");
        }
    }
}
