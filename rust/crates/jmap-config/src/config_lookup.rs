// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapConfigLookup`: the `EConfigLookupWorker` behind the "Look Up Account
//! Details" step of Evolution. This worker closes the OAuth 2.0 gap of M7.
//!
//! [`oauth2_service`](crate::oauth2_service) and
//! [`crate::oauth2_setup::discover_and_register`] are both complete. Before this
//! module, nothing called the latter. The manual server-settings page that
//! [`backend::insert_widgets`](crate::backend) builds has no trigger for a
//! blocking network probe. Such a trigger could block the GTK main thread, or
//! need a maintainer decision about which UI asks for it. This module adds
//! neither. It implements `EConfigLookupWorker`, a generic mechanism that the
//! account assistant of Evolution already has for all providers.
//! `e-ews-config-lookup.c` of `evolution-ews` uses the same mechanism for
//! Exchange autodiscovery. The trigger is the "Look Up Account Details" button
//! of the assistant. `EConfigLookup` already runs this worker on a pool thread
//! (`g_thread_pool_push` in `e-config-lookup.c`, read in that file).
//!
//! ## Read against real upstream source
//!
//! `/tmp/evo-src` (the `evolution` checkout of this VM, not EDS) has three
//! `EConfigLookupWorker` implementers under `src/modules/config-lookup/`. This
//! module follows `e-webdav-config-lookup.c` almost line for line:
//! `extensible_type = E_TYPE_CONFIG_LOOKUP` in `class_init`, `constructed`
//! chains up and calls `e_config_lookup_register_worker`, and `run()` gives an
//! `EConfigLookupResultSimple` to `e_config_lookup_add_result`.
//!
//! `e-config-lookup.c`/`.h` and `e-config-lookup-result-simple.c`/`.h` (same
//! tree) set the ownership rules. `e_config_lookup_result_simple_new` is a
//! plain `g_object_new` (transfer full to its caller). Each `_add_*` copies the
//! string or value it gets. The doc comment of `e_config_lookup_add_result`
//! says that the `EConfigLookup` "assumes ownership of the result and frees it
//! when no longer needed". Thus this crate builds a result, fills it, gives it
//! away, and never unrefs it.
//!
//! ## The redirect URI, and a wrong guess corrected before release
//!
//! An earlier plan set `redirect_uri` to the RFC 8252 out-of-band URN,
//! `"urn:ietf:wg:oauth:2.0:oob"`. The EDS `libedataserverui`/`libedataserver`
//! source shows that this guess is wrong for *this* prompter. (The source came
//! from `gitlab.gnome.org/GNOME/evolution-data-server`, `master`. This VM has
//! no local EDS checkout, only one of `evolution`.)
//!
//! - `ECredentialsPrompterImplOAuth2` never compares the browser's navigation
//!   with the redirect URI.
//! - The default of `get_authentication_policy` (`e-oauth2-service.c`) is
//!   always `ALLOW`.
//! - The prompter extracts the authorization code from each URI on which the
//!   embedded `WebKitWebView` gets to `WEBKIT_LOAD_FINISHED`.
//!   `e_oauth2_service_util_extract_from_uri` parses `code=`/`error=` from the
//!   query or fragment of any URI, with no prefix check.
//!
//! Thus WebKit must *finish* (not fail) that navigation. The
//! `e-oauth2-service-google.c` that EDS ships does this with a private-use URI
//! *scheme* (`«reversed client id»:/oauth2redirect`), not the OOB URN. This is
//! a real precedent: Google account setup works today with this prompter.
//! [`REDIRECT_URI`] follows that precedent.
//!
//! ## Where the live dispatch of `run()` is proven
//!
//! Only a real `EConfigLookup` calls `run()`. `e_config_lookup_new` does not
//! construct one without a live `ESourceRegistry` (see the
//! `g_return_val_if_fail` in `e-config-lookup.c`). Thus the live dispatch needs
//! the `dbus-run-session` environment of the M9 functional harness
//! (`cmake/Functional.cmake`). `cargo test` alone does not have it.
//!
//! - The unit tests call [`probe_host`] directly, because it needs none of that.
//! - The FFI shell below is built and registered the same way as
//!   [`crate::oauth2_service::Service`].
//! - `jmap-functional/tests/config-lookup.rs` drives its dispatch through a
//!   real `EConfigLookup`. This is the M9 layer-1 coverage that
//!   `evo-sys/tests/config_lookup.rs` named as the next increment. It is now
//!   done.
//!
//! This dispatch was also run once by hand, outside the test suite. A scratch
//! C program linked `evolution-shell-3.0` (`EConfigLookup` is in `e-util`, not
//! in an EDS library) and loaded this module with
//! `e_module_load_all_in_directory`. Under `dbus-run-session`, it constructed a
//! real `ESourceRegistry`/`EConfigLookup` and ran it against a
//! `jmap-mockd --oauth2` instance. It produced exactly one positive, complete
//! result. This shows that the mechanism works end to end. It also shows that a
//! headless client needs `LD_LIBRARY_PATH=/usr/lib/evolution`: the transitive
//! `libevolution-mail.so` dependency of the module is not on the default loader
//! path. This run is not a standing test, and it does not replace the
//! automated harness above.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    EExtension, ENamedParameters, ESource, ESourceCollection, e_extension_get_extensible,
    e_named_parameters_get, e_source_backend_dup_backend_name, e_source_collection_get_type,
};
use evo_sys::{
    E_CONFIG_LOOKUP_PARAM_EMAIL_ADDRESS, E_CONFIG_LOOKUP_PARAM_SERVERS,
    E_CONFIG_LOOKUP_RESULT_COLLECTION, E_CONFIG_LOOKUP_RESULT_PRIORITY_IMAP,
    E_CONFIG_LOOKUP_SOURCE_COLLECTION, EConfigLookup, EConfigLookupResult, EConfigLookupWorker,
    EConfigLookupWorkerInterface, e_config_lookup_add_result, e_config_lookup_get_source,
    e_config_lookup_get_type, e_config_lookup_register_worker,
    e_config_lookup_result_simple_add_boolean, e_config_lookup_result_simple_add_string,
    e_config_lookup_result_simple_add_uint, e_config_lookup_result_simple_new,
    e_config_lookup_worker_get_type,
};
use gio_sys::GCancellable;
use glib_sys::{GTRUE, GType, gchar, gint};
use gobject_sys::{g_object_ref, g_object_unref};
use jmap_backend_core::cancel::CancelBridge;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::i18n::{self, N_, translate_with};
use jmap_backend_core::marshal::{extension_if_present, take_string};
use jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD;
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl};
use jmap_backend_core::trampoline::guard;
use jmap_client::transport::UreqTransport;

use crate::account::{self, Account, BACKEND_NAME};
use crate::oauth2::{self, Config};
use crate::oauth2_setup::discover_and_register;

/// The private-use URI scheme that this crate registers with RFC 7591 and that
/// the OAuth2 service vtable's `get_redirect_uri` callback returns, via
/// [`crate::oauth2::redirect_uri`].
///
/// The module docs tell why this works with the WebKit consent prompter of EDS,
/// and the RFC 8252 out-of-band URN does not.
///
/// It is dotted reverse-DNS, not a bare word. Some providers (see the OAuth doc
/// of Fastmail) refuse to register a private-use redirect scheme without a dot.
pub const REDIRECT_URI: &str = "org.gnome.evolution.jmap:/redirect";

/// The priority of the one result that a successful lookup adds.
///
/// A lower value is a higher priority. This value must be STRICTLY better than
/// PRIORITY_IMAP and PRIORITY_SMTP of `e-config-lookup-result.h` (both 1000).
/// The page selects a backend only on `priority < selected`, thus a tie loses
/// to candidate order. Live observation (2026-08-23, Fastmail): at 1000 the
/// receiving page stayed on the imapx of ISPDB, but the sending page changed
/// to jmap. The result was a broken hybrid.
///
/// This result comes from the SRV record of the provider and a completed
/// RFC 7591 registration. Thus it is the answer of the provider, not a generic
/// guess. It is better than the ISPDB result by the same margin that the
/// Exchange autodiscovery of evolution-ews uses.
const PRIORITY: gint = E_CONFIG_LOOKUP_RESULT_PRIORITY_IMAP as gint - 100;

/// The host for RFC 8414 discovery. A positive result also writes it as
/// `[Authentication] host`.
///
/// The fallback is a bare domain, not a guessed subdomain, for the same reason
/// that `defaults.rs` gives one to the manual setup page. When the account
/// connects, the JMAP discovery (RFC 8620 §2.2) finds all else that
/// `.well-known/jmap` names.
///
/// If `servers` is given, it names the host explicitly and has priority. This
/// is the purpose of [`E_CONFIG_LOOKUP_PARAM_SERVERS`]. Only its first entry is
/// tried. CalDAV/CardDAV autodiscovery (`e-webdav-config-lookup.c`) tries each
/// listed server, because a server can be there on some hosts and not on
/// others. A JMAP deployment names exactly one issuer, thus a second entry
/// is a different server, not a fallback for the first. An explicit entry is
/// not a bare domain, thus `resolver` never gets it. SRV autodiscovery
/// (RFC 8620 §2.2) is for the email-domain fallback only.
///
/// Before it returns the bare domain, the email-domain fallback asks `resolver`
/// for a `_jmap._tcp.<domain>` SRV record (see `jmap_client::resolver`).
/// `jmap_client::ClientBuilder::connect_domain` uses the same seam and the same
/// fallback order. Thus this function also finds a deployment that only SRV
/// publishes (Fastmail), not only a `jmap_client` session fetch.
///
/// `discover_and_register` cannot use the return value directly. A `servers`
/// entry or an SRV target can contain a scheme and a port, which
/// `parse_target` reads. An SRV target has the form `host:port`.
/// `parse_target` reads that as a bare, secure host with that port, which is
/// correct for each target of a JMAP SRV record.
pub fn probe_host(
    email_address: &str,
    servers: Option<&str>,
    resolver: &dyn jmap_client::resolver::Resolver,
) -> Option<String> {
    if let Some(servers) = servers
        && let Some(first) = servers.split(';').map(str::trim).find(|s| !s.is_empty())
    {
        tracing::debug!(
            explicit_server = first,
            "using explicit server for JMAP config lookup"
        );
        return Some(first.to_owned());
    }
    let (_, domain) = email_address.split_once('@')?;
    if domain.is_empty() {
        return None;
    }
    if let Some(target) = resolver.lookup_srv(domain) {
        tracing::debug!(
            domain,
            target_host = target.host,
            target_port = target.port,
            "found JMAP SRV record for domain"
        );
        return Some(format!("{}:{}", target.host, target.port));
    }
    tracing::debug!(domain, "no SRV record found, falling back to bare domain");
    Some(domain.to_owned())
}

/// The address for the answer of [`probe_host`]: a bare host, an explicit port
/// (0 if none), and whether to use TLS.
struct Target {
    host: String,
    port: u16,
    secure: bool,
}

/// Reads a `scheme://host:port` override from the answer of [`probe_host`].
///
/// Upstream `e-webdav-config-lookup.c` uses the same convention for its
/// `servers` values (read in the copy in `/tmp/evo-src`). A bare host gets
/// HTTPS and no explicit port. This is correct for the email-domain fallback,
/// which is always a bare domain. An `http://`/`https://` prefix, with an
/// optional `:port` suffix, overrides both. Only with this override can a
/// `servers` entry name a local, plaintext test deployment on a non-default
/// port. The email-domain path never needs it.
///
/// A bare, unbracketed IPv6 literal (more than one colon, no `[...]`) stays
/// whole. [`jmap_backend_core::source`] accepts a host in that shape. A split
/// on the last `:` would read the last group as a port. A bracketed literal
/// (RFC 3986, `[::1]:8443`) is unwrapped to the same bare shape, and an
/// optional `:port` after it is read separately. In the unbracketed case, the
/// colons of the host would defeat the `!bare_host.contains(':')` guard below.
fn parse_target(host: &str) -> Option<Target> {
    let (secure, authority) = match host.strip_prefix("https://") {
        Some(rest) => (true, rest),
        None => match host.strip_prefix("http://") {
            Some(rest) => (false, rest),
            None => (true, host),
        },
    };
    if authority.is_empty() {
        return None;
    }
    let (bare_host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (bracketed_host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(port) => port.parse().ok()?,
            None if after.is_empty() => 0,
            None => return None,
        };
        (bracketed_host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((bare_host, port)) if !bare_host.contains(':') => (bare_host, port.parse().ok()?),
            _ => (authority, 0),
        }
    };
    if bare_host.is_empty() {
        return None;
    }
    Some(Target {
        host: bare_host.to_owned(),
        port,
        secure,
    })
}

/// The server that a lookup probes first when it runs again, in the `servers`
/// shape that [`parse_target`] reads.
///
/// It is the host, port and TLS choice of the JMAP account, as a previous
/// lookup or the server settings page wrote them. evolution-ews starts its
/// autodiscovery from the prior settings of the account in the same way.
///
/// The result is `None` (thus the probe uses the email-domain path of
/// [`probe_host`]), unless all of these conditions are true:
///
/// - The account is a JMAP account (`backend_name`). The assistant gives the
///   same collection to all workers, and the host of a different backend is
///   not a host for JMAP discovery.
/// - Its identity is the address of the lookup. A user who changed the address
///   wants discovery for the new domain, not the old host.
/// - Its host is not the bare email domain. The server page shows that domain
///   by default. A direct probe of it would skip the `_jmap._tcp` SRV lookup,
///   which is the only way to find a provider such as Fastmail.
fn prior_server(email: &str, backend_name: Option<&str>, account: &Account) -> Option<String> {
    if backend_name != Some(BACKEND_NAME) || !account.identity.eq_ignore_ascii_case(email) {
        return None;
    }
    let host = account.connection.host.as_str();
    let domain = email.rsplit_once('@').map(|(_, domain)| domain);
    if host.is_empty() || domain.is_some_and(|domain| domain.eq_ignore_ascii_case(host)) {
        return None;
    }
    let scheme = if account.connection.secure {
        "https"
    } else {
        "http"
    };
    // RFC 3986: an IPv6 literal is bracketed so its colons are not read as a
    // port separator.
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Some(match account.connection.port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// [`prior_server`] for the collection source that
/// `e_config_lookup_get_source` returns.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource`. This function only reads it.
unsafe fn prior_server_of(source: *mut ESource, email: &str) -> Option<String> {
    if source.is_null() {
        return None;
    }
    // The main thread owns the source and this runs on a worker thread. Thus
    // hold a reference during the read.
    // SAFETY: a valid source, and the check above makes sure it is not NULL.
    unsafe { g_object_ref(source.cast()) };
    // SAFETY: the source is valid and has a reference until the unref below.
    let server = unsafe { prior_server_of_held(source, email) };
    // SAFETY: releases the reference that the code above took.
    unsafe { g_object_unref(source.cast()) };
    server
}

/// [`prior_server_of`] for a source on which the caller holds a reference.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
unsafe fn prior_server_of_held(source: *mut ESource, email: &str) -> Option<String> {
    // SAFETY: no arguments. This registers the extension type that the lookup
    // below needs, as `account::read` does for its types.
    unsafe { e_source_collection_get_type() };
    // SAFETY: a valid source by this function's contract, and a header constant
    // that names an extension of the type that the code reads.
    let collection = unsafe {
        extension_if_present::<ESourceCollection>(source, E_SOURCE_EXTENSION_COLLECTION)
    }?;
    // SAFETY: a live extension, and an `ESourceCollection` is an
    // `ESourceBackend`. The string is NULL or a copy that this call owns.
    let backend_name = unsafe { take_string(e_source_backend_dup_backend_name(collection.cast())) };
    // SAFETY: a valid source, as above.
    let account = unsafe { account::read(source) };
    prior_server(email, backend_name.as_deref(), &account)
}

/// The account lookup worker. It has no state: all else that it needs is
/// `'static` or comes through the arguments of `run()`.
pub struct ConfigLookup;

/// The registered `JmapConfigLookup` type.
pub type JmapConfigLookup = ExtensionObject<ConfigLookup>;

impl Extension for ConfigLookup {
    const TYPE_NAME: &'static CStr = c"JmapConfigLookup";
    // `e_webdav_config_lookup_class_init` sets the same `extensible_type`.
    const EXTENSIBLE_NAME: &'static CStr = c"EConfigLookup";

    fn extensible_type() -> GType {
        // SAFETY: no arguments, and the type initializes itself.
        unsafe { e_config_lookup_get_type() }
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        vec![InterfaceDecl::filled_by::<Vtable>()]
    }

    unsafe fn constructed(extension: *mut EExtension) {
        // SAFETY: the contract of this method.
        unsafe { constructed(extension) }
    }
}

/// Registers this instance with the `EConfigLookup` that it extends, as
/// `e_webdav_config_lookup_constructed` does after its chain-up.
///
/// The registration occurs when each `EConfigLookup` is constructed, not at
/// module load. Different from [`crate::backend::JmapConfigServiceBackend`],
/// this type needs a live instance of the extended type to register *against*.
unsafe fn constructed(extension: *mut EExtension) {
    unsafe {
        // SAFETY: `extension` is live. An `EExtensible`
        // interface pointer has the same address as its instance (GObject
        // interfaces have no adjusted `this`). Thus this cast is the Rust
        // equivalent of the C `E_CONFIG_LOOKUP()` macro on the return value
        // of `e_extension_get_extensible`.
        let config_lookup = e_extension_get_extensible(extension).cast();
        e_config_lookup_register_worker(config_lookup, extension.cast::<EConfigLookupWorker>());
    }
}

/// Fills the copy of `EConfigLookupWorkerInterface` for [`JmapConfigLookup`].
struct Vtable;

// SAFETY: `EConfigLookupWorkerInterface` is the `#[repr(C)]` bindgen binding
// of the interface struct that `e_config_lookup_worker_get_type` names, and it
// starts with `GTypeInterface`. `evo-sys/tests/config_lookup.rs` pins the shape
// of the interface, and that the two slots have no default.
unsafe impl InterfaceImpl for Vtable {
    type Vtable = EConfigLookupWorkerInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initializes itself.
        unsafe { e_config_lookup_worker_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: the contract of `InterfaceImpl::interface_init`: this is the
        // copy of the vtable for this type, and nothing else can get to it yet.
        let vtable = unsafe { &mut *vtable };
        vtable.get_display_name = Some(get_display_name);
        vtable.run = Some(run);
    }
}

/// The label that the results list of the assistant shows while this worker
/// runs.
///
/// This is not the `display_name` of [`EConfigLookupResult`]. [`run`] sets that
/// for each result, and the user selects from those.
unsafe extern "C" fn get_display_name(_lookup_worker: *mut EConfigLookupWorker) -> *const gchar {
    guard("JmapConfigLookup::get_display_name", ptr::null(), || {
        i18n::translate_static(N_(c"Look up JMAP account details"))
    })
}

/// Reads a string parameter from `params`, or `None` if it is not set (NULL
/// from [`e_named_parameters_get`]).
///
/// # Safety
///
/// `params` must be NULL or a valid `ENamedParameters`, valid for the call.
unsafe fn param(params: *const ENamedParameters, name: &CStr) -> Option<String> {
    // SAFETY: from this function's contract.
    let value = unsafe { e_named_parameters_get(params, name.as_ptr()) };
    (!value.is_null()).then(|| {
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    })
}

/// Adds `value`, if present, as `extension_name`/`property_name` on `result`.
///
/// [`e_config_lookup_result_simple_add_string`] copies the string. Thus only
/// the `CString` must stay alive, and only during the call.
unsafe fn add_optional_string(
    result: *mut EConfigLookupResult,
    extension_name: &CStr,
    property_name: &CStr,
    value: Option<&str>,
) {
    if let Some(value) = value {
        let value = cstring_lossy(value);
        // SAFETY: `result` is a live `EConfigLookupResultSimple` by this
        // function's contract. The two names and the value are valid,
        // NUL-terminated strings during the call.
        unsafe {
            e_config_lookup_result_simple_add_string(
                result,
                extension_name.as_ptr(),
                property_name.as_ptr(),
                value.as_ptr(),
            )
        };
    }
}

/// Builds the one [`EConfigLookupResult`] of a successful discovery and gives
/// it to `config_lookup`.
///
/// The result is a complete JMAP account. `[Collection]`/`[Authentication]`/
/// `[Security]` get the same values as from [`crate::account::apply`], and
/// `[JMAP OAuth2]` gets the same values as from [`crate::oauth2::apply`].
/// After the call, `config_lookup` owns the result (see the module docs on the
/// transfer-full contract of `e_config_lookup_add_result`).
///
/// # Safety
///
/// `config_lookup` must be a valid `EConfigLookup`.
unsafe fn add_result(
    config_lookup: *mut EConfigLookup,
    email: &str,
    target: &Target,
    config: &Config,
) {
    let protocol = cstring_lossy(BACKEND_NAME);
    let description = cstring_lossy(&translate_with(
        // TRANSLATORS: %1$s is the server this account was discovered
        // against, shown in the account assistant's list of lookup results.
        c"JMAP account at %1$s, using OAuth 2.0",
        &[target.host.as_str()],
    ));
    let email_c = cstring_lossy(email);
    let host_c = cstring_lossy(&target.host);
    let security_method = if target.secure { c"tls" } else { c"none" };

    // SAFETY: each string argument is the pointer of a live, NUL-terminated
    // `CString`, valid during this call. `_new` copies each of them (see
    // `e-config-lookup-result-simple.c`) and returns a new, owned
    // `EConfigLookupResult` (see the module docs).
    let result = unsafe {
        e_config_lookup_result_simple_new(
            E_CONFIG_LOOKUP_RESULT_COLLECTION,
            PRIORITY,
            GTRUE,
            protocol.as_ptr(),
            i18n::translate_static(N_(c"JMAP account (OAuth 2.0)")),
            description.as_ptr(),
            ptr::null(),
        )
    };

    // SAFETY: the code above constructs `result`, and nothing else has it yet.
    // Each extension/property name is `'static`, and each value is a live
    // `CString` during its call.
    unsafe {
        e_config_lookup_result_simple_add_string(
            result,
            E_SOURCE_EXTENSION_COLLECTION.as_ptr(),
            c"backend-name".as_ptr(),
            protocol.as_ptr(),
        );
        e_config_lookup_result_simple_add_string(
            result,
            E_SOURCE_EXTENSION_COLLECTION.as_ptr(),
            c"identity".as_ptr(),
            email_c.as_ptr(),
        );
        for property in [c"mail-enabled", c"contacts-enabled", c"calendar-enabled"] {
            e_config_lookup_result_simple_add_boolean(
                result,
                E_SOURCE_EXTENSION_COLLECTION.as_ptr(),
                property.as_ptr(),
                GTRUE,
            );
        }
        e_config_lookup_result_simple_add_string(
            result,
            E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr(),
            c"host".as_ptr(),
            host_c.as_ptr(),
        );
        if target.port != 0 {
            e_config_lookup_result_simple_add_uint(
                result,
                E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr(),
                c"port".as_ptr(),
                u32::from(target.port),
            );
        }
        e_config_lookup_result_simple_add_string(
            result,
            E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr(),
            c"user".as_ptr(),
            email_c.as_ptr(),
        );
        e_config_lookup_result_simple_add_string(
            result,
            E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr(),
            c"method".as_ptr(),
            OAUTH2_AUTH_METHOD.as_ptr(),
        );
        e_config_lookup_result_simple_add_string(
            result,
            E_SOURCE_EXTENSION_SECURITY.as_ptr(),
            c"method".as_ptr(),
            security_method.as_ptr(),
        );

        add_optional_string(
            result,
            oauth2::EXTENSION_NAME,
            c"client-id",
            config.client_id.as_deref(),
        );
        add_optional_string(
            result,
            oauth2::EXTENSION_NAME,
            c"client-secret",
            config.client_secret.as_deref(),
        );
        add_optional_string(
            result,
            oauth2::EXTENSION_NAME,
            c"authorization-endpoint",
            config.authorization_endpoint.as_deref(),
        );
        add_optional_string(
            result,
            oauth2::EXTENSION_NAME,
            c"token-endpoint",
            config.token_endpoint.as_deref(),
        );
        add_optional_string(
            result,
            oauth2::EXTENSION_NAME,
            c"scope",
            config.scope.as_deref(),
        );
        add_optional_string(
            result,
            oauth2::EXTENSION_NAME,
            c"resource",
            config.resource.as_deref(),
        );
        add_optional_string(
            result,
            oauth2::EXTENSION_NAME,
            c"redirect-uri",
            config.redirect_uri.as_deref(),
        );

        e_config_lookup_add_result(config_lookup, result);
    }
}

/// Probes the email address in `params` (or `servers`) for OAuth 2.0
/// discovery and registration. On success, adds one complete-account
/// [`EConfigLookupResult`].
///
/// It is silent when there is nothing to probe: no email address, an empty
/// domain, or a host that does not parse. `e-webdav-config-lookup.c` is silent
/// in the same way for a host without CalDAV/CardDAV. Many lookup workers run
/// in parallel on the user's input, and most of them do not match.
///
/// When a specific host gets a probe, a failure there (a network error, a
/// non-JMAP host, a deployment with no RFC 7591 registration endpoint) is not
/// "no match". It is "a JMAP host that is blocked". Thus `error` reports it
/// (see [`crate::oauth2_setup::Error::to_gerror`]).
///
/// Does not change `*out_restart_params`. `to_gerror` never reports one of the
/// two `EConfigLookupWorkerError` variants (a missing password, a certificate
/// to trust). Thus no failure of this worker needs a restart. Also, the caller
/// in `e-config-lookup.c` sets it to `NULL` before the call.
unsafe extern "C" fn run(
    _lookup_worker: *mut EConfigLookupWorker,
    config_lookup: *mut EConfigLookup,
    params: *const ENamedParameters,
    _out_restart_params: *mut *mut ENamedParameters,
    cancellable: *mut GCancellable,
    error: *mut *mut glib_sys::GError,
) {
    guard("JmapConfigLookup::run", (), || unsafe {
        let Some(email) = param(params, E_CONFIG_LOOKUP_PARAM_EMAIL_ADDRESS) else {
            return;
        };
        // A server that the user names has priority. If there is none, a lookup
        // that runs again starts from the host of the account (see
        // `prior_server`).
        let servers = param(params, E_CONFIG_LOOKUP_PARAM_SERVERS).or_else(|| {
            // SAFETY: `config_lookup` is the live lookup of this worker. The
            // source it returns is NULL or one that the main thread owns
            // (`transfer none`). `prior_server_of` takes a reference
            // immediately. This makes the window smaller, but cannot close it.
            let source =
                e_config_lookup_get_source(config_lookup, E_CONFIG_LOOKUP_SOURCE_COLLECTION);
            prior_server_of(source, &email)
        });
        // RFC 8620 §2.2: the provider can publish its JMAP host as a
        // `_jmap._tcp` record, and not answer at the bare email domain. This is
        // most important here: this worker decides if Evolution offers a JMAP
        // account. If it does not find a provider, the generic ISPDB
        // autoconfig wins (this caused imapx for the Fastmail account of the
        // operator).
        let resolver = jmap_backend_core::resolver::SystemResolver;
        let Some(host) = probe_host(&email, servers.as_deref(), &resolver) else {
            return;
        };
        let Some(target) = parse_target(&host) else {
            return;
        };

        tracing::debug!(
            target_host = target.host,
            target_port = target.port,
            secure = target.secure,
            "running JMAP config lookup worker"
        );

        // SAFETY: `cancellable` is NULL or a valid `GCancellable` that EDS
        // keeps alive during this call, by the contract of `run` (the same as
        // the C signature of `e_config_lookup_worker_run`).
        let bridge = CancelBridge::new(cancellable);
        let cancel_flag = bridge.flag().clone();
        let transport = UreqTransport::default();

        let config = match discover_and_register(
            &transport,
            &target.host,
            target.port,
            target.secure,
            REDIRECT_URI,
            Some(&cancel_flag),
        ) {
            Ok(config) => config,
            Err(discover_error) => {
                tracing::debug!(
                    target_host = target.host,
                    %discover_error,
                    "JMAP config lookup worker: discovery and registration failed"
                );
                jmap_backend_core::error::set_raw_gerror(error, discover_error.to_gerror());
                return;
            }
        };

        tracing::debug!(
            target_host = target.host,
            client_id = ?config.client_id,
            "JMAP config lookup worker added result"
        );

        add_result(config_lookup, &email, &target, &config);
    });
}

#[cfg(test)]
mod tests {
    use jmap_client::resolver::{NoSrvResolver, Resolver, SrvTarget};

    use std::ffi::CString;
    use std::ptr;

    use eds_sys::e_source_new_with_uid;
    use evo_sys::{E_CONFIG_LOOKUP_RESULT_PRIORITY_IMAP, E_CONFIG_LOOKUP_RESULT_PRIORITY_SMTP};
    use gobject_sys::g_object_unref;
    use jmap_collection_sync::Parts;
    use jmap_collection_sync::child_source::Connection;

    use super::{PRIORITY, REDIRECT_URI, parse_target, prior_server, prior_server_of, probe_host};
    use crate::account::{self, Account};

    #[test]
    fn redirect_uri_scheme_is_dotted_reverse_dns() {
        // The OAuth 2.0 doc of Fastmail requires a private-use redirect scheme
        // in reverse-DNS notation with at least one dot, or a loopback/https
        // URI. These URIs do not fit the WebKit consent prompter of EDS (see
        // the module docs). Dynamic client registration refuses a scheme
        // without a dot.
        let scheme = REDIRECT_URI.split(':').next().expect("a URI has a scheme");
        assert!(
            scheme.contains('.'),
            "redirect URI scheme {scheme:?} has no dot; providers requiring \
             reverse-DNS notation (e.g. Fastmail) would reject it"
        );
    }

    /// Returns one fixed answer (or none) for each domain. This is the same
    /// fake that `jmap-client/tests/srv_discovery.rs` uses.
    struct FakeResolver(Option<SrvTarget>);

    impl Resolver for FakeResolver {
        fn lookup_srv(&self, _domain: &str) -> Option<SrvTarget> {
            self.0.clone()
        }
    }

    #[test]
    fn uses_the_email_domain_when_no_servers_are_given() {
        assert_eq!(
            probe_host("vera@example.com", None, &NoSrvResolver),
            Some("example.com".to_owned())
        );
    }

    #[test]
    fn an_explicit_servers_entry_wins_over_the_domain() {
        assert_eq!(
            probe_host("vera@example.com", Some("jmap.example.net"), &NoSrvResolver),
            Some("jmap.example.net".to_owned())
        );
    }

    #[test]
    fn only_the_first_servers_entry_is_tried() {
        assert_eq!(
            probe_host(
                "vera@example.com",
                Some("first.example; second.example"),
                &NoSrvResolver
            ),
            Some("first.example".to_owned())
        );
    }

    #[test]
    fn blank_servers_fall_back_to_the_domain() {
        assert_eq!(
            probe_host("vera@example.com", Some("   "), &NoSrvResolver),
            Some("example.com".to_owned())
        );
    }

    #[test]
    fn an_address_with_no_at_sign_has_no_host_to_probe() {
        assert_eq!(probe_host("not-an-address", None, &NoSrvResolver), None);
    }

    #[test]
    fn an_address_with_an_empty_domain_has_no_host_to_probe() {
        assert_eq!(probe_host("vera@", None, &NoSrvResolver), None);
    }

    #[test]
    fn an_srv_record_wins_over_the_bare_domain() {
        let resolver = FakeResolver(Some(SrvTarget {
            host: "api.example.com".to_owned(),
            port: 443,
        }));
        assert_eq!(
            probe_host("vera@example.com", None, &resolver),
            Some("api.example.com:443".to_owned())
        );
    }

    #[test]
    fn no_srv_record_falls_back_to_the_bare_domain() {
        let resolver = FakeResolver(None);
        assert_eq!(
            probe_host("vera@example.com", None, &resolver),
            Some("example.com".to_owned())
        );
    }

    #[test]
    fn an_explicit_servers_entry_is_never_resolved_for_srv() {
        // This resolver answers for *any* domain. When `servers` names an
        // explicit host, the probe must not ask it: that host is the answer,
        // not a bare email domain for autodiscovery.
        let resolver = FakeResolver(Some(SrvTarget {
            host: "wrong.example.com".to_owned(),
            port: 443,
        }));
        assert_eq!(
            probe_host("vera@example.com", Some("jmap.example.net"), &resolver),
            Some("jmap.example.net".to_owned())
        );
    }

    #[test]
    fn an_srv_target_parses_as_a_bare_secure_host_with_its_port() {
        let target = parse_target("api.example.com:443").expect("host:port parses");
        assert_eq!(target.host, "api.example.com");
        assert_eq!(target.port, 443);
        assert!(target.secure);
    }

    #[test]
    fn a_bare_host_defaults_to_tls_and_no_explicit_port() {
        let target = parse_target("example.com").expect("a bare host parses");
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 0);
        assert!(target.secure);
    }

    #[test]
    fn an_explicit_scheme_and_port_override_both() {
        let target = parse_target("http://127.0.0.1:40565").expect("scheme and port parse");
        assert_eq!(target.host, "127.0.0.1");
        assert_eq!(target.port, 40565);
        assert!(!target.secure);
    }

    #[test]
    fn an_explicit_https_scheme_with_a_port_stays_secure() {
        let target = parse_target("https://jmap.example.net:8443").expect("scheme and port parse");
        assert_eq!(target.host, "jmap.example.net");
        assert_eq!(target.port, 8443);
        assert!(target.secure);
    }

    #[test]
    fn a_scheme_with_no_port_leaves_the_port_unset() {
        let target = parse_target("http://localhost").expect("a scheme alone parses");
        assert_eq!(target.host, "localhost");
        assert_eq!(target.port, 0);
        assert!(!target.secure);
    }

    #[test]
    fn a_bare_unbracketed_ipv6_literal_is_not_split_on_its_last_colon() {
        let target = parse_target("::1").expect("a bare IPv6 literal parses");
        assert_eq!(target.host, "::1");
        assert_eq!(target.port, 0);
        assert!(target.secure);
    }

    #[test]
    fn a_bracketed_ipv6_literal_with_a_port_splits_correctly() {
        let target = parse_target("https://[::1]:8443").expect("bracketed IPv6+port parses");
        assert_eq!(target.host, "::1");
        assert_eq!(target.port, 8443);
        assert!(target.secure);
    }

    #[test]
    fn a_bracketed_ipv6_literal_with_no_port_defaults_to_zero() {
        let target = parse_target("http://[2001:db8::1]").expect("bracketed IPv6 alone parses");
        assert_eq!(target.host, "2001:db8::1");
        assert_eq!(target.port, 0);
        assert!(!target.secure);
    }

    #[test]
    fn a_bracketed_ipv6_literal_with_trailing_garbage_has_no_target_to_reach() {
        assert!(parse_target("https://[::1]garbage").is_none());
    }

    #[test]
    fn a_non_numeric_port_suffix_has_no_target_to_reach() {
        assert!(parse_target("example.com:not-a-port").is_none());
    }

    #[test]
    fn an_empty_authority_has_no_target_to_reach() {
        assert!(parse_target("https://").is_none());
    }

    #[test]
    fn the_result_outranks_imap_and_smtp_by_the_margin_ews_uses() {
        let imap = E_CONFIG_LOOKUP_RESULT_PRIORITY_IMAP as i32;
        let smtp = E_CONFIG_LOOKUP_RESULT_PRIORITY_SMTP as i32;
        // Lower is better, and a tie loses to candidate order (see `PRIORITY`).
        assert_eq!(PRIORITY, imap - 100);
        assert!(PRIORITY < smtp);
    }

    /// A JMAP account for `alice@example.com`, as a previous lookup or the
    /// server settings page wrote it.
    fn prior(host: &str, port: Option<u16>, secure: bool) -> Account {
        Account {
            identity: "alice@example.com".to_owned(),
            connection: Connection {
                host: host.to_owned(),
                port,
                user: Some("alice@example.com".to_owned()),
                auth_method: None,
                secure,
            },
            parts: Parts {
                mail: true,
                contacts: true,
                calendars: true,
            },
            oauth2_registered: false,
        }
    }

    #[test]
    fn a_rerun_probes_the_host_the_account_already_names() {
        let account = prior("jmap.example.net", None, true);
        let server = prior_server("alice@example.com", Some("jmap"), &account);
        assert_eq!(server.as_deref(), Some("https://jmap.example.net"));
    }

    #[test]
    fn a_rerun_keeps_the_accounts_port_and_plaintext_choice() {
        let account = prior("127.0.0.1", Some(8080), false);
        let server = prior_server("alice@example.com", Some("jmap"), &account)
            .expect("a named host seeds the probe");
        assert_eq!(server, "http://127.0.0.1:8080");
        let target = parse_target(&server).expect("the seed parses back");
        assert_eq!(
            (target.host.as_str(), target.port, target.secure),
            ("127.0.0.1", 8080, false)
        );
    }

    #[test]
    fn a_rerun_brackets_an_ipv6_host_so_its_port_parses_back() {
        let account = prior("2001:db8::1", Some(8443), true);
        let server = prior_server("alice@example.com", Some("jmap"), &account)
            .expect("a named host seeds the probe");
        let target = parse_target(&server).expect("the seed parses back");
        assert_eq!(
            (target.host.as_str(), target.port, target.secure),
            ("2001:db8::1", 8443, true)
        );
    }

    #[test]
    fn the_bare_email_domain_is_left_to_srv_discovery() {
        // The server page shows the email domain by default. A probe of it
        // would skip the `_jmap._tcp` lookup that a provider such as Fastmail
        // needs.
        let account = prior("Example.COM", None, true);
        assert_eq!(
            prior_server("alice@example.com", Some("jmap"), &account),
            None
        );
    }

    #[test]
    fn another_backends_account_does_not_seed_the_probe() {
        let account = prior("outlook.office365.com", None, true);
        assert_eq!(
            prior_server("alice@example.com", Some("ews"), &account),
            None
        );
        assert_eq!(prior_server("alice@example.com", None, &account), None);
    }

    #[test]
    fn a_changed_email_address_does_not_reuse_the_old_host() {
        let account = prior("jmap.example.net", None, true);
        assert_eq!(
            prior_server("bob@example.org", Some("jmap"), &account),
            None
        );
    }

    #[test]
    fn the_email_address_matches_case_insensitively() {
        let account = prior("jmap.example.net", None, true);
        assert_eq!(
            prior_server("Alice@Example.com", Some("jmap"), &account).as_deref(),
            Some("https://jmap.example.net")
        );
    }

    #[test]
    fn an_account_with_no_host_does_not_seed_the_probe() {
        let account = prior("", None, true);
        assert_eq!(
            prior_server("alice@example.com", Some("jmap"), &account),
            None
        );
    }

    #[test]
    fn a_rerun_reads_the_seed_from_the_collection_source() {
        let uid = CString::new("jmap-config-lookup-prior").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments are a NUL-terminated uid, no D-Bus
        // object and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");

        // SAFETY: `source` is live until the unref below. `apply` writes
        // `[Collection] backend-name = jmap` with the account.
        let server = unsafe {
            account::apply(source, &prior("jmap.example.net", Some(443), true));
            prior_server_of(source, "alice@example.com")
        };
        // SAFETY: NULL is "no source", which the assistant can return.
        let none = unsafe { prior_server_of(ptr::null_mut(), "alice@example.com") };
        // SAFETY: the one reference that `e_source_new_with_uid` gave.
        unsafe { g_object_unref(source.cast()) };

        assert_eq!(server.as_deref(), Some("https://jmap.example.net:443"));
        assert_eq!(none, None);
    }
}
