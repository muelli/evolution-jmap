// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading the JMAP account out of an `ESource`.
//!
//! EDS gives a backend one `ESource` and nothing else: it is the full
//! description of the account. This module turns it into the two pieces a
//! JMAP client needs (the origin to fetch `/.well-known/jmap` from, and the
//! user name to authenticate as), plus the identifier of the address book or
//! calendar that the source stands for.
//!
//! Which extension holds which setting is a decision, not a lookup. EDS has
//! no JMAP extension for these settings (the custom `[JMAP Rebase]`
//! extension holds only the rebase opt-in), so the standard extensions hold
//! them:
//!
//! - `Authentication`: `host`, `port`, `user`. Each remote EDS backend uses
//!   these three keys, so an account written by hand looks like a CalDAV or
//!   IMAP account.
//! - `Security`: `secure`. Selects the scheme. In a keyfile, this property is
//!   `Method=tls` or `Method=none`, *not* `Secure=`: `secure` is a boolean
//!   over the `Method` string, and EDS stores only the string. EDS does not
//!   reject a keyfile that says `Secure=true`. It ignores that key, and the
//!   property then reads back as `none`.
//! - `Resource`: `identity`. The JMAP identifier of the object that this
//!   source stands for: an address book id under the address book backend, a
//!   calendar id under the calendar backend. "Resource identity" is the
//!   correct name for it, and the backends of EDS use this extension for a
//!   server-side object identifier.
//!
//! The password is deliberately *not* here. It gets to `connect_sync` as an
//! `ENamedParameters` that EDS fills from libsecret. A JMAP account must
//! never read a credential from a `.source` keyfile.
//!
//! Thus a hand-written source (the manual test recipe that M3 requires, in
//! `~/.config/evolution/sources/jmap-test.source`) looks like this:
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
//! You can omit `[Security]`: here it defaults to TLS, not to the FALSE
//! default of `ESourceSecurity:secure`. You can omit `[Resource]` to get the
//! default address book or default calendar of the account. For
//! `jmap-mockd`, `Host` is `127.0.0.1`, `Port` is the port of the mock and
//! `[Security] Method` is `none`.
//!
//! The keyfile for the mock is a file, not a doc comment:
//! `docs/examples/jmap-mock.source`. Its recipe is in
//! `docs/manual-test-book-backend.md`, and the `recipe` test of
//! `jmap-backend-book` checks both.
//!
//! ## Why the host is validated
//!
//! The code makes the origin by concatenation, so a host is not only data. A
//! `.source` file is a plain file in the home directory of the user, and on a
//! shared machine, possibly a different person wrote it. A `.source` file
//! with `evil.example.com` in a field for a bare host name could otherwise
//! send the client to a different server, or get a plaintext endpoint past
//! the TLS check below. The code accepts only a bare host name or an IP
//! literal.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use eds_sys::{
    E_CLIENT_ERROR_INVALID_ARG, E_CLIENT_ERROR_TLS_NOT_AVAILABLE,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_RESOURCE, E_SOURCE_EXTENSION_SECURITY,
    EClientError, ESource, ESourceAuthentication, ESourceResource, ESourceSecurity,
    e_client_error_create, e_source_authentication_dup_host, e_source_authentication_dup_method,
    e_source_authentication_dup_user, e_source_authentication_get_port,
    e_source_authentication_get_type, e_source_resource_dup_identity, e_source_resource_get_type,
    e_source_security_get_secure, e_source_security_get_type,
};
use eds_sys::{EBackend, e_backend_get_source, e_backend_get_type};
use glib_sys::GError;

use crate::error::cstring_lossy;
use crate::i18n::{translate, translate_with};
use crate::marshal::{extension_if_present, take_string};

/// The `[Authentication]` and `[Security]` fields of an account.
///
/// An absent group gives the defaults of a new, empty extension, except for
/// `secure`. See [`read_auth`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFields {
    pub host: Option<String>,
    /// 0 means "not set", as in the keyfile.
    pub port: u16,
    pub user: Option<String>,
    pub method: Option<String>,
    pub secure: bool,
}

/// Reads the `[Authentication]` and `[Security]` fields of `source`.
///
/// This function adds no group to the source. `e_source_get_extension`
/// creates a group that it cannot find, and a reader that the source owner
/// does not expect must not change the account file. An empty string is
/// `None`, as [`take_string`] gives.
///
/// An absent `[Security]` group means TLS. `ESourceSecurity:secure` defaults
/// to FALSE, so a read of a new extension cannot tell "the keyfile has no
/// `[Security]` group" from "the user disabled TLS". Plain HTTP for the first
/// case would silently downgrade each hand-written account.
///
/// This can run on a worker thread while the main loop frees an old string
/// in a setter. Thus each string read uses a `dup_` getter.
///
/// # Safety
///
/// `source` must be a valid `ESource` that stays alive for the call. This
/// function only reads it.
pub unsafe fn read_auth(source: *mut ESource) -> AuthFields {
    // `e_source_has_extension` finds an extension through the registered
    // children of `E_TYPE_SOURCE_EXTENSION`. A reference to the GType
    // registers a type that nothing in this process used yet.
    // SAFETY: no arguments, and the type system initializes itself.
    unsafe {
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // SAFETY: the source is valid by this function's contract, and the name
    // is a header constant.
    let secure = match unsafe {
        extension_if_present::<ESourceSecurity>(source, E_SOURCE_EXTENSION_SECURITY)
    } {
        None => true,
        // SAFETY: a live extension, by `extension_if_present`'s contract.
        Some(security) => unsafe { e_source_security_get_secure(security) != 0 },
    };

    // SAFETY: as above.
    let Some(auth) = (unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }) else {
        return AuthFields {
            host: None,
            port: 0,
            user: None,
            method: None,
            secure,
        };
    };
    // SAFETY: a live extension. Each `dup_` getter gives NULL or a copy that
    // `take_string` frees.
    unsafe {
        AuthFields {
            host: take_string(e_source_authentication_dup_host(auth)),
            port: e_source_authentication_get_port(auth),
            user: take_string(e_source_authentication_dup_user(auth)),
            method: take_string(e_source_authentication_dup_method(auth)),
            secure,
        }
    }
}

/// What a backend needs from its `ESource` in order to build a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceConfig {
    /// Where to fetch the session document from: an explicit endpoint, or a
    /// bare domain eligible for `_jmap._tcp` SRV autodiscovery before the
    /// `.well-known/jmap` fallback. See [`connect`] for what this becomes.
    pub target: ConnectTarget,
    /// The user name to authenticate as, if the source names one.
    pub user: Option<String>,
    /// The JMAP object that this source stands for: an address book id in
    /// the address book backend, a calendar id in the calendar backend. It is
    /// one keyfile field with two meanings, so its name comes from the field,
    /// not from a meaning. Absent means "the default of the account", which
    /// the backend resolves at connect time.
    ///
    /// Held verbatim: a collection that a different user shares with this
    /// user is in the account of that user, and its source writes the field
    /// as `<accountId>/<collectionId>`. Read it through [`Self::account_id`]
    /// and [`Self::collection_id`], which split it.
    pub resource_id: Option<String>,
    /// Whether this account opts into rebasing the advertised URLs of the
    /// session onto the origin of the actual connection (see
    /// [`crate::rebase::rebase_urls`]). `false` for each account written
    /// before that setting existed.
    pub rebase_urls: bool,
}

/// A source that cannot become a connection.
///
/// These are configuration faults, not server faults: a retry does not help,
/// and the offline cache of the meta backend is not the correct answer. Thus
/// they go to Evolution as their own `E_CLIENT_ERROR` codes, not through the
/// client error mapping in [`crate::error`].
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
            Self::MissingHost => f.write_str(&translate(
                // TRANSLATORS: shown when an account's Authentication
                // extension names no host to connect to.
                c"the account does not name a JMAP server",
            )),
            Self::InvalidHost(host) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is the host name the account named.
                c"\"%1$s\" is not a valid server host name",
                &[host.as_str()],
            )),
            Self::InsecureTransport(host) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is the host name the account named.
                c"refusing to contact %1$s without TLS: credentials and contacts would go over the network in the clear",
                &[host.as_str()],
            )),
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
        // SAFETY: the code is one of the enum's own values, and the call
        // copies the message.
        unsafe { e_client_error_create(self.client_error_code(), message.as_ptr()) }
    }
}

impl SourceConfig {
    /// The JMAP account that contains the collection, if the source names
    /// one. `None` means the primary account for the capability of the
    /// backend.
    ///
    /// The account is in `[Resource] Identity`, not in a key of its own,
    /// because that field is a registered `ESource` property. EDS writes it
    /// to the `.source` keyfile, so it gets to the address book or calendar
    /// factory, which is a different process from the registry that created
    /// the source. JMAP ids use only `A-Za-z0-9-_` (RFC 8620 §1.2), so a `/`
    /// cannot be part of either half.
    pub fn account_id(&self) -> Option<&str> {
        self.split_resource().0
    }

    /// The address book or calendar id, without any account prefix; `None`
    /// means "the account's default", as for [`Self::resource_id`].
    pub fn collection_id(&self) -> Option<&str> {
        self.resource_id.as_deref().map(|_| self.split_resource().1)
    }

    /// `[Resource] Identity` as (account, collection). If the value is not
    /// exactly two non-empty halves, it stays whole as a collection id. Thus
    /// the server refuses it under the name in the keyfile, and the backend
    /// does not guess which part is correct.
    fn split_resource(&self) -> (Option<&str>, &str) {
        let resource = self.resource_id.as_deref().unwrap_or_default();
        match resource.split_once('/') {
            Some((account, collection))
                if !account.is_empty() && !collection.is_empty() && !collection.contains('/') =>
            {
                (Some(account), collection)
            }
            _ => (None, resource),
        }
    }

    /// Reads `source`'s JMAP account settings.
    ///
    /// # Safety
    ///
    /// `source` must be a valid `ESource`, i.e. the one EDS gave the backend.
    /// This function only reads it, and nothing outlives the call.
    pub unsafe fn from_source(source: *mut ESource) -> Result<Self, SourceError> {
        // SAFETY: the source is valid for the whole call, by this function's
        // own contract.
        let auth = unsafe { read_auth(source) };

        // `e_source_has_extension` cannot find an extension type that nothing
        // has referenced yet. A reference to the GType registers it.
        // SAFETY: no arguments, and the type system initializes itself.
        unsafe { e_source_resource_get_type() };
        // Use the same `extension_if_present` guard as `read_auth`. A source
        // read for diagnostics or validation must not get an empty
        // `[Resource]` group only because something read it.
        // SAFETY: the source is valid for the whole call and the name is a
        // header constant.
        let resource =
            unsafe { extension_if_present::<ESourceResource>(source, E_SOURCE_EXTENSION_RESOURCE) };
        // This runs on a worker thread, thus the `dup_` getter.
        // SAFETY: a live extension, by `extension_if_present`'s contract.
        let resource_id = match resource {
            Some(resource) => unsafe { take_string(e_source_resource_dup_identity(resource)) },
            None => None,
        };

        let target = connect_target(auth.host.as_deref(), auth.port, auth.secure)?;

        // SAFETY: `source` is valid for the whole call, by this function's
        // own contract.
        let rebase_urls = unsafe { crate::rebase::rebase_urls(source) };

        Ok(Self {
            target,
            user: auth.user,
            resource_id,
            rebase_urls,
        })
    }
}

/// Where a JMAP client looks for the server: an endpoint that is already
/// concrete, or a bare domain that RFC 8620 §2.2 autodiscovery applies to.
///
/// [`connect_target`] already has the data for this distinction, and would
/// otherwise discard it. A source that names a port states an endpoint. A
/// source that does not is exactly the RFC 8620 §2.2 case "the domain is the
/// entry point". A plain email+password account setup makes this shape:
/// `from_identity` in `jmap-config/src/defaults.rs` writes the bare email
/// domain as `Authentication:Host` with no port, on purpose, per that RFC.
/// See [`connect`] for what each variant does at connect time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectTarget {
    /// Scheme, host and port, with no trailing slash: what
    /// `jmap_client::Client::connect` calls the origin. The client connects
    /// to it directly: SRV autodiscovery does not apply to an exact endpoint.
    Origin(String),
    /// A bare domain, secure and with no port stated: eligible for
    /// `_jmap._tcp.<domain>` SRV autodiscovery before the bare-domain
    /// `https://<domain>/.well-known/jmap` fallback (RFC 8620 §2.2).
    Domain(String),
}

/// Connects to the server that `target` names. For a
/// [`ConnectTarget::Domain`], uses `_jmap._tcp` SRV autodiscovery.
///
/// A [`ConnectTarget::Domain`] is resolved through [`crate::resolver::
/// SystemResolver`], the real `_jmap._tcp` lookup. `jmap-client` has this
/// interface for it, and the "Look Up Account Details" worker of
/// `jmap-config` uses the same one. A record only changes discovery: for a
/// domain that publishes no record, the client uses
/// `https://<domain>/.well-known/jmap`. Each deployment that answers at its
/// own domain needs this fallback.
///
/// The client dials a [`ConnectTarget::Origin`] as stated and never resolves
/// it. SRV autodiscovery answers "where is the JMAP server of this domain",
/// and an explicit endpoint already answers that question.
///
/// `rebase_urls` is the opt-in of the account (see
/// [`crate::rebase::rebase_urls`]). If it is true, or
/// `JMAP_LIVE_SERVER_REBASE_URLS` is `1` or `true`, rebasing is on for this
/// connection. Thus the environment variable stays a process-wide override
/// for the `--features live-server` harness and the committed probes, and
/// each account can opt in or out on its own.
pub fn connect(
    target: &ConnectTarget,
    rebase_urls: bool,
    credentials: jmap_client::Credentials,
) -> Result<jmap_client::Client, jmap_client::Error> {
    let rebase = rebase_urls || jmap_client::rebase_urls_from_env();
    match target {
        ConnectTarget::Origin(origin) => {
            tracing::debug!(origin, rebase, "connecting to JMAP origin");
            jmap_client::Client::builder()
                .rebase_urls_to_origin(rebase)
                .connect(origin, credentials)
        }
        ConnectTarget::Domain(domain) => {
            tracing::debug!(
                domain,
                rebase,
                "connecting to JMAP domain via SRV autodiscovery"
            );
            jmap_client::Client::builder()
                .rebase_urls_to_origin(rebase)
                .resolver(crate::resolver::SystemResolver)
                .connect_domain(domain, credentials)
        }
    }
}

/// Decides the [`ConnectTarget`] for a JMAP client, and refuses the two ways
/// an account can point it to an incorrect destination.
///
/// This is separate from [`SourceConfig::from_source`] because the mail side
/// makes the same decisions from a different place. Camel keeps the server of
/// a service on the `CamelNetworkSettings` interface, not in `ESource`
/// extensions, so `jmap-mail` reads different fields but must get the same
/// answer. The host validation and the TLS rule must not have two copies: a
/// second copy is a second thing to forget to fix.
///
/// `host` is absent or non-empty, as both sides already make it
/// ([`take_string`] on the EDS side, the same normalisation over the empty
/// construct default of Camel on the mail side). `port` is 0 for "not set",
/// which is what an unwritten keyfile key and an unconfigured settings object
/// both read back as.
///
/// [`take_string`]: crate::marshal::take_string
pub fn connect_target(
    host: Option<&str>,
    port: u16,
    secure: bool,
) -> Result<ConnectTarget, SourceError> {
    let host = host.ok_or(SourceError::MissingHost)?;
    // A fully qualified name (one trailing dot, RFC 1035 §3.1) and its
    // relative form are the same DNS entry. Put unchanged into a URL, the
    // dotted form is a host that the server's own redirect does not
    // recognise as itself. The client withholds credentials across that
    // redirect, so the bearer token is lost.
    let host = host.trim_end_matches('.');
    let authority = authority(host)?;
    if !secure && !is_loopback(host) {
        return Err(SourceError::InsecureTransport(host.to_owned()));
    }

    // An IP literal is never a domain for SRV autodiscovery: there is no
    // email-style entry point to resolve, only the given address.
    let is_ip_literal = host.parse::<Ipv4Addr>().is_ok() || host.parse::<Ipv6Addr>().is_ok();
    if port == 0 && secure && !is_ip_literal {
        return Ok(ConnectTarget::Domain(host.to_owned()));
    }

    let scheme = if secure { "https" } else { "http" };
    Ok(ConnectTarget::Origin(match port {
        // The keyfile writes 0 for "not set". Without a port in the origin,
        // the default port of the scheme applies, not port 0.
        0 => format!("{scheme}://{authority}"),
        // A stated default port is also removed: RFC 6454 §6.2 origins have
        // no default port, and RFC 8414 §3.3 compares issuers byte-for-byte.
        // An SRV record that names port 443 explicitly (as Fastmail's does)
        // must still give the issuer that its metadata states. If not, OAuth
        // discovery can never validate. This does not change the connection.
        443 if secure => format!("{scheme}://{authority}"),
        80 if !secure => format!("{scheme}://{authority}"),
        port => format!("{scheme}://{authority}:{port}"),
    }))
}

/// The host and port that the network-reachability monitor of EDS watches
/// for this account (the job of `EBackendClass::get_destination_address`).
///
/// Read directly from `[Authentication]`, the same fields that
/// [`SourceConfig::from_source`] reads, but without the host validation or
/// TLS refusal of [`connect_target`]. A malformed or insecure-mode host is
/// still a real host to watch, and this value is only advisory: nothing here
/// dials it. This mirrors the fallback branch of evolution-ews's own
/// `ews_backend_get_destination_address`, which evolution-ews uses when it
/// has no connection-URL setting to parse. A JMAP account never has such a
/// setting. One addition to that literal read: a port of 0 becomes 443.
/// Port 0 is the "not set" value of EDS, and each bare-domain SRV-eligible
/// account has it. 443 is the default port of a JMAP connection (the default
/// of `connect_target` for this case), and a monitor must not watch port 0.
///
/// # Safety
///
/// `source` must be a valid `ESource` that stays alive for the call. This
/// function only reads it.
pub unsafe fn destination_address(source: *mut ESource) -> Option<(String, u16)> {
    // `read_auth` adds no group: a source read for reachability monitoring,
    // which the caller does not own, must keep its account file.
    // SAFETY: a valid source, by this function's contract.
    let AuthFields { host, port, .. } = unsafe { read_auth(source) };
    let host = host?;
    if host.is_empty() {
        return None;
    }
    Some((host, if port == 0 { 443 } else { port }))
}

/// The `ESource` for which EDS constructed `backend`, or NULL if `backend` is
/// not an `EBackend`.
///
/// `e_backend_get_source` almost does this check already, through
/// `g_return_val_if_fail (E_IS_BACKEND (backend))`. The GLib type check
/// returns FALSE for a NULL instance or an instance with no class, and does
/// not dereference it. But it also logs a critical. That is correct for a
/// real bug and incorrect for the callers that ask for a valid reason: a
/// vfunc that runs on an instance that EDS has not fully constructed, and the
/// `detached()` test instances of the backend crates, which are deliberately
/// not GObjects. A GObject check first changes "is there a source to use"
/// into a usual NULL check.
///
/// # Safety
///
/// `backend` must be NULL, a valid `GTypeInstance`, or a pointer to memory
/// whose leading `GTypeInstance` field is NULL (a zeroed instance struct
/// satisfies this). The returned `ESource` is borrowed, not owned.
pub unsafe fn backend_source(backend: *mut EBackend) -> *mut ESource {
    // SAFETY: `g_type_check_instance_is_a` itself checks for a NULL instance
    // and a NULL class, so this is legal for the callers above. The type
    // function takes no arguments.
    let is_backend = unsafe {
        gobject_sys::g_type_check_instance_is_a(backend.cast(), e_backend_get_type()) != 0
    };
    if !is_backend {
        return std::ptr::null_mut();
    }
    // SAFETY: GObject has just confirmed this is an `EBackend`.
    unsafe { e_backend_get_source(backend) }
}

/// Makes the origin string that [`connect_target`] would connect to, for a
/// caller that wants only the endpoint and not the SRV-eligibility
/// distinction: `Server::origin` in `jmap-backend-collection`, a display
/// value that child sources copy and that nothing connects to directly.
pub fn origin(host: Option<&str>, port: u16, secure: bool) -> Result<String, SourceError> {
    Ok(match connect_target(host, port, secure)? {
        ConnectTarget::Origin(origin) => origin,
        ConnectTarget::Domain(domain) => format!("https://{domain}"),
    })
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

/// A host name and nothing else: no scheme, no port, no path, no userinfo.
///
/// Deliberately stricter than RFC 1123: internationalised names get to EDS
/// already in punycode. Any other character in a field that goes into a URL
/// is at best a misconfiguration.
fn is_bare_host_name(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('.')
        && !host.starts_with('-')
        && !host.contains("..")
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Whether plaintext to this host stays on the machine. This includes all
/// hosts that the mock server and a local development instance use, and no
/// others.
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

/// An `ESource` with only a uid and no registry, released on drop. For tests.
///
/// EDS makes a source from a keyfile in the same way, with
/// `e_source_new_with_uid` and no D-Bus object. Thus its extensions behave as
/// they do in a backend, and tests call the real readers, not mocks.
#[cfg(feature = "testing")]
pub struct OwnedSource(crate::owned::Owned<ESource>);

#[cfg(feature = "testing")]
impl OwnedSource {
    /// Makes a source with the uid `uid`. Panics if EDS refuses it.
    #[must_use]
    pub fn new(uid: &str) -> Self {
        let uid = std::ffi::CString::new(uid).expect("no NUL in a test uid");
        let mut error = std::ptr::null_mut();
        // SAFETY: the documented arguments are a NUL-terminated uid, a NULL
        // D-Bus object and a GError out-parameter. The result is `(transfer
        // full)`, and `Owned` takes that one reference.
        let source = unsafe {
            crate::owned::Owned::from_raw(eds_sys::e_source_new_with_uid(
                uid.as_ptr(),
                std::ptr::null_mut(),
                &mut error,
            ))
        };
        Self(source.expect("e_source_new_with_uid failed"))
    }

    /// The source, still owned here.
    #[must_use]
    pub fn as_ptr(&self) -> *mut ESource {
        self.0.as_ptr()
    }
}

/// A `.source` file loaded as `evolution-source-registry` loads it, released on
/// drop. For tests.
///
/// `e_server_side_source_new` needs a registry server to report changes to.
/// A server that does not run owns no bus name and reads no user sources.
#[cfg(feature = "testing")]
pub struct RegistrySource {
    // Declared first, so it is released before the server.
    source: crate::owned::Owned<ESource>,
    _server: crate::owned::Owned<eds_sys::ESourceRegistryServer>,
}

#[cfg(feature = "testing")]
impl RegistrySource {
    /// Parses the keyfile at `path`. Panics with the GError message if EDS refuses it.
    #[must_use]
    pub fn load(path: &std::path::Path) -> Self {
        let path_string =
            std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("no NUL in a path");
        let mut error = std::ptr::null_mut();
        // SAFETY: the constructor takes no arguments and returns a new
        // reference. Its declared return type is the base class, `EDBusServer
        // *`; C does this downcast implicitly. The path is NUL-terminated and
        // copied. The GFile and the source are `(transfer full)`.
        let (server, source) = unsafe {
            let server = crate::owned::Owned::from_raw(
                eds_sys::e_source_registry_server_new().cast::<eds_sys::ESourceRegistryServer>(),
            )
            .expect("e_source_registry_server_new failed");
            let file =
                crate::owned::Owned::from_raw(eds_sys::g_file_new_for_path(path_string.as_ptr()))
                    .expect("g_file_new_for_path never fails");
            let source = crate::owned::Owned::from_raw(eds_sys::e_server_side_source_new(
                server.as_ptr(),
                file.as_ptr(),
                &mut error,
            ));
            (server, source)
        };
        let Some(source) = source else {
            // SAFETY: a NULL return means the GError was set.
            let message = unsafe { std::ffi::CStr::from_ptr((*error).message) };
            panic!(
                "the registry could not read {}: {}",
                path.display(),
                message.to_string_lossy()
            );
        };
        Self {
            source,
            _server: server,
        }
    }

    /// The source, still owned here.
    #[must_use]
    pub fn as_ptr(&self) -> *mut ESource {
        self.source.as_ptr()
    }
}

#[cfg(test)]
mod tests {
    use jmap_tracing_capture::capture;

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
    fn source_error_display_names_the_offending_host() {
        assert!(SourceError::MissingHost.to_string().contains("JMAP server"));
        assert!(
            SourceError::InvalidHost("evil.example.com/x".into())
                .to_string()
                .contains("evil.example.com/x")
        );
        assert!(
            SourceError::InsecureTransport("jmap.example.com".into())
                .to_string()
                .contains("jmap.example.com")
        );
    }

    #[test]
    fn a_default_port_only_drops_when_the_scheme_also_matches() {
        // 443 is removed only when the scheme is https (`secure`); over plain
        // HTTP it is a non-default port like all others and must stay.
        assert_eq!(
            origin(Some("localhost"), 443, false).as_deref(),
            Ok("http://localhost:443")
        );
        // 80 is removed only when the scheme is http (`!secure`); over TLS it
        // is a non-default port like all others and must stay.
        assert_eq!(
            origin(Some("jmap.example.com"), 80, true).as_deref(),
            Ok("https://jmap.example.com:80")
        );
    }

    /// The rules that `jmap-mail` uses this function for. `tests/source.rs`
    /// tests them through an `ESource`; this test pins them on the shared
    /// entry point, which has a second caller that does not use an `ESource`.
    /// RFC 8414 §3.3 compares issuers byte-for-byte, and RFC 6454 §6.2
    /// serializes an origin without the default port of its scheme. Thus a
    /// stated default port must not be in the origin. This is not cosmetic:
    /// the `_jmap._tcp` SRV record of Fastmail names port 443 explicitly, but
    /// its authorization-server metadata says
    /// `"issuer": "https://api.fastmail.com"` (no port). An origin that keeps
    /// `:443` can never pass the issuer check, and OAuth autodiscovery
    /// silently finds nothing. Non-default ports must stay, or no local
    /// plaintext server is reachable.
    #[test]
    fn a_default_port_serializes_out_of_the_origin() {
        assert_eq!(
            origin(Some("api.fastmail.com"), 443, true).as_deref(),
            Ok("https://api.fastmail.com")
        );
        assert_eq!(
            origin(Some("localhost"), 80, false).as_deref(),
            Ok("http://localhost")
        );
        assert_eq!(
            origin(Some("jmap.example.com"), 8443, true).as_deref(),
            Ok("https://jmap.example.com:8443")
        );
        assert_eq!(
            origin(Some("localhost"), 8080, false).as_deref(),
            Ok("http://localhost:8080")
        );
    }

    #[test]
    fn the_origin_applies_the_host_rules_whoever_supplies_the_host() {
        assert_eq!(origin(None, 0, true), Err(SourceError::MissingHost));
        assert_eq!(
            origin(Some("evil.example.com/x"), 0, true),
            Err(SourceError::InvalidHost("evil.example.com/x".into()))
        );
        assert_eq!(
            origin(Some("jmap.example.com"), 0, false),
            Err(SourceError::InsecureTransport("jmap.example.com".into()))
        );
        // A port that nobody named is omitted, so the default port of the
        // scheme applies.
        assert_eq!(
            origin(Some("jmap.example.com"), 0, true).as_deref(),
            Ok("https://jmap.example.com")
        );
        assert_eq!(
            origin(Some("::1"), 8080, false).as_deref(),
            Ok("http://[::1]:8080")
        );
    }

    #[test]
    fn a_bare_domain_with_no_port_is_srv_eligible() {
        // A plain email+password setup makes this shape: no port named,
        // secure by default (RFC 8620 §2.2's "the domain is the entry
        // point").
        assert_eq!(
            connect_target(Some("fastmail.com"), 0, true),
            Ok(ConnectTarget::Domain("fastmail.com".into()))
        );
    }

    #[test]
    fn an_explicit_port_is_an_origin_not_a_domain() {
        // Each backend test connects to a mock server through an explicit
        // port. That must continue to connect directly, never through SRV
        // lookup.
        assert_eq!(
            connect_target(Some("jmap.example.com"), 8443, true),
            Ok(ConnectTarget::Origin(
                "https://jmap.example.com:8443".into()
            ))
        );
    }

    #[test]
    fn an_ip_literal_is_an_origin_even_with_no_port() {
        // There is no email-style domain to run SRV autodiscovery against
        // when the account already names a bare address.
        assert_eq!(
            connect_target(Some("203.0.113.5"), 0, true),
            Ok(ConnectTarget::Origin("https://203.0.113.5".into()))
        );
        assert_eq!(
            connect_target(Some("::1"), 0, false),
            Ok(ConnectTarget::Origin("http://[::1]".into()))
        );
    }

    #[test]
    fn a_trailing_dot_host_normalizes_to_the_same_domain() {
        // A fully qualified name (one trailing dot) and its relative form
        // are the same DNS entry (RFC 1035 §3.1). The dotted form, unchanged
        // in a URL, sends `https://api.fastmail.com./...`. Fastmail answers
        // with a redirect to the host without the dot, and that redirect
        // drops the bearer token.
        assert_eq!(
            connect_target(Some("api.fastmail.com."), 0, true),
            Ok(ConnectTarget::Domain("api.fastmail.com".into()))
        );
        assert_eq!(
            connect_target(Some("jmap.example.com."), 8443, true),
            Ok(ConnectTarget::Origin(
                "https://jmap.example.com:8443".into()
            ))
        );
        assert_eq!(
            origin(Some("api.fastmail.com."), 443, true).as_deref(),
            Ok("https://api.fastmail.com")
        );
    }

    #[test]
    fn insecure_and_missing_host_still_refuse_before_the_domain_decision() {
        assert_eq!(connect_target(None, 0, true), Err(SourceError::MissingHost));
        assert_eq!(
            connect_target(Some("jmap.example.com"), 0, false),
            Err(SourceError::InsecureTransport("jmap.example.com".into()))
        );
    }

    #[test]
    fn only_loopback_addresses_count_as_local() {
        for host in ["localhost", "LocalHost", "127.0.0.1", "127.9.9.9", "::1"] {
            assert!(is_loopback(host), "{host} should be loopback");
        }
        // The near-misses that a plausible name could exploit.
        //
        // A resolver *does* send the last four spellings to 127.0.0.1, but
        // this function does not see them as loopback: an IPv4-mapped IPv6
        // literal, the address as one decimal integer, the octal-per-octet
        // form, and the short form. Thus each fails *closed*: plaintext is
        // refused and TLS is necessary. That is the safe direction for a
        // check that decides if credentials can go over the network in the
        // clear. This test pins it, so that "loosening this to be helpful"
        // must be a deliberate act.
        for host in [
            "localhost.example.com",
            "notlocalhost",
            "10.0.0.1",
            "0.0.0.0",
            "127.0.0.1.example.com",
            "::2",
            "::ffff:127.0.0.1",
            "2130706433",
            "0177.0.0.1",
            "127.1",
        ] {
            assert!(!is_loopback(host), "{host} should not be loopback");
        }
    }

    #[test]
    fn connect_traces_origin_field_when_connecting_to_origin() {
        let target = ConnectTarget::Origin("http://127.0.0.1:1".to_owned());
        let (_, entries) = capture(|| connect(&target, false, jmap_client::Credentials::none()));

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "origin" && value == "http://127.0.0.1:1"),
            "expected origin field, got {entries:?}"
        );
    }

    #[test]
    fn connect_traces_domain_field_when_connecting_to_domain() {
        let target = ConnectTarget::Domain("invalid.domain.invalid".to_owned());
        let (_, entries) = capture(|| connect(&target, false, jmap_client::Credentials::none()));

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "domain" && value == "invalid.domain.invalid"),
            "expected domain field, got {entries:?}"
        );
    }

    #[test]
    fn connect_traces_rebase_true_when_the_source_opts_in_even_with_no_env_var() {
        // SAFETY: in this process, only the tests in this module touch this
        // variable. The `connect` tests read it, and the two `rebase` tests
        // remove it; no test sets it.

        unsafe { std::env::remove_var("JMAP_LIVE_SERVER_REBASE_URLS") };

        let target = ConnectTarget::Origin("http://127.0.0.1:1".to_owned());
        let (_, entries) = capture(|| connect(&target, true, jmap_client::Credentials::none()));

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "rebase" && value == "true"),
            "expected the per-source opt-in to turn rebasing on, got {entries:?}"
        );
    }

    #[test]
    fn connect_traces_rebase_false_when_the_source_does_not_opt_in() {
        // SAFETY: as above.
        unsafe { std::env::remove_var("JMAP_LIVE_SERVER_REBASE_URLS") };

        let target = ConnectTarget::Origin("http://127.0.0.1:1".to_owned());
        let (_, entries) = capture(|| connect(&target, false, jmap_client::Credentials::none()));

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "rebase" && value == "false"),
            "expected no opt-in and no env var to leave rebasing off, got {entries:?}"
        );
    }
}
