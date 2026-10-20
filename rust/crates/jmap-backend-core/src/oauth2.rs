// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether an account authenticates with OAuth 2.0, and the access token if it
//! does.
//!
//! ## The rule is EDS's, copied and not invented
//!
//! Nothing here decides *policy*. Every WebDAV, CalDAV and CardDAV account in
//! Evolution authenticates through `e_soup_session_setup_message_credentials`
//! (evolution-data-server 3.52.3, `src/libedataserver/e-soup-session.c`). It
//! selects like this:
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
//! The bearer branch gets its token from
//! `e_source_get_oauth2_access_token_sync`. [`method_is_oauth2`] and
//! [`access_token`] are those two lines for JMAP. The rule is copied exactly,
//! not approximated. Other parts of Evolution (the setup UI, a hand-edited
//! keyfile, a future EDS release) write accounts to satisfy this rule. If a
//! JMAP account read the same keyfield differently, Evolution would think it
//! is OAuth2 and this code would ask for a password.
//!
//! The `"OAuth2"` literal and the alias lookup are two different questions,
//! and the code asks both. `"OAuth2"` is the generic spelling: "the
//! registered service that claims this source". An alias is the
//! [`name`][name] of a *particular* service in the field. This project's own
//! setup writes an alias (`jmap_config::oauth2_service::NAME`, `"JMAP"`). The
//! alias lookup is a live query against the registered services, not a
//! constant list. Thus it answers yes for our service only when
//! `module-jmap-backend.so` has registered it. That is also the only
//! condition in which the token fetch can work.
//!
//! [name]: ../../jmap_config/oauth2_service/constant.NAME.html
//!
//! ## Why the services object is held and not made per question
//!
//! The copy of EDS's rule above has one difference: which of the two
//! spellings of the alias lookup it calls.
//! `e_soup_session_setup_message_credentials` uses
//! `e_oauth2_services_is_oauth2_alias_static()`. On every call, that function
//! creates an `EOAuth2Services`, queries it and drops that reference again.
//! Its own documentation names the precondition that makes this safe:
//!
//! > The #EOAuth2Services is implemented as a singleton, thus it won't be much
//! > trouble, **as long as there is something else having created one
//! > instance.**
//!
//! If nothing else has, the reference it drops is the last one, and two
//! threads that ask at the same time race. One is inside
//! `oauth2_services_dispose`, which frees `priv->services` without clearing
//! the field. The other is inside `oauth2_services_constructor`, which still
//! sees the not-yet-cleared `services_singleton` and takes a reference to it.
//! This is a legal GObject resurrection of an object whose service list is
//! already freed. The code then walks the resurrected instance and
//! dereferences a dangling `EOAuth2Service`. The result is a SIGSEGV in the
//! process that asked. For a collection backend, that process is
//! `evolution-source-registry`, so all accounts in the session fail.
//!
//! Every real Evolution process satisfies the precondition by chance:
//! `e_source_registry_init()` holds an `EOAuth2Services` for the registry's
//! lifetime. Thus *today*, nothing gets this crash by accident. This module
//! intentionally holds its own: a single instance, created on the first
//! question and never released. Thus this module satisfies EDS's
//! precondition itself and does not depend on other code in the process.
//! `e_oauth2_services_is_oauth2_alias()` is then the same lookup that
//! `_static` makes, without the create-and-destroy around it.
//!
//! **To hold one does *not* change which services the lookup finds.** An
//! `EOAuth2Services` loads its extensions one time, at construction
//! (`e_extensible_load_extensions`). Thus the services that answer are the
//! ones registered when the instance was built. In a process that holds a
//! registry, the instance is the registry's. `_static` already returned that
//! same instance and never constructed its own, because the singleton was
//! alive the full time. One more reference to it does not change when it was
//! built. This fixes the instance's age only where nothing else holds one,
//! and that is exactly where it fixes the crash. This module intentionally
//! does *not* call `e_extensible_reload_extensions()` to widen that again. It
//! changes the extension array without a lock of its own. A call per question
//! would thus add back a race of the same kind that this removes.
//!
//! ## What this module intentionally does not examine again
//!
//! **That the connection is encrypted.** EDS's own rule pre-fills credentials
//! only on `https`. This code sends a bearer token over the origin that
//! [`SourceConfig`](crate::source::SourceConfig) resolved. But this is the same
//! rule, not a weaker one, enforced one layer higher:
//! [`source::origin`](crate::source::origin) refuses to build a non-TLS origin
//! for a host that is not loopback. Thus a token can leave this process in
//! clear only over a connection to this machine (the `jmap-mockd` development
//! case). A second TLS test here would be a rule that exists twice, and a
//! later correction would change only one of them.
//!
//! **The token's lifetime.** `e_source_get_oauth2_access_token_sync` also
//! gives the number of seconds that the token is valid, and this code ignores
//! it. A connect fetches a token and uses it for that connect. The next
//! connect asks again. EDS refreshes an expired token inside that call. Thus
//! the number is useful only to code that keeps a connection open across the
//! expiry and authenticates again in place. No JMAP backend here does that.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, EOAuth2Services, ESource, ESourceAuthentication,
    ESourceCredentialsProvider, e_oauth2_services_is_oauth2_alias, e_oauth2_services_new,
    e_source_authentication_get_method, e_source_authentication_get_type,
    e_source_credentials_provider_new, e_source_credentials_provider_ref_credentials_source,
    e_source_get_oauth2_access_token_sync, e_source_get_parent, e_source_registry_new_sync,
};
use gio_sys::{
    G_DBUS_ERROR_NAME_HAS_NO_OWNER, G_DBUS_ERROR_SERVICE_UNKNOWN, G_IO_ERROR_NOT_FOUND,
    GCancellable, g_dbus_error_quark, g_io_error_quark,
};
use glib_sys::{GError, GFALSE, g_error_free, g_free};

use crate::connect::ConnectError;
use crate::forever::Forever;
use crate::i18n::{translate, translate_with};
use crate::marshal::{extension_if_present, read_string};

/// The generic spelling of "this source authenticates with OAuth 2.0", not
/// the name of one particular service.
pub const OAUTH2_METHOD: &str = "OAuth2";

/// The name of *this project's* `EOAuth2Service`. [`OAUTH2_METHOD`] is the
/// generic counterpart of this particular-service spelling.
///
/// It is here and not in the crate that implements the service
/// ([`jmap_config::oauth2_service::NAME`], which is now this constant).
/// Three separate places must agree on the same literal, and only two of
/// them are in that crate:
///
/// - `e_oauth2_service_get_name` returns it, and
///   `EOAuth2Service::can_process`'s default implementation compares
///   `[Authentication] Method` against it. Thus the setup UI writes it into
///   an account that uses OAuth 2.0.
/// - [`method_is_oauth2`] recognises it through
///   `e_oauth2_services_is_oauth2_alias`. So does
///   `mail_ui_session_authenticate_sync` when it decides if a rejected
///   silent attempt can recover through the consent window.
/// - [`jmap_mail::sasl`] gives it as its `CamelServiceAuthType::authproto`.
///   Thus `camel_sasl_authtype` answers for the same string that
///   `mail_config_auth_check_host_changed_cb` uses to look up the mechanism
///   (`camel_sasl_authtype (e_oauth2_service_get_name (oauth2_service))`,
///   evolution 3.52.4).
///
/// `jmap-mail` cannot see `jmap-config`, because the Camel provider does not
/// depend on the account-editor module. A literal in each crate would be two
/// spellings that nothing compares. A rename of one would silently change
/// the mail half of an OAuth 2.0 account back to a password prompt.
///
/// [`jmap_config::oauth2_service::NAME`]: ../../jmap_config/oauth2_service/constant.NAME.html
/// [`jmap_mail::sasl`]: ../../jmap_mail/sasl/index.html
pub const OAUTH2_SERVICE_NAME: &CStr = c"JMAP";

/// The `[Authentication] Method` string of an OAuth 2.0 JMAP account. All
/// parts of this project must agree on this spelling. It is also the only
/// one that `EOAuth2Service::can_process`'s default implementation accepts
/// (`eos_default_can_process`, e-oauth2-service.c: it compares the source's
/// method against `e_oauth2_service_get_name` and nothing else).
///
/// On an EDS that has no dynamically-registered-client service of its own,
/// this is [`OAUTH2_SERVICE_NAME`], the name of the `EOAuth2Service` that this
/// project registers. On EDS 3.63.1 and newer, it is the name of upstream's
/// `EOAuth2ServiceDynamic`. Its `get_name` returns `"OAuth2Dynamic"`, and it
/// reads the client registration that this project already stores, from its
/// own `[OAuth2 Client]` group. Where upstream has it, this project registers
/// no service of its own. If two services claim one account, the result of
/// `e_oauth2_services_find` is random.
///
/// This name is intentionally different from [`OAUTH2_SERVICE_NAME`]. The two
/// were once the same string and thus the same constant. That is how a method
/// name gets an update in one of the five places that compare against it,
/// and stays stale in the other four. A reader here always asks "what does
/// this account's `method` say", never "what is our own service called".
/// Only the first changes with the EDS version.
#[cfg(not(eds_oauth2_dynamic))]
pub const OAUTH2_AUTH_METHOD: &CStr = OAUTH2_SERVICE_NAME;

/// The `#[cfg(not(eds_oauth2_dynamic))]` arm above has this constant's
/// documentation.
#[cfg(eds_oauth2_dynamic)]
pub const OAUTH2_AUTH_METHOD: &CStr = c"OAuth2Dynamic";

/// Whether the EDS used for this build has its own `EOAuth2ServiceDynamic`.
/// If so, this project must disable its own `EOAuth2Service` and `CamelSasl`
/// and use that one.
///
/// This is a `const bool`, not a second `#[cfg]` for every caller. Thus the
/// crates that only *skip a registration* (the three backend modules, the
/// Camel provider) need neither the cfg nor a `build.rs` that re-emits it.
/// The decision also stays in one place, with the constant it comes from.
#[cfg(not(eds_oauth2_dynamic))]
pub const EDS_HAS_DYNAMIC_OAUTH2: bool = false;

/// See the `#[cfg(not(eds_oauth2_dynamic))]` arm above.
#[cfg(eds_oauth2_dynamic)]
pub const EDS_HAS_DYNAMIC_OAUTH2: bool = true;

/// The process's `EOAuth2Services`, alive for the full life of the process.
/// The module docs tell why a held instance is necessary to ask the question
/// safely. It is a [`Forever`], intentionally never released: the absence of
/// a release is what makes the singleton safe to query. All code below only
/// reads it.
static SERVICES: OnceLock<Forever<EOAuth2Services>> = OnceLock::new();

/// The held `EOAuth2Services`, created on the first question.
///
/// `OnceLock` closes the window that the module docs describe, and does not
/// only make it smaller. The first thread that asks creates the instance
/// while all others wait. Thus no second thread can be part-way through a
/// create-or-destroy of the singleton while this one runs. After that, this
/// module holds a reference, so nothing else in the process can drop the
/// last one. This includes EDS's own transient `ESourceRegistry` inside
/// [`access_token`]: the code gets there only *after* a question, thus after
/// this function ran.
fn services() -> *mut EOAuth2Services {
    SERVICES
        .get_or_init(|| {
            // SAFETY: no arguments. The returned reference is transfer-full
            // and intentionally never given back. The pointer goes only to
            // EDS's own accessors. The ones that this module calls take the
            // instance's `property_lock` around every read of the service
            // list (`e-oauth2-services.c`).
            unsafe { Forever::new(e_oauth2_services_new()) }
        })
        .as_ptr()
}

/// The process's `ESourceCredentialsProvider` and the `ESourceRegistry` that
/// it needs to stay usable. Both stay alive for the full life of the process.
///
/// There are two pointers because `e_source_credentials_provider_new` keeps
/// only a `GWeakRef` to its registry (`e-source-credentials-provider.c`). If
/// the registry is released, every lookup of the provider returns NULL. To
/// hold both is the same intentional never-release as [`SERVICES`] above,
/// for a stronger reason. An `ESourceRegistry` is a D-Bus client that loads
/// the full source list on construction. One per question would add a round
/// trip before every connect.
struct Credentials {
    provider: Forever<ESourceCredentialsProvider>,
    _registry: Forever<eds_sys::ESourceRegistry>,
}

static CREDENTIALS: OnceLock<Credentials> = OnceLock::new();

/// The held provider, created on the first child source that asks.
///
/// NULL when no registry is available, which is a process with no session
/// bus: a unit test, or a backend whose daemon is gone. That means "resolve
/// to nothing", and [`credentials_source`] then does not change the source.
/// This is the same behaviour as before this function existed.
fn credentials_provider() -> *mut ESourceCredentialsProvider {
    CREDENTIALS
        .get_or_init(|| {
            let mut error: *mut GError = ptr::null_mut();
            // SAFETY: a NULL cancellable and a writable out-parameter. The
            // registry comes back as a reference that this holder keeps.
            // Because of EDS's own singleton, a process that already has a
            // registry gets that one, and EDS does not build a second.
            let registry = unsafe { e_source_registry_new_sync(ptr::null_mut(), &mut error) };
            if registry.is_null() {
                // SAFETY: NULL or a GError that this call owns. The struct
                // owns the message string, and this code only reads it.
                let message = unsafe {
                    if error.is_null() {
                        None
                    } else {
                        read_string((*error).message)
                    }
                };
                tracing::debug!(
                    reason = ?message,
                    "no source registry, so a child account's token is fetched on the child"
                );
                // SAFETY: as above.
                unsafe {
                    if !error.is_null() {
                        g_error_free(error);
                    }
                }
                // SAFETY: NULL, which nothing dereferences.
                return unsafe {
                    Credentials {
                        provider: Forever::new(ptr::null_mut()),
                        _registry: Forever::new(ptr::null_mut()),
                    }
                };
            }
            // SAFETY: a live registry from the call above.
            let provider = unsafe { e_source_credentials_provider_new(registry) };
            // SAFETY: this code keeps both references and never gives them
            // back. The provider pointer goes only to
            // `e_source_credentials_provider_ref_credentials_source`. That
            // function takes the registry's own lock around its source
            // lookups and returns a reference of its own. This code never
            // dereferences the registry pointer.
            unsafe {
                Credentials {
                    provider: Forever::new(provider),
                    _registry: Forever::new(registry),
                }
            }
        })
        .provider
        .as_ptr()
}

/// The source whose stored credentials `source` actually uses: its
/// collection if it has one, otherwise `source` itself.
///
/// ## Why this is not the source the backend was built from
///
/// An account's mail, address books and calendars are separate `ESource`s
/// under one collection. Only the collection holds the refresh token. EDS
/// still answers `e_source_get_oauth2_access_token_sync` on all of them, so a
/// request on the child *works*, and that is the trap. EDS 3.52 computes the
/// answer for the given source. Thus two children that ask at the same time
/// cause two independent refresh exchanges, and each uses the same stored
/// refresh token. A server that rotates refresh tokens sees the second use as
/// a replay and revokes the full grant. The result is a consent prompt for
/// every service on the account at the same time. Measured against Fastmail
/// on 3.52.3: three children, three exchanges in two seconds, then `401`
/// with `a revocation tombstone stands for this session`.
///
/// Evolution's own mail session also resolves first, before the same call
/// (`mail_session_get_oauth2_access_token_sync`,
/// `src/libemail-engine/e-mail-session.c`). The rule it applies is EDS's
/// [`e_source_credentials_provider_ref_credentials_source`]: go to the
/// nearest collection ancestor, then keep it only if
/// `e_util_can_use_collection_as_credential_source` agrees that the two
/// sources really share a login.
///
/// EDS 3.61 also resolves internally (eds#645). There, this is redundant but
/// not wrong: the walk finds the collection that EDS already has. It stays
/// for 3.52, which is the version this project targets.
///
/// # Safety
///
/// `source` must be a valid `ESource`. The returned pointer is either
/// `source` itself, borrowed, or a reference that this function transfers to
/// the caller. The `owned` flag beside it tells which.
unsafe fn credentials_source(source: *mut ESource) -> (*mut ESource, bool) {
    // Only a source with a parent can have a collection above it. The lookup
    // needs a provider, which needs a registry. A registry built inside
    // `evolution-source-registry` makes the process call itself over D-Bus.
    // The collection backend runs there and gives its own collection source,
    // whose parent is empty, so this returns first.
    // SAFETY: a valid source by this function's contract. The source owns the
    // parent string, and this code only reads it.
    let parent = unsafe { read_string(e_source_get_parent(source)) };
    if parent.is_none_or(|parent| parent.is_empty()) {
        return (source, false);
    }

    let provider = credentials_provider();
    if provider.is_null() {
        return (source, false);
    }

    // SAFETY: a live provider that this module holds, and a valid source. The
    // returned reference is transfer-full, as the `true` tells.
    let holder = unsafe { e_source_credentials_provider_ref_credentials_source(provider, source) };
    if holder.is_null() {
        return (source, false);
    }
    (holder, true)
}

/// Whether `[Authentication] Method` names OAuth 2.0. The module docs tell
/// whose rule this is.
///
/// `None` is a source with no `[Authentication]` group. It is not the same as
/// `Some("none")`, which a source that *has* the group gives when nobody set
/// a method (`ESourceAuthentication:method` has no unset state). Neither is
/// OAuth 2.0.
pub fn method_is_oauth2(method: Option<&str>) -> bool {
    let Some(method) = method else {
        return false;
    };
    if method == OAUTH2_METHOD {
        return true;
    }
    // An interior NUL cannot be in an authentication method. EDS stores the
    // field as a C string, so `e_source_authentication_set_method` cannot
    // write a value that contains one. To refuse is the safe direction: the
    // answer is "not OAuth2" and the account uses the password path. The
    // alternative is to truncate and possibly match a service whose name is
    // a prefix of the string.
    let Ok(method) = CString::new(method) else {
        return false;
    };
    // An `EOAuth2Services` that construction failed to make has no registered
    // services to match against. An empty one gives the same answer. This is
    // the same safe direction as the interior-NUL case above: the account
    // uses the password path, not a token that nothing could issue.
    let services = services();
    if services.is_null() {
        return false;
    }
    // SAFETY: a live `EOAuth2Services` that this module holds a reference to
    // for the process's lifetime, and a NUL-terminated string valid for the
    // call. The function applies EDS's own guard against "none",
    // "plain/password" and the empty string. It locks the instance around the
    // lookup and takes ownership of nothing of ours.
    unsafe { e_oauth2_services_is_oauth2_alias(services, method.as_ptr()) != GFALSE }
}

/// [`method_is_oauth2`] applied to a source. `connect_sync` uses it through
/// this function.
///
/// # Safety
///
/// `source` must be a valid `ESource` (the one EDS gave the backend). The
/// function only reads from it, and nothing outlives the call.
pub unsafe fn source_uses_oauth2(source: *mut ESource) -> bool {
    // As everywhere in this crate, the lookup finds an extension by name. It
    // walks the registered children of `E_TYPE_SOURCE_EXTENSION`, so it cannot
    // find a type that nothing has referenced yet.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe { e_source_authentication_get_type() };

    // Make sure that the extension exists before `e_source_get_extension`
    // runs: it *creates* an extension that it cannot find. This source is the
    // user's own account source. To create a group on it only to read a
    // default is a side effect on another object's state.
    // SAFETY: the source is valid by this function's contract, and the name
    // is a header constant.
    let Some(auth) = (unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }) else {
        return false;
    };

    // SAFETY: the extension exists and the source owns it. The extension
    // owns the string that the getter returns, and the string outlives this
    // call.
    let method = unsafe { read_string(e_source_authentication_get_method(auth)) };

    method_is_oauth2(method.as_deref())
}

/// Why an OAuth 2.0 access token could not be obtained silently through EDS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilentRefreshFailureReason {
    /// There is no stored OAuth 2.0 secret (refresh token) for this account
    /// yet (nobody consented for the account, or the secret was removed).
    NoStoredSecret,
    /// The login keyring that holds the stored secret is locked, and it
    /// cannot be unlocked without a display.
    KeyringLocked,
    /// A transport or spawn failure of the secret store (D-Bus or daemon).
    SecretStoreFailure,
    /// A D-Bus peer that the token fetch must reach does not exist (see
    /// [`is_service_gone`]). This is an infrastructure failure, never a
    /// credential failure.
    ServiceGone,
    /// The authorization server rejected the refresh token exchange (for
    /// example `invalid_grant`, an expired refresh token, or a rotation
    /// mismatch).
    ServerRejectedRefresh,
    /// The source is not supported or has no registered `EOAuth2Service`.
    UnregisteredService,
    /// The refresh operation was cancelled.
    Cancelled,
    /// The service returned an empty access token string.
    EmptyToken,
    /// An unrecognized or generic I/O error in the `G_IO_ERROR` domain.
    OtherIoError,
    /// An error in an unrecognized error domain.
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

/// Classifies a failed `e_source_get_oauth2_access_token_sync` call. It uses
/// the returned `GError` domain and code, and the keyring status.
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

    // Ask this before the secret-store question, because it is the narrower
    // of the two D-Bus-domain outcomes. A name that nobody owns is not a
    // store failure. `is_secret_store_failure` also excludes it, so this
    // order helps the reader but is not necessary for correct results.
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

/// Whether `error` is a failure *below* EDS's own OAuth 2.0 logic (the
/// secret store itself, or the D-Bus call that reaches it). The alternative
/// is an outcome that `evolution-data-server`'s own OAuth 2.0 code
/// intentionally sets. That code sets all its intentional outcomes in the
/// `G_IO_ERROR` domain, with different codes (`e-oauth2-service.c`,
/// `e-source.c`, `e-server-side-source.c`). Examples are: nobody has ever
/// consented, the server rejected a refresh attempt, or this source has no
/// registered `EOAuth2Service`. This function intentionally answers false for
/// all of them. "No registered service" is what every `ESource` that this
/// crate's own tests build gets (no real registry is behind them). Those
/// tests already pin `REQUIRED` for it.
///
/// This was confirmed by hand, not assumed. When `org.freedesktop.secrets`
/// cannot start (a broken D-Bus activation, a real, reproducible form of a
/// dead secret store), the call that this function classifies for gives
/// `g_dbus_error_quark()`/`G_DBUS_ERROR_SPAWN_EXEC_FAILED`. That is a
/// different *domain*. The domain is the only distinction here, so this
/// function does not guess at each code that EDS's own machinery can set.
///
/// **Two D-Bus codes are excluded**, and only two: [`is_service_gone`]'s.
/// They mean "nothing owns that bus name", which is a dead-peer failure and
/// not a store failure.
///
/// **This does not catch every "the store is locked" case.** Consider a
/// collection that exists but is locked, and whose unlock prompt cannot show
/// (no display, or the user dismisses it). libsecret's search then answers
/// "not found", because that API's documented contract says that a dismissed
/// or unshowable prompt is not an error. Thus here it looks the same as
/// "nobody has ever consented". In that case, no GError comes to this
/// function, so this function cannot do anything differently.
/// [`is_secret_not_found`] and [`crate::secret_store`] answer it instead.
///
/// # Safety
///
/// `error` must be a valid, non-NULL `GError`.
unsafe fn is_secret_store_failure(error: *const GError) -> bool {
    // SAFETY: the caller's contract; `g_dbus_error_quark` takes no arguments.
    unsafe { (*error).domain == g_dbus_error_quark() && !is_service_gone(error) }
}

/// Whether `error` is "the bus has no owner for the name that this call
/// went to". This is the captured dead-peer failure. It is the only D-Bus
/// outcome that is neither a credential nor a secret store failure.
///
/// **The mechanism, found from the source code, not from the trace.** A
/// backend process holds the `ESource` that EDS gave it for the account's
/// lifetime. That `ESource` holds a strong reference, in `priv->dbus_object`,
/// to the `GDBusObject` proxy that its `ESourceRegistry`'s
/// `GDBusObjectManagerClient` built for it. (`e-source.c` sets the field one
/// time, and clears it only in dispose or in
/// `__e_source_private_replace_dbus_object`.) GLib addresses every such
/// interface proxy to the manager's `name_owner`, which is a **unique** bus
/// name, and asserts this (`gio/gdbusobjectmanagerclient.c`, GLib 2.80.0:
/// `"g-name", name_owner` at the `g_initable_new` in `add_interfaces`,
/// `g_dbus_is_unique_name` at the start of that function, and the comment
/// "this is fine … and use a unique name").
/// When `evolution-source-registry` restarts, the manager removes the
/// proxies from *its own* map (`on_notify_g_name_owner`). But the proxies
/// that an `ESource` holds stay alive, still addressed to the unique name of
/// the dead process. EDS points them to the new process only later and
/// asynchronously, from the registry's own `GMainContext`
/// (`source_registry_name_appeared` →
/// `source_registry_object_added_no_owner` →
/// `__e_source_private_replace_dbus_object`, `e-source-registry.c`). A
/// synchronous token fetch in that window goes to a dead peer. The bus
/// answers `G_DBUS_ERROR_SERVICE_UNKNOWN` with a message that names the peer:
/// "The name :1.4 was not provided by any .service files".
///
/// **EDS 3.52 does not recover from it. That is an upstream problem, not
/// ours.** `source_get_oauth2_access_token_sync` (`e-source.c`) has an
/// in-process fallback: it finds the account's `EOAuth2Service` and calls it
/// directly. But it uses the fallback only when the D-Bus *interface object*
/// is absent, never when a call on a present one fails. Thus this crate can
/// only classify it: report it, and never ask for consent because of it.
///
/// **The interface is present for our accounts, so this failure can occur.**
/// EDS's own registry module `module-oauth2-services.c` calls
/// `e_server_side_source_set_oauth2_support` for every server-side source
/// whose `[Authentication] Method` names a registered `EOAuth2Service` (ours
/// is `jmap_config::oauth2_service::NAME`). That setter exports the
/// `Source.OAuth2Support` D-Bus interface at the source's object path. The
/// tests below build the resulting `GError` by hand.
/// `jmap-functional/tests/oauth2-stale-proxy.rs` proves that real daemons
/// produce exactly this error: it holds an `ESource` across a registry
/// `SIGKILL`. If that test stops reporting `oauth2-support-exported=1`, this
/// classification is unreachable, but the tests below still pass.
///
/// **Both codes, because a bus can give either.** A method call on an
/// unowned name that cannot be activated gets `SERVICE_UNKNOWN`. Other bus
/// operations give the sibling, `NAME_HAS_NO_OWNER`. Neither is a password
/// problem, and a new sign-in fixes neither.
///
/// **Intentionally not limited to unique names.** The peer in the captured
/// trace is a `:1.N`. When `org.freedesktop.secrets` fails to activate, the
/// same two codes occur with a well-known name in the message. To tell them
/// apart, the code must parse `dbus-daemon`'s English error text. That was
/// ruled out for this classification ("domain **and** code, not message
/// text"). It is also not necessary: both mean "a service that this sign-in
/// needs is not running", and both are `ERROR` and never consent. The
/// resulting message names the peer and does not claim which case it is.
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

/// Whether `error` is EDS's "this account has no stored secret". A *locked*
/// keyring gives the same outcome. Thus this is the only outcome for which
/// the code asks the secret store itself.
///
/// `eos_lookup_token_sync` (`e-oauth2-service.c`) sets exactly this for both
/// "nobody has ever consented" and "the lookup came back empty". A
/// collection whose unlock prompt cannot show gives the second.
/// [`is_secret_store_failure`] above and [`crate::secret_store`] tell why the
/// two are identical here.
///
/// Domain **and** code, not message text, by design. The narrow match is
/// necessary, not only tidy. EDS's other intentional `G_IO_ERROR`-domain
/// outcomes must continue to go to [`ConnectError::OAuth2`]/`REQUIRED`
/// without change. A bare `ESource` with no registry behind it gives
/// `G_IO_ERROR_NOT_SUPPORTED`, and every `ESource` that this crate's own
/// tests build is such a source. A wider match would make those tests depend
/// on whether the test machine has a locked keyring.
///
/// # Safety
///
/// `error` must be a valid, non-NULL `GError`.
unsafe fn is_secret_not_found(error: *const GError) -> bool {
    // SAFETY: the caller's contract; `g_io_error_quark` takes no arguments.
    unsafe { (*error).domain == g_io_error_quark() && (*error).code == G_IO_ERROR_NOT_FOUND }
}

/// The report for a dead peer: [`ConnectError::ServiceGone`] with the bus's
/// own message, which is where the peer gets *named*.
///
/// This is separate from [`access_token`] only so that a test can use it.
/// The result must be an `ERROR` that names the dead peer. If the message
/// silently stopped including EDS's own text, it would still compile and
/// still be `ERROR`, but nobody could tell which service died.
fn service_gone_error(message: Option<String>) -> ConnectError {
    ConnectError::ServiceGone(translate_with(
        // TRANSLATORS: %1$s is the message bus's own text naming which service
        // has no owner, e.g. "The name :1.4 was not provided by any .service
        // files".
        c"a service this account's sign-in needs is not running, so no access token could be fetched: %1$s",
        &[&message.unwrap_or_else(|| translate(c"no further detail was given"))],
    ))
}

/// The OAuth 2.0 access token for this account's requests, from the
/// `EOAuth2Service` that claims `source`.
///
/// The refresh occurs here. Inside this call, EDS looks up the account's
/// refresh token in libsecret and exchanges it for an access token. Thus the
/// returned token is valid now.
///
/// A failure is [`ConnectError::OAuth2`] when it is "nobody has consented to
/// this account yet" or "the refresh was rejected". That variant tells why
/// it asks Evolution to authenticate and does not discard what it has. A
/// failure below that decision is [`ConnectError::SecretStore`]: a store
/// failure per `is_secret_store_failure` below, a locked keyring, or no
/// secret service. A dead D-Bus peer per `is_service_gone` is
/// [`ConnectError::ServiceGone`].
///
/// # Safety
///
/// `source` must be a valid `ESource`, and `cancellable` must be NULL or a
/// valid `GCancellable`. An EDS vfunc receives these. Nothing here outlives
/// the call.
pub unsafe fn access_token(
    source: *mut ESource,
    cancellable: *mut GCancellable,
) -> Result<String, ConnectError> {
    let account_id = unsafe { read_string(eds_sys::e_source_get_uid(source)) };
    // SAFETY: a valid source by this function's contract.
    let (holder, owned) = unsafe { credentials_source(source) };
    if owned {
        // SAFETY: a live source from the call above.
        let holder_id = unsafe { read_string(eds_sys::e_source_get_uid(holder)) };
        tracing::trace!(
            ?account_id,
            ?holder_id,
            "the account's collection holds the credentials, so the token is fetched on it"
        );
    }
    tracing::debug!(?account_id, "fetching OAuth 2.0 access token via EDS");
    let mut token = ptr::null_mut();
    let mut expires_in = 0;
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: a valid source and cancellable by this function's contract, and
    // three writable out-parameters. The token comes back as a GLib
    // allocation that this call owns. The same applies to the GError.
    let ok = unsafe {
        e_source_get_oauth2_access_token_sync(
            holder,
            cancellable,
            &mut token,
            &mut expires_in,
            &mut error,
        )
    };

    if owned {
        // SAFETY: a reference that `credentials_source` transferred to this
        // call, and that nothing above kept.
        unsafe { gobject_sys::g_object_unref(holder.cast()) };
    }

    if ok == GFALSE || token.is_null() {
        // SAFETY: `error` is NULL or a GError that this call owns. To read
        // its message and classify its domain/code borrows fields that the
        // struct owns. The out-parameter contract requires the free after.
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

        // Ask this before the store question. `classify_failure` uses the
        // same order for the same reason: both are D-Bus-domain failures,
        // and this one is not the store's.
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

        // EDS gives "no stored secret" for two different situations: nobody
        // has consented yet, or the keyring that holds the consent is
        // locked. Thus only this answer needs a second question. The code
        // asks it here, *after* the fetch failed, only to classify that
        // failure. This makes the inherent race harmless. If the store is
        // unlocked between the fetch and this call, the answer is "not
        // locked", and the account gets the consent window it would get
        // anyway. If the store is locked in that window, it really is
        // locked. The code does not skip or retry anything because of the
        // answer, so the answer can change only the message.
        //
        // Only `Some(true)`: a secret service that is not available, or
        // that does not exist, answers `None`. A machine with no keyring
        // must continue to behave exactly as before (see
        // `crate::secret_store`).
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

        // The other case where a store is not useful: it does not exist. A
        // sign-in window here would complete, but there is no location for
        // the token. Thus the next fetch would ask again. The user does all
        // the sign-in steps and nothing keeps the result. The correct
        // outcome is an error.
        //
        // Only `Some(false)`, as in the locked test above and for the same
        // reason. A desktop whose keyring has not started yet answers
        // `Some(true)`, because the bus can activate it. A machine that
        // cannot be asked answers `None`. Both must continue to behave
        // exactly as before this test existed. Consent is useless only when
        // the bus knows of no such service, running or startable.
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

        // A failure with no GError should not occur, because EDS sets one on
        // every path. But a NULL message must not cause a panic in a
        // backend. Thus it becomes a sentence that tells exactly what is
        // known.
        return Err(ConnectError::OAuth2(message.unwrap_or_else(|| {
            translate(
                // TRANSLATORS: shown when EDS refused an OAuth 2.0 account's
                // access token but did not say why.
                c"the account has no OAuth 2.0 access token",
            )
        })));
    }

    // SAFETY: non-NULL on success, a NUL-terminated GLib allocation that this
    // scope owns. The code copies it out and then frees it, as
    // `(transfer full)` requires.
    let value = unsafe {
        let value = read_string(token);
        g_free(token.cast());
        value
    };

    // `read_string` answers `None` only for NULL, which `token.is_null()`
    // already excluded. The server would reject an empty token. An error here
    // is better than a request with `Authorization: Bearer `.
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

    /// Builds a real `GError`, not a fake struct. [`is_secret_store_failure`]
    /// reads its `domain` field directly, so the test must use the same GLib
    /// allocation as the production code, not a hand-made substitute.
    fn error(domain: glib_sys::GQuark, code: i32) -> *mut GError {
        let message = CString::new("boom").unwrap();
        // SAFETY: a valid domain and a NUL-terminated message. Every caller
        // below frees the result.
        unsafe { g_error_new_literal(domain, code, message.as_ptr()) }
    }

    /// EDS's own OAuth 2.0 code intentionally sets every outcome in the
    /// `G_IO_ERROR` domain, with different codes. This includes "no
    /// registered service", not only "no grant yet". None of them is a
    /// secret-store failure. The `jmap-backend-core/tests/oauth2.rs`
    /// integration test examines the "no registered service" form end to end
    /// and expects `REQUIRED`. This test pins the unit below it.
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

    /// The real, reproduced form of a dead secret store. A broken
    /// `org.freedesktop.secrets` D-Bus activation gives
    /// `g_dbus_error_quark()`/`G_DBUS_ERROR_SPAWN_EXEC_FAILED`. This was
    /// confirmed by hand against a real `gnome-keyring-daemon` whose
    /// `.service` file pointed at a missing binary. The domain is different
    /// from `G_IO_ERROR`, and this must not open a new consent window.
    #[test]
    fn a_broken_secret_service_is_a_secret_store_failure() {
        let error = error(
            unsafe { g_dbus_error_quark() },
            G_DBUS_ERROR_SPAWN_EXEC_FAILED,
        );
        assert!(unsafe { is_secret_store_failure(error) });
        unsafe { g_error_free(error) };
    }

    /// The form that `eos_lookup_token_sync` gives both for "nobody has ever
    /// consented" and for an empty lookup from a locked collection. This is
    /// the only outcome for which [`access_token`] asks the secret store.
    #[test]
    fn a_missing_secret_is_the_one_ambiguous_outcome() {
        let error = error(unsafe { g_io_error_quark() }, G_IO_ERROR_NOT_FOUND);
        assert!(unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }

    /// An `ESource` with no registry behind it gives
    /// `G_IO_ERROR_NOT_SUPPORTED`. Every `ESource` that this crate's own
    /// tests build is such a source. `tests/oauth2.rs` and
    /// `jmap-backend-collection`'s `tests/authenticate.rs` both pin it to
    /// `REQUIRED`. It must not get to the secret store question. If it did,
    /// those tests would depend on whether the test machine has a locked
    /// keyring.
    #[test]
    fn no_registered_service_is_not_a_missing_secret() {
        let error = error(unsafe { g_io_error_quark() }, G_IO_ERROR_NOT_SUPPORTED);
        assert!(!unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }

    /// Domain and code, not code alone. `G_IO_ERROR_NOT_FOUND` is the
    /// integer 1, which is also an ordinary code in every other error domain.
    /// This includes the D-Bus domain, which this module classifies
    /// separately.
    #[test]
    fn a_missing_secret_is_recognised_by_domain_not_by_code_alone() {
        let error = error(unsafe { g_dbus_error_quark() }, G_IO_ERROR_NOT_FOUND);
        assert!(!unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }

    /// D-Bus timeouts and spawn failures below libsecret are in the
    /// `g_dbus_error_quark()` domain, and are classified as secret-store
    /// failures. `SERVICE_UNKNOWN`/`NAME_HAS_NO_OWNER` are intentionally *not*
    /// in this group now. See
    /// [`the_dead_peer_codes_are_not_blamed_on_the_keyring`].
    #[test]
    fn dbus_timeout_and_transport_errors_are_secret_store_failures() {
        // Different D-Bus codes (NO_REPLY=4, SPAWN_FORK_FAILED=24)
        for code in [4, 24, G_DBUS_ERROR_SPAWN_EXEC_FAILED] {
            let error = error(unsafe { g_dbus_error_quark() }, code);
            assert!(
                unsafe { is_secret_store_failure(error) },
                "expected D-Bus error code {code} to be classified as secret-store failure"
            );
            unsafe { g_error_free(error) };
        }
    }

    /// The captured form of a dead registry peer: a token fetch that fails
    /// with `G_DBUS_ERROR_SERVICE_UNKNOWN` ("The name :1.4 was not provided
    /// by any .service files"). The cause is that the `ESource` still holds a
    /// `GDBusObjectManagerClient` proxy addressed to a registry that has
    /// restarted since.
    ///
    /// It must not be reported as a keyring failure. The keyring is
    /// serviceable, and an instruction to unlock it points the user at the
    /// wrong thing. The test covers both codes that a bus gives for "nobody
    /// owns that name".
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

    /// A dead peer is `ServiceGone`. As with every other infrastructure
    /// failure, it never escalates to a consent window.
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

    /// The captured failure end to end, built as production builds it, not
    /// with a hardcoded code.
    ///
    /// `g_dbus_error_new_for_dbus_error` is the exact conversion that
    /// `gdbusconnection.c` applies to a bus reply.
    /// `org.freedesktop.DBus.Error.ServiceUnknown` with this message is what
    /// `dbus-daemon` really gives for a method call to an unowned unique
    /// name. This was confirmed by hand on this machine, not assumed:
    ///
    /// ```text
    /// $ dbus-run-session -- dbus-send --session --print-reply \
    ///       --dest=:1.9999 /org/freedesktop/DBus org.freedesktop.DBus.Peer.Ping
    /// Error org.freedesktop.DBus.Error.ServiceUnknown: The name :1.9999 was
    /// not provided by any .service files
    /// ```
    ///
    /// `g_dbus_error_strip_remote_error` is then EDS's own next line
    /// (`source_get_oauth2_access_token_sync`, `e-source.c`). Thus the
    /// message in the trace has no `GDBus.Error:` prefix. This is the real
    /// `GError` that [`access_token`] classifies. The only difference is that
    /// no registry died.
    #[test]
    fn the_wire_error_a_dead_peer_produces_is_service_gone_and_still_names_it() {
        let name = CString::new("org.freedesktop.DBus.Error.ServiceUnknown").unwrap();
        let text = CString::new("The name :1.4 was not provided by any .service files").unwrap();
        // SAFETY: two NUL-terminated strings. This code owns the returned
        // `GError` and frees it below.
        let error = unsafe {
            let error = gio_sys::g_dbus_error_new_for_dbus_error(name.as_ptr(), text.as_ptr());
            gio_sys::g_dbus_error_strip_remote_error(error);
            error
        };

        assert!(unsafe { is_service_gone(error) });
        let (reason, _quark, _domain_name, _code, message) = unsafe { classify_failure(error) };
        assert_eq!(reason, SilentRefreshFailureReason::ServiceGone);
        assert!(!reason.escalates_to_consent());

        // The peer must be in the text that the user sees. That is all of
        // "naming the dead peer".
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

    /// Domain and code, the same narrow match that
    /// [`a_missing_secret_is_recognised_by_domain_not_by_code_alone`] pins.
    /// `G_DBUS_ERROR_SERVICE_UNKNOWN` is the integer 2, which is also an
    /// ordinary code in `G_IO_ERROR` (`G_IO_ERROR_NOT_FOUND` is 1, `EXISTS` 2).
    #[test]
    fn a_dead_peer_is_recognised_by_domain_not_by_code_alone() {
        let error = error(unsafe { g_io_error_quark() }, G_DBUS_ERROR_SERVICE_UNKNOWN);
        assert!(!unsafe { is_service_gone(error) });
        unsafe { g_error_free(error) };
    }

    /// Classifies each category of silent-refresh failure: its reason, domain
    /// name and code, and whether it escalates to an interactive consent
    /// prompt.
    #[test]
    fn classify_failure_covers_all_silent_refresh_failure_reasons() {
        use gio_sys::{G_IO_ERROR_CANCELLED, G_IO_ERROR_CONNECTION_REFUSED, G_IO_ERROR_FAILED};

        // D-Bus or secret store failure -> SecretStoreFailure, no consent
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

        // Missing secret -> NoStoredSecret (escalates to consent), or
        // KeyringLocked if the keyring of the test machine is locked
        let not_found = error(unsafe { g_io_error_quark() }, G_IO_ERROR_NOT_FOUND);
        let (reason, _quark, _domain_name, code, _msg) = unsafe { classify_failure(not_found) };
        assert!(
            reason == SilentRefreshFailureReason::NoStoredSecret
                || reason == SilentRefreshFailureReason::KeyringLocked
        );
        assert_eq!(code, G_IO_ERROR_NOT_FOUND);
        unsafe { g_error_free(not_found) };

        // Server refresh rejection (HTTP 400 Bad Request, invalid_grant,
        // rotation mismatch) -> ServerRejectedRefresh
        let refused = error(unsafe { g_io_error_quark() }, G_IO_ERROR_CONNECTION_REFUSED);
        let (reason, _quark, domain_name, code, _msg) = unsafe { classify_failure(refused) };
        assert_eq!(reason, SilentRefreshFailureReason::ServerRejectedRefresh);
        assert_eq!(reason.as_str(), "server_rejected_refresh");
        assert!(reason.escalates_to_consent());
        assert_eq!(code, G_IO_ERROR_CONNECTION_REFUSED);
        assert_eq!(domain_name.as_deref(), Some("g-io-error-quark"));
        unsafe { g_error_free(refused) };

        // Unregistered service or not supported -> UnregisteredService
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

        // Other generic I/O error -> OtherIoError
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

    /// `services()` must return the real singleton, never a null substitute.
    /// Every caller uses `is_null()` to mean "could not be built".
    /// `e_oauth2_services_new()` needs no registry or bus, so this must not
    /// fail in any environment.
    #[test]
    fn services_singleton_is_never_null() {
        assert!(!services().is_null());
    }

    /// `Display` must actually write `as_str()`'s text, not only succeed.
    /// [`service_gone_error`]'s own test examines only `ConnectError`'s
    /// `Display`, never this type's.
    #[test]
    fn display_writes_the_same_text_as_as_str() {
        for reason in [
            SilentRefreshFailureReason::NoStoredSecret,
            SilentRefreshFailureReason::KeyringLocked,
            SilentRefreshFailureReason::ServiceGone,
        ] {
            assert_eq!(reason.to_string(), reason.as_str());
        }
    }

    /// A source with no parent is its own credentials source, and the answer
    /// comes without a provider.
    ///
    /// The collection backend needs this guard; it is not a detail. That
    /// backend runs inside `evolution-source-registry` and calls
    /// [`access_token`] on its own collection source, whose `Parent` is
    /// empty. If the parent test were not first, the process would build an
    /// `ESourceRegistry`, a D-Bus client of itself. The answer to that
    /// question is the source that the caller already has.
    ///
    /// The test also asserts the not-owned half of the answer. A caller that
    /// unrefs a reference it did not take drops the backend's own source.
    #[test]
    fn a_parentless_source_is_its_own_credentials_source() {
        let uid = CString::new("no-parent").unwrap();
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, no D-Bus object, a GError out-parameter.
        let source =
            unsafe { eds_sys::e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");

        // SAFETY: the live source built above.
        let (holder, owned) = unsafe { credentials_source(source) };
        assert_eq!(holder, source, "a parentless source resolves to itself");
        assert!(
            !owned,
            "nothing was referenced, so nothing is to be released"
        );
        assert!(
            CREDENTIALS.get().is_none(),
            "answering this must not have built a credentials provider"
        );

        // SAFETY: this owns the only reference that the constructor returned.
        unsafe { gobject_sys::g_object_unref(source.cast()) };
    }
}
