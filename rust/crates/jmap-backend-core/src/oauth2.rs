// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether an account authenticates with OAuth 2.0, and the access token if it
//! does.
//!
//! ## The rule is EDS's, transcribed rather than invented
//!
//! Nothing here decides *policy*. `e_soup_session_setup_message_credentials`
//! (evolution-data-server 3.52.3, `src/libedataserver/e-soup-session.c`) is the
//! function every WebDAV, CalDAV and CardDAV account in Evolution
//! authenticates through, and it chooses like this:
//!
//! ```c
//! if (g_strcmp0 (auth_method, "OAuth2") == 0 ||
//!     e_oauth2_services_is_oauth2_alias_static (auth_method)) {
//!         success = e_soup_session_maybe_prepare_bearer_auth (…);
//! } else if (…) {
//! } else if (user && *user) {
//!         success = e_soup_session_maybe_prepare_basic_auth (…);
//! }
//! ```
//!
//! …and the bearer branch's token is `e_source_get_oauth2_access_token_sync`.
//! [`method_is_oauth2`] and [`access_token`] are those two lines for JMAP. The
//! rule is copied rather than approximated because it is the one an account
//! *written* by anything else in Evolution — the setup UI, a hand-edited
//! keyfile, a future EDS release — will have been written to satisfy, and a
//! JMAP account that read the same keyfield differently would be an account
//! Evolution thinks is OAuth2 and this code asks for a password.
//!
//! The `"OAuth2"` literal and the alias lookup are two different questions and
//! both are asked. `"OAuth2"` is the generic spelling, meaning "whichever
//! registered service claims this source"; an alias is a *particular* service's
//! [`name`][name] written into the field, which is what this project's own
//! setup writes (`jmap_config::oauth2_service::NAME`, `"JMAP"`). The alias
//! lookup is a live query against the registered services rather than a
//! constant list, so it answers yes for our service exactly when
//! `module-jmap-backend.so` has registered it — which is the same condition
//! under which the token could be fetched at all.
//!
//! [name]: ../../jmap_config/oauth2_service/constant.NAME.html
//!
//! ## Why the services object is held rather than made per question
//!
//! The one transcription liberty taken with EDS's rule above is which of the
//! two spellings of the alias lookup is called.
//! `e_oauth2_services_is_oauth2_alias_static()` — the one
//! `e_soup_session_setup_message_credentials` uses — creates an
//! `EOAuth2Services`, queries it and drops that reference again on every call,
//! and its own documentation names the precondition that makes that safe:
//!
//! > The #EOAuth2Services is implemented as a singleton, thus it won't be much
//! > trouble, **as long as there is something else having created one
//! > instance.**
//!
//! Where nothing else has, the reference it drops is the last one, and two
//! threads asking at once race: one is inside `oauth2_services_dispose`, which
//! frees `priv->services` without clearing the field, while the other is inside
//! `oauth2_services_constructor`, which still sees the not-yet-cleared
//! `services_singleton` and takes a reference to it — a legal GObject
//! resurrection of an object whose service list has just been freed. The
//! resurrected instance is then walked, and a dangling `EOAuth2Service` is
//! dereferenced. That is a SIGSEGV in whichever process asked, which for a
//! collection backend is `evolution-source-registry` and so every account in
//! the session.
//!
//! Every real Evolution process happens to satisfy the precondition —
//! `e_source_registry_init()` holds an `EOAuth2Services` for the registry's
//! lifetime — so this is a crash nothing reaches by accident *today*. This
//! module holds one on purpose instead: a single instance, created on the first
//! question and never released, which is EDS's precondition met deliberately
//! rather than inherited from whoever else happens to be in the process.
//! `e_oauth2_services_is_oauth2_alias()` is then the same lookup `_static`
//! would have made, minus the create-and-destroy around it.
//!
//! **What holding one does *not* change is which services are found.** An
//! `EOAuth2Services` loads its extensions once, at construction
//! (`e_extensible_load_extensions`), so which services answer depends on what
//! had registered one by the time the instance was built. In a process that
//! holds a registry, the instance is the registry's — and `_static` was already
//! returning that same one, never getting as far as constructing anything of
//! its own, because the singleton was alive throughout. Taking one more
//! reference to it changes nothing about when it was built. Only where nothing
//! else holds one does this fix the instance's age, and that is exactly where
//! it fixes the crash. `e_extensible_reload_extensions()` is deliberately *not*
//! called to re-widen that: it mutates the extension array under no lock of its
//! own, so calling it per question would put back a race of the same kind this
//! removes.
//!
//! ## What is deliberately not re-checked here
//!
//! **That the connection is encrypted.** EDS's own rule pre-fills credentials
//! only on `https`, and this code sends a bearer token over whatever origin
//! [`SourceConfig`](crate::source::SourceConfig) resolved — but that is not a
//! weaker rule, it is the same rule enforced one layer up:
//! [`source::origin`](crate::source::origin) already refuses to build a
//! non-TLS origin for any host that is not loopback. So a token can only ever
//! leave this process in clear over a connection to this machine, which is the
//! `jmap-mockd` development case, and adding a second TLS test here would be a
//! rule that exists twice and gets corrected once.
//!
//! **The token's lifetime.** `e_source_get_oauth2_access_token_sync` also
//! answers how many seconds the token is good for, and it is dropped on the
//! floor: a connect fetches a token, uses it for that connect, and the next
//! connect asks again. EDS refreshes an expired token inside that call, so the
//! number is only of use to something that holds a connection open across the
//! expiry and re-authenticates in place — which no JMAP backend here does.

use std::ffi::CString;
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, EOAuth2Services, ESource, ESourceAuthentication,
    e_oauth2_services_is_oauth2_alias, e_oauth2_services_new, e_source_authentication_get_method,
    e_source_authentication_get_type, e_source_get_oauth2_access_token_sync,
};
use gio_sys::{
    G_DBUS_ERROR_NAME_HAS_NO_OWNER, G_DBUS_ERROR_SERVICE_UNKNOWN, G_IO_ERROR_NOT_FOUND,
    GCancellable, g_dbus_error_quark, g_io_error_quark,
};
use glib_sys::{GError, GFALSE, g_error_free, g_free};

use crate::connect::ConnectError;
use crate::i18n::{translate, translate_with};
use crate::marshal::{extension_if_present, read_string};

/// The generic spelling of "this source authenticates with OAuth 2.0", as
/// opposed to the name of one particular service.
pub const OAUTH2_METHOD: &str = "OAuth2";

/// The process's `EOAuth2Services`, kept alive for as long as the process is —
/// see the module docs on why holding one is part of asking the question
/// safely.
///
/// A raw pointer rather than a wrapper with a `Drop`, deliberately: there is no
/// point in the process's life at which releasing this would be an improvement,
/// and a release is exactly the thing whose absence makes the singleton safe to
/// query. Everything below only ever reads it.
struct Services(*mut EOAuth2Services);

// SAFETY: the pointer is only ever handed back to EDS's own accessors, and the
// ones this module calls take the instance's `property_lock` around every read
// of the service list (`e-oauth2-services.c`). The pointer itself is written
// once, under `OnceLock`, and read-only afterwards.
unsafe impl Send for Services {}
// SAFETY: as `Send`.
unsafe impl Sync for Services {}

static SERVICES: OnceLock<Services> = OnceLock::new();

/// The held `EOAuth2Services`, created on the first question.
///
/// `OnceLock` is what closes the window the module docs describe rather than
/// merely narrowing it: the first thread to ask creates the instance while
/// every other waits, so no second thread can be part-way through a
/// create-or-destroy of the singleton while this one runs. From then on this
/// module holds a reference, so nothing else in the process can drop the last
/// one either — including EDS's own transient `ESourceRegistry` inside
/// [`access_token`], which is only ever reached *after* a question has been
/// asked and so after this has run.
fn services() -> *mut EOAuth2Services {
    // SAFETY: no arguments; the reference this returns is transfer-full and is
    // deliberately never given back.
    SERVICES
        .get_or_init(|| Services(unsafe { e_oauth2_services_new() }))
        .0
}

/// Whether `[Authentication] Method` names OAuth 2.0 — see the module docs for
/// whose rule this is.
///
/// `None` is a source with no `[Authentication]` group at all. It is not the
/// same as `Some("none")`, which is what a source that *has* the group reads
/// back as when nobody set a method — `ESourceAuthentication:method` has no
/// unset state — and both are equally not OAuth 2.0.
pub fn method_is_oauth2(method: Option<&str>) -> bool {
    let Some(method) = method else {
        return false;
    };
    if method == OAUTH2_METHOD {
        return true;
    }
    // An interior NUL cannot be an authentication method: EDS stores the field
    // as a C string, so a value containing one could never have been written
    // through `e_source_authentication_set_method` in the first place. Refusing
    // is the safe direction — it answers "not OAuth2" and the account falls
    // back to the password path, rather than truncating and possibly matching a
    // service whose name is a prefix of the string.
    let Ok(method) = CString::new(method) else {
        return false;
    };
    // An `EOAuth2Services` that could not be constructed is one with no
    // registered services to match against, which is the same answer an empty
    // one gives — and the same safe direction as the interior-NUL case above:
    // the account falls back to the password path rather than to a token
    // nothing could have issued.
    let services = services();
    if services.is_null() {
        return false;
    }
    // SAFETY: a live `EOAuth2Services` this module holds a reference to for the
    // process's lifetime, and a NUL-terminated string valid for the call. The
    // function applies EDS's own guard against "none"/"plain/password"/the
    // empty string, locks the instance around the lookup, and takes nothing of
    // ours.
    unsafe { e_oauth2_services_is_oauth2_alias(services, method.as_ptr()) != GFALSE }
}

/// [`method_is_oauth2`] asked of a source, which is how `connect_sync` reaches
/// it.
///
/// # Safety
///
/// `source` must be a valid `ESource` — the one EDS handed the backend. It is
/// only read from, and nothing outlives the call.
pub unsafe fn source_uses_oauth2(source: *mut ESource) -> bool {
    // As everywhere in this crate an extension is looked up by name: the
    // lookup walks the registered children of `E_TYPE_SOURCE_EXTENSION`, so a
    // type nothing has referenced yet is one it cannot find.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe { e_source_authentication_get_type() };

    // Asked before `e_source_get_extension`, which *creates* the extension it
    // cannot find: this is handed the user's own account source, and creating
    // a group on it to read a default out of it is a side effect on someone
    // else's object.
    // SAFETY: a valid source by this function's contract, and a header
    // constant for the name.
    let Some(auth) = (unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }) else {
        return false;
    };

    // SAFETY: the extension exists, is owned by the source, and the string
    // the getter answers with is owned by the extension and outlives this
    // call.
    let method = unsafe { read_string(e_source_authentication_get_method(auth)) };

    method_is_oauth2(method.as_deref())
}

/// Why an OAuth 2.0 access token could not be obtained silently through EDS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilentRefreshFailureReason {
    /// No OAuth 2.0 secret (refresh token) has been stored for this account
    /// yet (the account has not been consented to, or was removed).
    NoStoredSecret,
    /// The login keyring holding the stored secret is locked and cannot be
    /// unlocked headlessly.
    KeyringLocked,
    /// A secret store (D-Bus / daemon) transport or spawn failure occurred.
    SecretStoreFailure,
    /// A D-Bus peer the token fetch had to reach does not exist — see
    /// [`is_service_gone`]. Infrastructure, never a credential.
    ServiceGone,
    /// The authorization server rejected the refresh token exchange (e.g.
    /// `invalid_grant`, expired refresh token, or rotation mismatch).
    ServerRejectedRefresh,
    /// The source is not supported or has no registered `EOAuth2Service`.
    UnregisteredService,
    /// The refresh operation was cancelled.
    Cancelled,
    /// The service returned an empty access token string.
    EmptyToken,
    /// An unrecognized or generic I/O error occurred in the `G_IO_ERROR` domain.
    OtherIoError,
    /// An unrecognized error domain occurred.
    OtherError,
    /// EDS failed without setting a GError.
    NullError,
}

impl SilentRefreshFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoStoredSecret => "no_stored_secret",
            Self::KeyringLocked => "keyring_locked",
            Self::SecretStoreFailure => "secret_store_failure",
            Self::ServiceGone => "service_gone",
            Self::ServerRejectedRefresh => "server_rejected_refresh",
            Self::UnregisteredService => "unregistered_service",
            Self::Cancelled => "cancelled",
            Self::EmptyToken => "empty_token",
            Self::OtherIoError => "other_io_error",
            Self::OtherError => "other_error",
            Self::NullError => "null_error",
        }
    }

    pub const fn escalates_to_consent(self) -> bool {
        match self {
            Self::NoStoredSecret
            | Self::ServerRejectedRefresh
            | Self::UnregisteredService
            | Self::EmptyToken
            | Self::OtherIoError
            | Self::OtherError
            | Self::NullError => true,
            Self::KeyringLocked
            | Self::SecretStoreFailure
            | Self::ServiceGone
            | Self::Cancelled => false,
        }
    }
}

impl std::fmt::Display for SilentRefreshFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classifies a failed `e_source_get_oauth2_access_token_sync` call by inspecting
/// the returned `GError` domain, code, and keyring status.
///
/// # Safety
///
/// If `error` is non-NULL, it must point to a valid `GError`.
pub unsafe fn classify_failure(
    error: *const GError,
) -> (
    SilentRefreshFailureReason,
    Option<String>,
    Option<String>,
    i32,
    Option<String>,
) {
    if error.is_null() {
        return (SilentRefreshFailureReason::NullError, None, None, 0, None);
    }

    let domain = unsafe { (*error).domain };
    let code = unsafe { (*error).code };
    let message = unsafe { read_string((*error).message) };

    let domain_name = unsafe {
        let ptr = glib_sys::g_quark_to_string(domain);
        read_string(ptr)
    };

    // Asked before the secret-store question because it is the narrower of
    // the two D-Bus-domain outcomes — a name nobody owns is not a store that
    // misbehaved. `is_secret_store_failure` excludes it in its own right, so
    // this order is how the pair reads rather than what makes it correct.
    let reason = if unsafe { is_service_gone(error) } {
        SilentRefreshFailureReason::ServiceGone
    } else if unsafe { is_secret_store_failure(error) } {
        SilentRefreshFailureReason::SecretStoreFailure
    } else if unsafe { is_secret_not_found(error) } {
        if crate::secret_store::default_collection_is_locked() == Some(true) {
            SilentRefreshFailureReason::KeyringLocked
        } else {
            SilentRefreshFailureReason::NoStoredSecret
        }
    } else if domain == unsafe { gio_sys::g_io_error_quark() } {
        match code {
            gio_sys::G_IO_ERROR_CONNECTION_REFUSED => {
                SilentRefreshFailureReason::ServerRejectedRefresh
            }
            gio_sys::G_IO_ERROR_NOT_SUPPORTED | gio_sys::G_IO_ERROR_FAILED => {
                SilentRefreshFailureReason::UnregisteredService
            }
            gio_sys::G_IO_ERROR_CANCELLED => SilentRefreshFailureReason::Cancelled,
            _ => SilentRefreshFailureReason::OtherIoError,
        }
    } else {
        SilentRefreshFailureReason::OtherError
    };

    (reason, Some(domain.to_string()), domain_name, code, message)
}

/// Whether `error` is a failure *underneath* EDS's own OAuth 2.0 reasoning —
/// the secret store itself, or the D-Bus call that reaches it — rather than
/// one of the outcomes `evolution-data-server`'s own OAuth 2.0 code
/// deliberately sets. Every deliberate outcome that code sets — nobody has
/// ever consented, a refresh attempt was rejected, this source has no
/// registered `EOAuth2Service` at all — is set in the `G_IO_ERROR` domain
/// (`e-oauth2-service.c`, `e-source.c`, `e-server-side-source.c`), whichever
/// code; all of them are answered [`ConnectError::OAuth2`]/`REQUIRED`
/// unchanged, on purpose — even "no registered service", which is what
/// every `ESource` this crate's own tests build (never backed by a real
/// registry) hits, and those tests already pin `REQUIRED` for it.
///
/// Confirmed by hand, not assumed (`docs/ROADMAP.md` item 17's own ask): with
/// `org.freedesktop.secrets` unable to start (a broken D-Bus activation, a
/// real, reproducible shape of a dead secret store), the exact call this
/// function classifies for answers `g_dbus_error_quark()`/
/// `G_DBUS_ERROR_SPAWN_EXEC_FAILED` — a different *domain*, which is the one
/// distinction drawn here rather than guessing at every code EDS's own
/// machinery might set.
///
/// **Two D-Bus codes are carved out**, and only two:
/// [`is_service_gone`]'s. They are the ones that mean "nothing owns that bus
/// name", which is `docs/ROADMAP.md` item 22's failure and not a store
/// failure at all.
///
/// **Does not catch every "the store is locked" case.** A collection that
/// exists but is locked, whose unlock prompt cannot be shown (no display,
/// or the user dismisses it), answers libsecret's search as "not found" —
/// by that API's own documented contract a dismissed or unshowable prompt
/// is not an error — so it is indistinguishable here from "nobody has ever
/// consented". No GError reaches this function to classify in that case;
/// there is nothing this function could do differently, and
/// [`is_secret_not_found`] plus [`crate::secret_store`] are what answer it
/// instead.
///
/// # Safety
///
/// `error` must be a valid, non-NULL `GError`.
unsafe fn is_secret_store_failure(error: *const GError) -> bool {
    // SAFETY: the caller's contract; `g_dbus_error_quark` takes no arguments.
    unsafe { (*error).domain == g_dbus_error_quark() && !is_service_gone(error) }
}

/// Whether `error` is "the bus has no owner for the name this call was
/// addressed to" — `docs/ROADMAP.md` item 22's captured failure, and the one
/// D-Bus outcome that is neither a credential nor a secret store.
///
/// **The mechanism, established from source rather than from the trace.** A
/// backend process holds the `ESource` EDS handed it for the account's
/// lifetime, and that `ESource` holds — strongly, in `priv->dbus_object`
/// (`e-source.c`, set once, cleared only in dispose or by
/// `__e_source_private_replace_dbus_object`) — the `GDBusObject` proxy its
/// `ESourceRegistry`'s `GDBusObjectManagerClient` built for it. GLib builds
/// every such interface proxy addressed to the manager's `name_owner`, which
/// is a **unique** bus name and asserted to be one
/// (`gio/gdbusobjectmanagerclient.c`, GLib 2.80.0: `"g-name", name_owner` at
/// the `g_initable_new` in `add_interfaces`, `g_dbus_is_unique_name` at that
/// function's head, and the comment "this is fine … and use a unique name").
/// When `evolution-source-registry` restarts, the manager drops the proxies
/// from *its own* map (`on_notify_g_name_owner`) but the ones an `ESource`
/// holds live on, still addressed to the unique name of the process that is
/// gone; EDS re-points them only later and asynchronously, from the
/// registry's own `GMainContext` (`source_registry_name_appeared` →
/// `source_registry_object_added_no_owner` →
/// `__e_source_private_replace_dbus_object`, `e-source-registry.c`). Any
/// synchronous token fetch inside that window is addressed at a dead peer,
/// and the bus answers `G_DBUS_ERROR_SERVICE_UNKNOWN` with the message that
/// names it: "The name :1.4 was not provided by any .service files".
///
/// **EDS 3.52 does not recover from it, and that is upstream's, not ours.**
/// `source_get_oauth2_access_token_sync` (`e-source.c`) does have an
/// in-process fallback — find the account's `EOAuth2Service` and call it
/// directly — but reaches it only when the D-Bus *interface object* is
/// absent, never when a call on a present one fails. So this classification
/// is the whole of what this crate can do about it, which is item 22's Do(3)
/// second branch: report it, never consent over it.
///
/// **The interface is present for our accounts, which is what makes any of
/// this reachable.** EDS's own registry module `module-oauth2-services.c`
/// calls `e_server_side_source_set_oauth2_support` for every server-side
/// source whose `[Authentication] Method` names a registered
/// `EOAuth2Service` — ours is `jmap_config::oauth2_service::NAME` — and that
/// setter exports the `Source.OAuth2Support` D-Bus interface at the source's
/// object path. The tests below build the resulting `GError` by hand;
/// `jmap-functional/tests/oauth2-stale-proxy.rs` is item 22's Do(1) and
/// proves real daemons produce exactly it, by holding an `ESource` across a
/// registry `SIGKILL`. If that test ever stops reporting
/// `oauth2-support-exported=1`, this classification has become unreachable
/// and the tests below would keep passing without it.
///
/// **Both codes, because a bus can answer either.** `SERVICE_UNKNOWN` is what
/// a method call on an unowned, unactivatable name gets; `NAME_HAS_NO_OWNER`
/// is the sibling other bus operations answer with. Neither is a password
/// problem and neither is fixed by signing in again.
///
/// **Deliberately not narrowed to unique names.** The peer in the captured
/// trace is a `:1.N`, and `org.freedesktop.secrets` failing to activate
/// produces the same two codes with a well-known name in the message — but
/// telling those apart would mean parsing `dbus-daemon`'s English error text,
/// which `docs/ROADMAP.md` item 17 ruled out for exactly this classification
/// ("domain **and** code, not message text"). It does not need telling apart:
/// both are "a service this sign-in needs is not running", both are `ERROR`
/// and never consent, and the message this produces names whichever one it
/// was rather than asserting which.
///
/// # Safety
///
/// `error` must be a valid, non-NULL `GError`.
unsafe fn is_service_gone(error: *const GError) -> bool {
    // SAFETY: the caller's contract; `g_dbus_error_quark` takes no arguments.
    unsafe {
        (*error).domain == g_dbus_error_quark()
            && matches!(
                (*error).code,
                G_DBUS_ERROR_SERVICE_UNKNOWN | G_DBUS_ERROR_NAME_HAS_NO_OWNER
            )
    }
}

/// Whether `error` is EDS's "this account has no stored secret" — the one
/// outcome a *locked* keyring is indistinguishable from, and so the only one
/// worth asking the secret store itself about.
///
/// `eos_lookup_token_sync` (`e-oauth2-service.c`) sets exactly this for both
/// "nobody has ever consented" and "the lookup came back empty", which is
/// what a collection whose unlock prompt cannot be shown produces — see
/// [`is_secret_store_failure`] above and [`crate::secret_store`] for why the
/// two arrive here identical.
///
/// Domain **and** code, not message text, per `docs/ROADMAP.md` item 17's own
/// ask. The narrowness is load-bearing rather than tidiness: EDS's other
/// deliberate `G_IO_ERROR`-domain outcomes must keep going to
/// [`ConnectError::OAuth2`]/`REQUIRED` untouched, and `G_IO_ERROR_NOT_SUPPORTED`
/// — what a bare `ESource` with no registry behind it answers, which is every
/// `ESource` this crate's own tests build — is the one that would otherwise
/// make those tests depend on whether the machine running them has a locked
/// keyring.
///
/// # Safety
///
/// `error` must be a valid, non-NULL `GError`.
unsafe fn is_secret_not_found(error: *const GError) -> bool {
    // SAFETY: the caller's contract; `g_io_error_quark` takes no arguments.
    unsafe { (*error).domain == g_io_error_quark() && (*error).code == G_IO_ERROR_NOT_FOUND }
}

/// What a dead peer is reported as — [`ConnectError::ServiceGone`] carrying
/// the bus's own message, which is where the peer gets *named*.
///
/// Split out of [`access_token`] only so that a test can drive it: the
/// naming is `docs/ROADMAP.md` item 22's Do(3) in full ("surface item-17-style
/// as an ERROR naming the dead peer"), and a message that quietly stopped
/// including EDS's own text would still compile, still be `ERROR`, and still
/// leave nobody able to tell which service died.
fn service_gone_error(message: Option<String>) -> ConnectError {
    ConnectError::ServiceGone(translate_with(
        // TRANSLATORS: %1$s is the message bus's own text naming which service
        // has no owner, e.g. "The name :1.4 was not provided by any .service
        // files".
        c"a service this account's sign-in needs is not running, so no access token could be fetched: %1$s",
        &[&message.unwrap_or_else(|| translate(c"no further detail was given"))],
    ))
}

/// The OAuth 2.0 access token to send this account's requests as, from
/// whichever `EOAuth2Service` claims `source`.
///
/// This is where the refresh happens: EDS looks the account's refresh token up
/// in libsecret and exchanges it for an access token inside this call, so what
/// comes back is good now. A failure is [`ConnectError::OAuth2`] when it is
/// "nobody has consented to this account yet" or "the refresh was rejected" —
/// see that variant on why it asks Evolution to authenticate rather than
/// discard what it has — and [`ConnectError::SecretStore`] when it is
/// something underneath that decision instead, per
/// `is_secret_store_failure` below.
///
/// # Safety
///
/// `source` must be a valid `ESource` and `cancellable` NULL or a valid
/// `GCancellable` — which is what an EDS vfunc receives. Nothing here outlives
/// the call.
pub unsafe fn access_token(
    source: *mut ESource,
    cancellable: *mut GCancellable,
) -> Result<String, ConnectError> {
    let account_id = unsafe { read_string(eds_sys::e_source_get_uid(source)) };
    tracing::debug!(?account_id, "fetching OAuth 2.0 access token via EDS");
    let mut token = ptr::null_mut();
    let mut expires_in = 0;
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: a valid source and cancellable by this function's contract, and
    // three writable out-parameters. The token comes back as a GLib allocation
    // this call owns, and the GError likewise.
    let ok = unsafe {
        e_source_get_oauth2_access_token_sync(
            source,
            cancellable,
            &mut token,
            &mut expires_in,
            &mut error,
        )
    };

    if ok == GFALSE || token.is_null() {
        // SAFETY: `error` is NULL or a GError this call owns; reading its
        // message and classifying its domain/code borrow fields the struct
        // owns, and freeing it afterwards is what the out-parameter contract
        // asks for.
        let (
            reason,
            error_domain,
            error_domain_name,
            error_code,
            message,
            store_failure,
            secret_not_found,
            service_gone,
        ) = unsafe {
            let (r, dq, dn, c, msg) = classify_failure(error);
            let sf = if error.is_null() {
                false
            } else {
                is_secret_store_failure(error)
            };
            let snf = if error.is_null() {
                false
            } else {
                is_secret_not_found(error)
            };
            let sg = if error.is_null() {
                false
            } else {
                is_service_gone(error)
            };
            if !error.is_null() {
                g_error_free(error);
            }
            (r, dq, dn, c, msg, sf, snf, sg)
        };

        let escalates_to_consent = reason.escalates_to_consent();
        let reason_str = reason.as_str();

        tracing::debug!(
            ?account_id,
            reason = reason_str,
            escalates_to_consent,
            error_domain = error_domain.as_deref(),
            error_domain_name = error_domain_name.as_deref(),
            error_code,
            error_message = message.as_deref(),
            store_failure,
            secret_not_found,
            service_gone,
            "failed to obtain OAuth 2.0 access token"
        );

        // Asked before the store question, the same order `classify_failure`
        // uses and for the same reason: both are D-Bus-domain failures, and
        // this one is not the store's.
        if service_gone {
            return Err(service_gone_error(message));
        }

        if store_failure {
            return Err(ConnectError::SecretStore(translate_with(
                // TRANSLATORS: %1$s is EDS's own message for why the
                // account's secret store (keyring) could not be reached.
                c"the account's secret store (keyring) could not be reached: %1$s",
                &[&message.unwrap_or_else(|| translate(c"no further detail was given"))],
            )));
        }

        // "No stored secret" is the one answer EDS gives for two different
        // situations — nobody has consented yet, and the keyring holding the
        // consent is locked — so it is the one answer worth a second
        // question. Asked here, *after* the fetch has already failed and
        // only to classify that failure, which is what makes the inherent
        // race harmless: a store unlocked between the fetch and this call
        // answers "not locked" and the account gets the consent window it
        // would have got anyway, while one locked in that window is a store
        // that really is locked. Nothing is skipped or retried on the
        // strength of the answer, so it can only ever change the message.
        //
        // `Some(true)` and nothing else: a secret service that could not be
        // reached, or that is not there at all, answers `None`, and a
        // machine with no keyring must keep behaving exactly as before —
        // see `crate::secret_store`.
        if secret_not_found && crate::secret_store::default_collection_is_locked() == Some(true) {
            tracing::debug!(
                ?account_id,
                reason = "keyring_locked",
                escalates_to_consent = false,
                "the secret store is locked; not asking for consent"
            );
            return Err(ConnectError::SecretStore(translate(
                // TRANSLATORS: shown instead of a fresh sign-in window when
                // the account's stored token cannot be read because the
                // system keyring is locked, which signing in again cannot
                // fix.
                c"the login keyring is locked, so this account's OAuth 2.0 token can be neither read nor stored; unlock the keyring and try again",
            )));
        }

        // The other way a store can be no use to us: not there at all. A
        // sign-in window offered here would complete and then have nowhere to
        // put the token, so the very next fetch would ask again — the user
        // does the whole dance and nothing keeps it. Erroring out is the
        // honest outcome (maintainer, 2026-08-29).
        //
        // `Some(false)` and nothing else, the same discipline the locked
        // check above uses and for the same reason. A desktop whose keyring
        // simply has not been started yet answers `Some(true)` — the bus can
        // activate it — and a machine that cannot be asked answers `None`;
        // both must keep behaving exactly as they did before this check
        // existed. Only "the bus knows of no such service, running or
        // startable" is a machine where consent cannot lead anywhere.
        if secret_not_found && crate::secret_store::service_is_available() == Some(false) {
            tracing::debug!(
                ?account_id,
                reason = "no_secret_store",
                escalates_to_consent = false,
                "no secret store to keep a token in; not asking for consent"
            );
            return Err(ConnectError::SecretStore(translate(
                // TRANSLATORS: shown instead of a fresh sign-in window when
                // the system has no keyring service at all, so a token
                // obtained by signing in could not be saved anywhere.
                c"this system has no keyring service to store an OAuth 2.0 token in, so signing in again would not keep this account signed in; enable a keyring service and try again",
            )));
        }

        // A failure with no GError should not happen — EDS sets one on every
        // path — but a NULL message is not worth turning into a panic in a
        // backend, so it becomes a sentence that says exactly what is known.
        return Err(ConnectError::OAuth2(message.unwrap_or_else(|| {
            translate(
                // TRANSLATORS: shown when EDS refused an OAuth 2.0 account's
                // access token but did not say why.
                c"the account has no OAuth 2.0 access token",
            )
        })));
    }

    // SAFETY: non-NULL on success, a NUL-terminated GLib allocation this scope
    // owns; copied out and then freed, as `(transfer full)` requires.
    let value = unsafe {
        let value = read_string(token);
        g_free(token.cast());
        value
    };

    // `read_string` answers `None` for NULL only, which `token.is_null()`
    // already ruled out; an empty token is a token the server would reject, and
    // saying so here is better than sending `Authorization: Bearer `.
    match value {
        Some(value) if !value.is_empty() => {
            tracing::debug!(?account_id, expires_in, "obtained OAuth 2.0 access token");
            Ok(value)
        }
        _ => {
            tracing::debug!(
                ?account_id,
                reason = "empty_token",
                escalates_to_consent = true,
                "empty OAuth 2.0 access token received"
            );
            Err(ConnectError::OAuth2(translate(
                // TRANSLATORS: shown when EDS handed back an OAuth 2.0 access
                // token that was empty rather than absent.
                c"the OAuth 2.0 service returned an empty access token",
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use gio_sys::{
        G_DBUS_ERROR_NAME_HAS_NO_OWNER, G_DBUS_ERROR_SERVICE_UNKNOWN,
        G_DBUS_ERROR_SPAWN_EXEC_FAILED, G_IO_ERROR_NOT_FOUND, G_IO_ERROR_NOT_SUPPORTED,
        g_dbus_error_quark, g_io_error_quark,
    };
    use glib_sys::g_error_new_literal;

    use super::*;

    /// Builds a real `GError` rather than a fake struct: [`is_secret_store_failure`]
    /// reads its `domain` field directly, so the test should exercise the same
    /// GLib allocation the production code does, not a hand-rolled substitute.
    fn error(domain: glib_sys::GQuark, code: i32) -> *mut GError {
        let message = CString::new("boom").unwrap();
        // SAFETY: a valid domain and a NUL-terminated message; the result is
        // freed by every caller below.
        unsafe { g_error_new_literal(domain, code, message.as_ptr()) }
    }

    /// `G_IO_ERROR`, whichever code, is the domain EDS's own OAuth 2.0 code
    /// deliberately sets every outcome in — including "no registered
    /// service", not just "no grant yet" — and none of it is a secret-store
    /// failure. The `jmap-backend-core/tests/oauth2.rs` integration test
    /// exercises the "no registered service" shape end to end and expects
    /// `REQUIRED`; this pins the unit underneath it.
    #[test]
    fn the_g_io_error_domain_is_never_a_secret_store_failure() {
        for code in [G_IO_ERROR_NOT_FOUND, G_IO_ERROR_NOT_SUPPORTED] {
            let error = error(unsafe { g_io_error_quark() }, code);
            assert!(
                !unsafe { is_secret_store_failure(error) },
                "expected code {code} to not be a secret-store failure"
            );
            unsafe { g_error_free(error) };
        }
    }

    /// The real, reproduced shape of a dead secret store: a broken
    /// `org.freedesktop.secrets` D-Bus activation answers
    /// `g_dbus_error_quark()`/`G_DBUS_ERROR_SPAWN_EXEC_FAILED`, confirmed by
    /// hand against a real `gnome-keyring-daemon` whose `.service` file
    /// pointed at a missing binary — a different domain from `G_IO_ERROR`,
    /// and this must not open a fresh consent window.
    #[test]
    fn a_broken_secret_service_is_a_secret_store_failure() {
        let error = error(
            unsafe { g_dbus_error_quark() },
            G_DBUS_ERROR_SPAWN_EXEC_FAILED,
        );
        assert!(unsafe { is_secret_store_failure(error) });
        unsafe { g_error_free(error) };
    }

    /// The shape `eos_lookup_token_sync` gives both for "nobody has ever
    /// consented" and for a lookup a locked collection answered empty — the
    /// only outcome [`access_token`] asks the secret store itself about.
    #[test]
    fn a_missing_secret_is_the_one_ambiguous_outcome() {
        let error = error(unsafe { g_io_error_quark() }, G_IO_ERROR_NOT_FOUND);
        assert!(unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }

    /// `G_IO_ERROR_NOT_SUPPORTED` is what an `ESource` with no registry
    /// behind it answers — every `ESource` this crate's own tests build, and
    /// `tests/oauth2.rs`/`jmap-backend-collection`'s `tests/authenticate.rs`
    /// both pin it to `REQUIRED`. It must not reach the secret store
    /// question, or those tests would start depending on whether the machine
    /// running them has a locked keyring.
    #[test]
    fn no_registered_service_is_not_a_missing_secret() {
        let error = error(unsafe { g_io_error_quark() }, G_IO_ERROR_NOT_SUPPORTED);
        assert!(!unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }

    /// Domain and code, not code alone: `G_IO_ERROR_NOT_FOUND` is the
    /// integer 1, which is a perfectly ordinary code in every other error
    /// domain too — including the D-Bus one this module already classifies
    /// separately.
    #[test]
    fn a_missing_secret_is_recognised_by_domain_not_by_code_alone() {
        let error = error(unsafe { g_dbus_error_quark() }, G_IO_ERROR_NOT_FOUND);
        assert!(!unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }

    /// D-Bus timeouts and spawn failures underneath libsecret are in the
    /// `g_dbus_error_quark()` domain and are classified as secret-store
    /// failures. `SERVICE_UNKNOWN`/`NAME_HAS_NO_OWNER` are deliberately *not*
    /// among them any more — see
    /// [`the_dead_peer_codes_are_not_blamed_on_the_keyring`].
    #[test]
    fn dbus_timeout_and_transport_errors_are_secret_store_failures() {
        // Various D-Bus codes (e.g. TIMEOUT=24, NO_REPLY=4)
        for code in [4, 24, G_DBUS_ERROR_SPAWN_EXEC_FAILED] {
            let error = error(unsafe { g_dbus_error_quark() }, code);
            assert!(
                unsafe { is_secret_store_failure(error) },
                "expected D-Bus error code {code} to be classified as secret-store failure"
            );
            unsafe { g_error_free(error) };
        }
    }

    /// The captured shape of `docs/ROADMAP.md` item 22: a token fetch that
    /// dies with `G_DBUS_ERROR_SERVICE_UNKNOWN` ("The name :1.4 was not
    /// provided by any .service files") because the `ESource` still holds a
    /// `GDBusObjectManagerClient` proxy addressed to a registry that has
    /// since restarted.
    ///
    /// It must not be reported as a keyring failure: the keyring is fine, and
    /// telling someone to unlock it sends them at the wrong thing. Both codes
    /// a bus answers for "nobody owns that name" are covered.
    #[test]
    fn the_dead_peer_codes_are_not_blamed_on_the_keyring() {
        for code in [G_DBUS_ERROR_SERVICE_UNKNOWN, G_DBUS_ERROR_NAME_HAS_NO_OWNER] {
            let error = error(unsafe { g_dbus_error_quark() }, code);
            assert!(
                unsafe { is_service_gone(error) },
                "expected D-Bus error code {code} to be classified as a dead peer"
            );
            assert!(
                !unsafe { is_secret_store_failure(error) },
                "expected D-Bus error code {code} to stop being a secret-store failure"
            );
            unsafe { g_error_free(error) };
        }
    }

    /// A dead peer is `ServiceGone`, and — like every other infrastructure
    /// failure item 17 classified — it never escalates to a consent window.
    /// This is item 22's Do(3) in one assertion.
    #[test]
    fn a_dead_peer_never_escalates_to_consent() {
        let dead = error(
            unsafe { g_dbus_error_quark() },
            G_DBUS_ERROR_SERVICE_UNKNOWN,
        );
        let (reason, _quark, _domain_name, code, _msg) = unsafe { classify_failure(dead) };
        assert_eq!(reason, SilentRefreshFailureReason::ServiceGone);
        assert_eq!(reason.as_str(), "service_gone");
        assert!(!reason.escalates_to_consent());
        assert_eq!(code, G_DBUS_ERROR_SERVICE_UNKNOWN);
        unsafe { g_error_free(dead) };
    }

    /// The captured failure end to end, built the way production builds it
    /// rather than by hardcoding a code.
    ///
    /// `g_dbus_error_new_for_dbus_error` is the exact conversion
    /// `gdbusconnection.c` applies to a bus reply, and
    /// `org.freedesktop.DBus.Error.ServiceUnknown` with this message is what
    /// `dbus-daemon` really answers a method call addressed at an unowned
    /// unique name — confirmed by hand on this machine, not assumed:
    ///
    /// ```text
    /// $ dbus-run-session -- dbus-send --session --print-reply \
    ///       --dest=:1.9999 /org/freedesktop/DBus org.freedesktop.DBus.Peer.Ping
    /// Error org.freedesktop.DBus.Error.ServiceUnknown: The name :1.9999 was
    /// not provided by any .service files
    /// ```
    ///
    /// `g_dbus_error_strip_remote_error` is then EDS's own next line
    /// (`source_get_oauth2_access_token_sync`, `e-source.c`), which is why the
    /// message the trace shows has no `GDBus.Error:` prefix. So this is the
    /// real `GError` [`access_token`] classifies, minus only the dead registry.
    #[test]
    fn the_wire_error_a_dead_peer_produces_is_service_gone_and_still_names_it() {
        let name = CString::new("org.freedesktop.DBus.Error.ServiceUnknown").unwrap();
        let text = CString::new("The name :1.4 was not provided by any .service files").unwrap();
        // SAFETY: two NUL-terminated strings; the `GError` returned is owned
        // here and freed below.
        let error = unsafe {
            let error = gio_sys::g_dbus_error_new_for_dbus_error(name.as_ptr(), text.as_ptr());
            gio_sys::g_dbus_error_strip_remote_error(error);
            error
        };

        assert!(unsafe { is_service_gone(error) });
        let (reason, _quark, _domain_name, _code, message) = unsafe { classify_failure(error) };
        assert_eq!(reason, SilentRefreshFailureReason::ServiceGone);
        assert!(!reason.escalates_to_consent());

        // The peer has to survive into what the user is shown; that is the
        // whole of "naming the dead peer".
        let reported = service_gone_error(message).to_string();
        assert!(
            reported.contains(":1.4"),
            "expected the dead peer to be named, got {reported:?}"
        );
        assert!(
            !reported.to_lowercase().contains("keyring"),
            "expected the keyring not to be blamed, got {reported:?}"
        );

        unsafe { g_error_free(error) };
    }

    /// Domain as well as code, the same narrowness
    /// [`a_missing_secret_is_recognised_by_domain_not_by_code_alone`] pins:
    /// `G_DBUS_ERROR_SERVICE_UNKNOWN` is the integer 2, an entirely ordinary
    /// code in `G_IO_ERROR` too (`G_IO_ERROR_NOT_FOUND` is 1, `EXISTS` 2).
    #[test]
    fn a_dead_peer_is_recognised_by_domain_not_by_code_alone() {
        let error = error(unsafe { g_io_error_quark() }, G_DBUS_ERROR_SERVICE_UNKNOWN);
        assert!(!unsafe { is_service_gone(error) });
        unsafe { g_error_free(error) };
    }

    /// Classifies each category of silent-refresh failure into its attributable
    /// reason, domain name, code, and whether it escalates to an interactive consent prompt.
    #[test]
    fn classify_failure_covers_all_silent_refresh_failure_reasons() {
        use gio_sys::{G_IO_ERROR_CANCELLED, G_IO_ERROR_CONNECTION_REFUSED, G_IO_ERROR_FAILED};

        // D-Bus / Secret store failure -> SecretStoreFailure, no consent escalation
        let dbus_err = error(
            unsafe { g_dbus_error_quark() },
            G_DBUS_ERROR_SPAWN_EXEC_FAILED,
        );
        let (reason, _quark, domain_name, code, msg) = unsafe { classify_failure(dbus_err) };
        assert_eq!(reason, SilentRefreshFailureReason::SecretStoreFailure);
        assert_eq!(reason.as_str(), "secret_store_failure");
        assert!(!reason.escalates_to_consent());
        assert_eq!(code, G_DBUS_ERROR_SPAWN_EXEC_FAILED);
        assert_eq!(msg.as_deref(), Some("boom"));
        assert!(domain_name.is_some());
        unsafe { g_error_free(dbus_err) };

        // Missing secret -> NoStoredSecret (assuming unlocked keyring in test), escalates to consent
        let not_found = error(unsafe { g_io_error_quark() }, G_IO_ERROR_NOT_FOUND);
        let (reason, _quark, _domain_name, code, _msg) = unsafe { classify_failure(not_found) };
        assert!(
            reason == SilentRefreshFailureReason::NoStoredSecret
                || reason == SilentRefreshFailureReason::KeyringLocked
        );
        assert_eq!(code, G_IO_ERROR_NOT_FOUND);
        unsafe { g_error_free(not_found) };

        // Server refresh rejection (HTTP 400 Bad Request / invalid_grant / rotation mismatch) -> ServerRejectedRefresh
        let refused = error(unsafe { g_io_error_quark() }, G_IO_ERROR_CONNECTION_REFUSED);
        let (reason, _quark, domain_name, code, _msg) = unsafe { classify_failure(refused) };
        assert_eq!(reason, SilentRefreshFailureReason::ServerRejectedRefresh);
        assert_eq!(reason.as_str(), "server_rejected_refresh");
        assert!(reason.escalates_to_consent());
        assert_eq!(code, G_IO_ERROR_CONNECTION_REFUSED);
        assert_eq!(domain_name.as_deref(), Some("g-io-error-quark"));
        unsafe { g_error_free(refused) };

        // Unregistered service / not supported -> UnregisteredService
        for code in [G_IO_ERROR_NOT_SUPPORTED, G_IO_ERROR_FAILED] {
            let err = error(unsafe { g_io_error_quark() }, code);
            let (reason, _quark, _domain_name, c, _msg) = unsafe { classify_failure(err) };
            assert_eq!(reason, SilentRefreshFailureReason::UnregisteredService);
            assert_eq!(reason.as_str(), "unregistered_service");
            assert!(reason.escalates_to_consent());
            assert_eq!(c, code);
            unsafe { g_error_free(err) };
        }

        // Cancelled -> Cancelled, no consent escalation
        let cancelled = error(unsafe { g_io_error_quark() }, G_IO_ERROR_CANCELLED);
        let (reason, _quark, _domain_name, code, _msg) = unsafe { classify_failure(cancelled) };
        assert_eq!(reason, SilentRefreshFailureReason::Cancelled);
        assert_eq!(reason.as_str(), "cancelled");
        assert!(!reason.escalates_to_consent());
        assert_eq!(code, G_IO_ERROR_CANCELLED);
        unsafe { g_error_free(cancelled) };

        // Other generic IO error
        let other_io = error(unsafe { g_io_error_quark() }, 999);
        let (reason, _quark, _domain_name, code, _msg) = unsafe { classify_failure(other_io) };
        assert_eq!(reason, SilentRefreshFailureReason::OtherIoError);
        assert_eq!(reason.as_str(), "other_io_error");
        assert!(reason.escalates_to_consent());
        assert_eq!(code, 999);
        unsafe { g_error_free(other_io) };

        // Null error pointer -> NullError
        let (reason, quark, domain_name, code, msg) = unsafe { classify_failure(ptr::null()) };
        assert_eq!(reason, SilentRefreshFailureReason::NullError);
        assert_eq!(reason.as_str(), "null_error");
        assert!(reason.escalates_to_consent());
        assert_eq!(quark, None);
        assert_eq!(domain_name, None);
        assert_eq!(code, 0);
        assert_eq!(msg, None);

        // Empty token
        assert_eq!(
            SilentRefreshFailureReason::EmptyToken.as_str(),
            "empty_token"
        );
        assert!(SilentRefreshFailureReason::EmptyToken.escalates_to_consent());
    }
}
