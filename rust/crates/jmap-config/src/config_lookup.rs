// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapConfigLookup`: the `EConfigLookupWorker` behind Evolution's own
//! "Look Up Account Details" step, which is what actually closes M7's OAuth
//! 2.0 gap.
//!
//! [`oauth2_service`](crate::oauth2_service) and
//! [`oauth2_setup::discover_and_register`] are both complete, but nothing yet
//! calls the latter — the manual server-settings page
//! [`backend::insert_widgets`](crate::backend) builds has no trigger for a
//! blocking network probe that would not risk blocking the GTK main thread
//! or need a maintainer decision about what UI asks for it. This module does
//! not add either: implementing `EConfigLookupWorker` is registering with a
//! mechanism Evolution's account assistant already has, generic across every
//! provider, the same one `evolution-ews`'s own
//! `e-ews-config-lookup.c` uses for Exchange autodiscovery. The trigger is
//! the assistant's own "Look Up Account Details" button; the thread is one
//! `EConfigLookup` already runs this on (`e-config-lookup.c`'s
//! `g_thread_pool_push`, confirmed by reading that file rather than assumed).
//!
//! ## Read against real upstream source, not assumed
//!
//! `/tmp/evo-src` (this VM's checkout of `evolution`, not EDS) has three real
//! `EConfigLookupWorker` implementers under `src/modules/config-lookup/`.
//! `e-webdav-config-lookup.c` is the shape this module follows almost line
//! for line: `extensible_type = E_TYPE_CONFIG_LOOKUP` in `class_init`,
//! `constructed` chaining up and then calling
//! `e_config_lookup_register_worker`, and `run()` building an
//! `EConfigLookupResultSimple` and handing it to `e_config_lookup_add_result`.
//! `e-config-lookup.c`/`.h` and `e-config-lookup-result-simple.c`/`.h` (same
//! tree) pin the ownership this module relies on:
//! `e_config_lookup_result_simple_new` is a plain `g_object_new` (transfer
//! full to its caller), every `_add_*` copies the string/value it is given,
//! and `e_config_lookup_add_result`'s own doc comment says outright that the
//! `EConfigLookup` "assumes ownership of the result and frees it when no
//! longer needed" — so a result here is built, populated, handed over, and
//! never unref'd by this crate.
//!
//! ## The redirect URI, and a wrong guess corrected before it shipped
//!
//! A previous session's plan (`docs/NIGHT-LOG.md`, three-hundred-and-fourth
//! session) was to fix `redirect_uri` to the RFC 8252 out-of-band URN,
//! `"urn:ietf:wg:oauth:2.0:oob"`. Checking that against EDS's actual
//! `libedataserverui`/`libedataserver` source (fetched from
//! `gitlab.gnome.org/GNOME/evolution-data-server`, `master` — this VM has no
//! local EDS checkout, only `evolution`'s) shows it is the wrong guess for
//! *this* prompter: `ECredentialsPrompterImplOAuth2` never compares the
//! browser's navigation against the redirect URI at all.
//! `get_authentication_policy`'s default (`e-oauth2-service.c`) is
//! unconditionally `ALLOW`, and the authorization code is extracted from
//! whatever URI the embedded `WebKitWebView` reaches `WEBKIT_LOAD_FINISHED`
//! on — `e_oauth2_service_util_extract_from_uri` parses `code=`/`error=` out
//! of any URI's query or fragment with no prefix check. What has to actually
//! work is WebKit *finishing* (not failing) that navigation, and EDS's own
//! shipped `e-oauth2-service-google.c` answers that with a private-use URI
//! *scheme* (`«reversed client id»:/oauth2redirect`), not the OOB URN — real,
//! load-bearing precedent, since Google account setup works today against
//! this exact prompter. [`REDIRECT_URI`] follows that precedent instead.
//!
//! ## Where `run()`'s live dispatch is proven
//!
//! `run()`'s live dispatch — reached by a real `EConfigLookup`, which
//! `e_config_lookup_new` refuses to construct without a live
//! `ESourceRegistry` (checked against `e-config-lookup.c`'s own
//! `g_return_val_if_fail`) — needs the `dbus-run-session` environment M9's
//! functional harness already sets up (`cmake/Functional.cmake`), which
//! `cargo test` alone does not have. [`probe_host`] is unit-tested directly
//! since it needs none of that; the FFI shell below is built and registered
//! the same way [`crate::oauth2_service::Service`] was, and its own dispatch
//! through a real `EConfigLookup` is what
//! `jmap-functional/tests/config-lookup.rs` now drives — the M9 layer-1
//! coverage `evo-sys/tests/config_lookup.rs` names as the next increment when
//! it was written, since landed.
//!
//! The 307th session (`docs/NIGHT-LOG.md`) hand-drove this dispatch once,
//! outside the test suite: a scratch C program linking `evolution-shell-3.0`
//! (which is where `EConfigLookup` actually lives — `e-util`, not any EDS
//! library), loading this module via `e_module_load_all_in_directory`,
//! constructing a real `ESourceRegistry`/`EConfigLookup` under
//! `dbus-run-session`, and running it against a `jmap-mockd --oauth2`
//! instance produced exactly one positive, complete result. That is evidence
//! the mechanism works end to end and that `LD_LIBRARY_PATH=/usr/lib/evolution`
//! is the missing piece a headless client needs (the module's own transitive
//! `libevolution-mail.so` dependency is not on the default loader path) — not
//! a standing test, and not a substitute for the automated harness above.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    EExtension, EExtensionClass, ENamedParameters, e_extension_get_extensible,
    e_extension_get_type, e_named_parameters_get,
};
use evo_sys::{
    E_CONFIG_LOOKUP_PARAM_EMAIL_ADDRESS, E_CONFIG_LOOKUP_PARAM_SERVERS,
    E_CONFIG_LOOKUP_RESULT_COLLECTION, EConfigLookup, EConfigLookupResult, EConfigLookupWorker,
    EConfigLookupWorkerInterface, e_config_lookup_add_result, e_config_lookup_get_type,
    e_config_lookup_register_worker, e_config_lookup_result_simple_add_boolean,
    e_config_lookup_result_simple_add_string, e_config_lookup_result_simple_add_uint,
    e_config_lookup_result_simple_new, e_config_lookup_worker_get_type,
};
use gio_sys::GCancellable;
use glib_sys::{GTRUE, GType, gchar};
use gobject_sys::{GObject, GObjectClass, g_type_class_peek};
use jmap_backend_core::cancel::CancelBridge;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{self, N_, translate_with};
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass};
use jmap_backend_core::trampoline::guard;
use jmap_client::transport::UreqTransport;

use crate::account::BACKEND_NAME;
use crate::oauth2::{self, Config};
use crate::oauth2_service;
use crate::oauth2_setup::discover_and_register;

/// The private-use URI scheme registered with RFC 7591 and answered by
/// [`crate::oauth2_service::Service::get_redirect_uri`] — see the module docs
/// for why this, and not the RFC 8252 out-of-band URN, is what actually works
/// against EDS's WebKit-based consent prompter.
///
/// Dotted reverse-DNS, not a bare word: some providers (Fastmail's OAuth
/// doc, confirmed in `docs/OAUTH-FASTMAIL.md`) require a private-use
/// redirect scheme to contain at least one dot and reject registration
/// otherwise.
pub const REDIRECT_URI: &str = "org.gnome.evolution.jmap:/redirect";

/// The host RFC 8414 discovery is asked of, and later written as
/// `[Authentication] host` in a positive result — a bare domain, not a
/// guessed subdomain, for the same reason `defaults.rs` gives the manual
/// setup page one: JMAP's own discovery (RFC 8620 §2.2) finds everything
/// else `.well-known/jmap` names once the account connects.
///
/// `servers`, when given, names the host explicitly and wins — the one thing
/// [`E_CONFIG_LOOKUP_PARAM_SERVERS`] exists for. Only its first entry is
/// tried: unlike CalDAV/CardDAV autodiscovery (`e-webdav-config-lookup.c`,
/// which tries every listed server because non-JMAP servers may sit at some
/// and not others), a JMAP deployment names exactly one issuer, so a second
/// entry would be trying a second server the first one failing. An explicit
/// entry is not a bare domain and so is never run through `resolver` — SRV
/// autodiscovery (RFC 8620 §2.2) is what the email-domain fallback below
/// exists to correct, not something an already-explicit host needs.
///
/// The email-domain fallback consults `resolver` for a `_jmap._tcp.<domain>`
/// SRV record (see `jmap_client::resolver`) before returning the bare domain
/// — the same seam and the same fallback order
/// `jmap_client::ClientBuilder::connect_domain` uses, so a deployment
/// published only via SRV (Fastmail; see `docs/NIGHT-LOG.md`, "JMAP SRV
/// autodiscovery") is discovered here too, not just once a `jmap_client`
/// session is already being fetched.
///
/// What this returns is not yet a host `discover_and_register` can use as-is
/// — a `servers` entry or an SRV target may carry a scheme and a port, which
/// [`parse_target`] reads back out; an SRV target is rendered `host:port`,
/// which `parse_target` parses as a bare, secure host with that port, the
/// right reading for a target a JMAP SRV record ever names.
pub(crate) fn probe_host(
    email_address: &str,
    servers: Option<&str>,
    resolver: &dyn jmap_client::resolver::Resolver,
) -> Option<String> {
    if let Some(servers) = servers
        && let Some(first) = servers.split(';').map(str::trim).find(|s| !s.is_empty())
    {
        return Some(first.to_owned());
    }
    let (_, domain) = email_address.split_once('@')?;
    if domain.is_empty() {
        return None;
    }
    if let Some(target) = resolver.lookup_srv(domain) {
        return Some(format!("{}:{}", target.host, target.port));
    }
    Some(domain.to_owned())
}

/// Where [`probe_host`]'s answer is actually reached: a bare host, an
/// explicit port when one was named, and whether to speak TLS.
struct Target {
    host: String,
    port: u16,
    secure: bool,
}

/// Reads a `scheme://host:port` override out of [`probe_host`]'s answer, the
/// same convention real upstream's own `e-webdav-config-lookup.c` uses for its
/// `servers` values (read against `/tmp/evo-src`'s copy, not assumed): a bare
/// host defaults to HTTPS and no explicit port — right for the email-domain
/// fallback, which is always a bare domain — and an `http://`/`https://`
/// prefix, with an optional `:port` suffix, overrides both. That override is
/// what lets a `servers` entry name a locally-run, plaintext, non-default-port
/// test deployment at all; the email-domain path never needs it.
///
/// A bare, unbracketed IPv6 literal (more than one colon, no `[...]`) is left
/// whole rather than split on its last colon — [`jmap_backend_core::source`]
/// accepts a host in that shape, and splitting one on `:` would cut it apart
/// as though the last group were a port.
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
    let (bare_host, port) = match authority.rsplit_once(':') {
        Some((bare_host, port)) if !bare_host.contains(':') => (bare_host, port.parse().ok()?),
        _ => (authority, 0),
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

/// The instance struct: nothing but [`EExtension`]'s own state — everything
/// this worker needs is either `'static` or reached through `run()`'s own
/// arguments.
#[repr(C)]
pub struct JmapConfigLookup {
    parent: EExtension,
}

/// The class struct: nothing but [`EExtensionClass`]'s own state.
#[repr(C)]
pub struct JmapConfigLookupClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and lead with EExtension's own
// instance/class structs; EExtension derives from GObject (eds-sys/tests/
// oauth2.rs checks its size against `g_type_query`).
unsafe impl ObjectSubclass for JmapConfigLookup {
    const NAME: &'static CStr = c"JmapConfigLookup";
    type Instance = JmapConfigLookup;
    type Class = JmapConfigLookupClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_extension_get_type() }
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        vec![InterfaceDecl::filled_by::<Vtable>()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with `EExtensionClass`, whose `extensible_type`
        // field is what `e_extensible_load_extensions` matches against —
        // `e_webdav_config_lookup_class_init`'s own assignment, mirrored here.
        unsafe { (*class).parent_class.extensible_type = e_config_lookup_get_type() };

        // SAFETY: `JmapConfigLookupClass` leads with `EExtensionClass`, which
        // leads with `GObjectClass` — the same transitive leading-field cast
        // `jmap_config::oauth2::Extension`'s `class_init` uses.
        let object_class = class.cast::<GObjectClass>();
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// Chains up (as `e_webdav_config_lookup_constructed` does), then registers
/// this instance with the `EConfigLookup` it extends — the "putting the type
/// in the type system is the registration" idiom, completed at the moment
/// each `EConfigLookup` is actually constructed rather than at module load,
/// since unlike [`crate::backend::JmapConfigServiceBackend`] this type needs
/// a live instance of what it extends to register itself *against*.
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapConfigLookup::constructed", (), || unsafe {
        // SAFETY: the parent class from a live instance's own class is
        // initialised and alive for as long as any instance is.
        let parent = g_type_class_peek(JmapConfigLookup::parent_type()).cast::<GObjectClass>();
        if let Some(chained) = parent.as_ref().and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject passes a live instance of this type; `EExtensible`
        // interface pointers are the same address as the instance they were
        // fetched from (GObject interfaces have no adjusted `this`), which is
        // what makes this cast the Rust equivalent of the C `E_CONFIG_LOOKUP()`
        // macro on `e_extension_get_extensible`'s answer.
        let config_lookup = e_extension_get_extensible(object.cast::<EExtension>()).cast();
        e_config_lookup_register_worker(config_lookup, object.cast::<EConfigLookupWorker>());
    });
}

/// The filling of [`JmapConfigLookup`]'s copy of `EConfigLookupWorkerInterface`.
struct Vtable;

// SAFETY: `EConfigLookupWorkerInterface` is bindgen's `#[repr(C)]` binding of
// the interface struct `e_config_lookup_worker_get_type` names, and it leads
// with `GTypeInterface` — `evo-sys/tests/config_lookup.rs` pins the
// interface's shape and that both slots have no default to fall back to.
unsafe impl InterfaceImpl for Vtable {
    type Vtable = EConfigLookupWorkerInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_config_lookup_worker_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: the contract of `InterfaceImpl::interface_init` — this is
        // our own copy of the vtable, and nothing else can reach it yet.
        let vtable = unsafe { &mut *vtable };
        vtable.get_display_name = Some(get_display_name);
        vtable.run = Some(run);
    }
}

/// The label the assistant's results list would show while this worker is
/// still running — not [`EConfigLookupResult`]'s own `display_name`, which
/// [`run`] sets per result and which is what the user actually picks from.
unsafe extern "C" fn get_display_name(_lookup_worker: *mut EConfigLookupWorker) -> *const gchar {
    guard("JmapConfigLookup::get_display_name", ptr::null(), || {
        i18n::translate_static(N_(c"Look up JMAP account details"))
    })
}

/// Reads a string parameter out of `params`, or `None` for one that is unset
/// — [`e_named_parameters_get`]'s own NULL-means-absent contract.
///
/// # Safety
///
/// `params` must be NULL or a valid `ENamedParameters`, valid for the call.
unsafe fn param(params: *const ENamedParameters, name: &CStr) -> Option<String> {
    // SAFETY: forwarded from this function's own contract.
    let value = unsafe { e_named_parameters_get(params, name.as_ptr()) };
    (!value.is_null()).then(|| {
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    })
}

/// Adds `value`, if present, as `extension_name`/`property_name` on `result`
/// — [`e_config_lookup_result_simple_add_string`] copies the string, so
/// nothing here needs to outlive the call beyond the `CString` that makes it.
unsafe fn add_optional_string(
    result: *mut EConfigLookupResult,
    extension_name: &CStr,
    property_name: &CStr,
    value: Option<&str>,
) {
    if let Some(value) = value {
        let value = cstring_lossy(value);
        // SAFETY: `result` is a live `EConfigLookupResultSimple` by this
        // function's contract; both names and the value are valid, NUL-
        // terminated strings for the call.
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

/// Builds the one [`EConfigLookupResult`] a successful discovery produces —
/// a complete JMAP account, `[Collection]`/`[Authentication]`/`[Security]`
/// filled the same way [`crate::account::apply`] fills them and `[JMAP
/// OAuth2]` filled the same way [`crate::oauth2::apply`] does — and hands it
/// to `config_lookup`, which owns it from that call on (see the module docs
/// on `e_config_lookup_add_result`'s transfer-full contract).
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

    // SAFETY: every string argument is a live, NUL-terminated `CString`'s
    // pointer, valid for this one call; `_new` copies each of them
    // (confirmed against `e-config-lookup-result-simple.c`) and returns a
    // new, owned `EConfigLookupResult` — see the module docs.
    let result = unsafe {
        e_config_lookup_result_simple_new(
            E_CONFIG_LOOKUP_RESULT_COLLECTION,
            // Lower is higher priority; matched to the plain IMAP priority
            // `e-config-lookup-result.h` defines (1000) since, like IMAP,
            // this is the one-account-one-mailbox case, not a fallback.
            1000,
            GTRUE,
            protocol.as_ptr(),
            i18n::translate_static(N_(c"JMAP account (OAuth 2.0)")),
            description.as_ptr(),
            ptr::null(),
        )
    };

    // SAFETY: `result` was just constructed above and is reachable by
    // nothing else yet; every extension/property name is `'static`, and
    // every value is a live `CString` for the duration of its own call.
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
            oauth2_service::NAME.as_ptr(),
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
            c"redirect-uri",
            config.redirect_uri.as_deref(),
        );

        e_config_lookup_add_result(config_lookup, result);
    }
}

/// Probes `params`' email address (or `servers`) for OAuth 2.0 discovery and
/// registration, adding one complete-account [`EConfigLookupResult`] on
/// success. Silent on any failure — a network error, a non-JMAP host, or a
/// deployment with no RFC 7591 registration endpoint — the same way
/// `e-webdav-config-lookup.c` stays silent for a host that turns out not to
/// speak CalDAV/CardDAV: several lookup workers run in parallel against
/// whatever the user typed, and most of them will not match.
///
/// Leaves `*out_restart_params` untouched: nothing this worker can hit needs
/// a restart (no password, no certificate trust decision — RFC 8414
/// discovery needs no credentials at all), and `e-config-lookup.c`'s own
/// caller already initialises it to `NULL` before the call.
unsafe extern "C" fn run(
    _lookup_worker: *mut EConfigLookupWorker,
    config_lookup: *mut EConfigLookup,
    params: *const ENamedParameters,
    _out_restart_params: *mut *mut ENamedParameters,
    cancellable: *mut GCancellable,
    _error: *mut *mut glib_sys::GError,
) {
    guard("JmapConfigLookup::run", (), || unsafe {
        let Some(email) = param(params, E_CONFIG_LOOKUP_PARAM_EMAIL_ADDRESS) else {
            return;
        };
        let servers = param(params, E_CONFIG_LOOKUP_PARAM_SERVERS);
        // RFC 8620 §2.2: the provider may publish its JMAP host as a
        // `_jmap._tcp` record rather than answer at the bare email domain, and
        // this worker is where that matters most — it is what decides whether
        // Evolution offers a JMAP account at all, so a provider missed here
        // loses to the generic ISPDB autoconfig (which is how the operator's
        // Fastmail setup ended up being offered imapx; see `docs/NIGHT-LOG.md`,
        // "JMAP SRV autodiscovery").
        let resolver = jmap_backend_core::resolver::SystemResolver;
        let Some(host) = probe_host(&email, servers.as_deref(), &resolver) else {
            return;
        };
        let Some(target) = parse_target(&host) else {
            return;
        };

        // SAFETY: `cancellable` is NULL or a valid `GCancellable` that EDS
        // keeps alive for the duration of this call, per `run`'s own
        // contract (mirroring `e_config_lookup_worker_run`'s C signature).
        let bridge = CancelBridge::new(cancellable);
        let cancel_flag = bridge.flag().clone();
        let transport = UreqTransport::default();

        let Ok(config) = discover_and_register(
            &transport,
            &target.host,
            target.port,
            target.secure,
            REDIRECT_URI,
            Some(&cancel_flag),
        ) else {
            return;
        };

        add_result(config_lookup, &email, &target, &config);
    });
}

#[cfg(test)]
mod tests {
    use jmap_client::resolver::{NoSrvResolver, Resolver, SrvTarget};

    use super::{REDIRECT_URI, parse_target, probe_host};

    #[test]
    fn redirect_uri_scheme_is_dotted_reverse_dns() {
        // Fastmail's OAuth 2.0 doc requires a private-use redirect scheme in
        // reverse-DNS notation with at least one dot (or a loopback/https
        // URI, neither of which fits EDS's WebKit-based consent prompter —
        // see the module docs) — a dot-less scheme is rejected at dynamic
        // client registration (docs/OAUTH-FASTMAIL.md).
        let scheme = REDIRECT_URI.split(':').next().expect("a URI has a scheme");
        assert!(
            scheme.contains('.'),
            "redirect URI scheme {scheme:?} has no dot; providers requiring \
             reverse-DNS notation (e.g. Fastmail) would reject it"
        );
    }

    /// Returns one fixed answer for every domain asked, or none — the same
    /// fake `jmap-client/tests/srv_discovery.rs` uses.
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
        // A resolver that would answer for *any* domain must not be asked at
        // all when `servers` names an explicit host — that host is not a
        // bare email domain to autodiscover, it is already the answer.
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
    fn a_non_numeric_port_suffix_has_no_target_to_reach() {
        assert!(parse_target("example.com:not-a-port").is_none());
    }

    #[test]
    fn an_empty_authority_has_no_target_to_reach() {
        assert!(parse_target("https://").is_none());
    }
}
