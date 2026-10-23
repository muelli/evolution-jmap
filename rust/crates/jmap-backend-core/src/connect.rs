// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opens the connection that `connect_sync` needs, without GObject.
//!
//! `connect_sync` is the one vfunc that must answer two questions together.
//! Did this work? If not, must Evolution ask the user for a password again?
//! The second answer is `out_auth_result`. A wrong answer is the difference
//! between a prompt that fixes the account and an account that is
//! permanently broken with no way to report it.
//! [`ConnectError::auth_result`] is that decision, next to the failures it
//! classifies.
//!
//! It is here and not in a backend because the decision must be the same
//! for all backends. An address book and a calendar get to this code by
//! different routes and resolve different collections. But "a 401 is the
//! only failure that needs a re-prompt" is a property of the EDS credentials
//! machinery, not of contacts. Such a rule goes wrong when it is written
//! twice and then corrected once. The backends keep only the
//! [`Collection`]-shaped parts: the capability they ask the session for, and
//! the list in which they look up their identifier.

use std::fmt;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_REQUIRED, E_CLIENT_ERROR_DBUS_ERROR, E_CLIENT_ERROR_INVALID_ARG,
    E_SOURCE_AUTHENTICATION_ACCEPTED, E_SOURCE_AUTHENTICATION_ERROR,
    E_SOURCE_AUTHENTICATION_REJECTED, E_SOURCE_AUTHENTICATION_REQUIRED,
    E_SOURCE_AUTHENTICATION_UNKNOWN, EClientError, ENamedParameters, ESource,
    ESourceAuthenticationResult, e_client_error_create,
};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_client::{Credentials, Error};
use jmap_proto::Id;

use crate::api_token::source_uses_api_token;
use crate::cancel::observe;
use crate::error::{cstring_lossy, invalid_arg_gerror, set_raw_gerror};
use crate::i18n::{translate, translate_with};
use crate::marshal::password as stored_password;
use crate::oauth2::{access_token, source_uses_oauth2};
use crate::source::SourceConfig;

/// What `connect_sync` writes into `out_auth_result` when it succeeds.
pub const ACCEPTED_AUTH_RESULT: ESourceAuthenticationResult = E_SOURCE_AUTHENTICATION_ACCEPTED;

/// The kind of server-side collection one EDS source stands for.
///
/// Here, the two backends are otherwise identical. The distinction stays only
/// because the user sees it. A user can act on "the account names calendar
/// \"Cal-1\", which the server does not have", but not on "the account names
/// collection \"Cal-1\"".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collection {
    AddressBook,
    Calendar,
}

impl Collection {
    /// The name of this collection in a message to the user, in the user's
    /// language.
    pub fn noun(self) -> String {
        match self {
            Self::AddressBook => translate(
                // TRANSLATORS: names one of an account's two kinds of
                // server-side collection, inside a sentence describing an
                // account error.
                c"address book",
            ),
            Self::Calendar => translate(
                // TRANSLATORS: names one of an account's two kinds of
                // server-side collection, inside a sentence describing an
                // account error.
                c"calendar",
            ),
        }
    }
}

impl fmt::Display for Collection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.noun())
    }
}

/// A connection that could not be opened.
#[derive(Debug)]
pub enum ConnectError {
    /// The account names a user but EDS has no password for it yet.
    CredentialsRequired,
    /// The account authenticates with OAuth 2.0 and no access token is
    /// available: nobody has consented yet, or the refresh failed. The string
    /// is the EDS message for the failure.
    ///
    /// Intentionally classified as [`E_SOURCE_AUTHENTICATION_REQUIRED`], not
    /// `REJECTED`. The distinction is more important here than on the
    /// password path. `REQUIRED` opens the consent window. `REJECTED` also
    /// tells EDS to discard the stored secret. For OAuth 2.0 that secret is
    /// the *refresh* token. A short network failure during the exchange does
    /// not make it invalid. If EDS discards it, a transient failure becomes
    /// a re-consent that the user must click through.
    OAuth2(String),
    /// An OAuth 2.0 token fetch failed *below* the authorization decision (in
    /// the secret store or in the network call of a refresh). A new consent
    /// cannot fix this. See `crate::oauth2::is_secret_store_failure` for how
    /// the two cases are told apart. Classified as `ERROR`, not `REQUIRED`:
    /// this variant exists to stop the loop of new consent windows on a dead
    /// keyring.
    SecretStore(String),
    /// An OAuth 2.0 token fetch went to a D-Bus peer that no longer exists.
    /// The account's `ESource` still holds the interface proxy of an
    /// `evolution-source-registry` that has restarted. The bus answers
    /// `SERVICE_UNKNOWN` and names the dead unique name. See
    /// [`crate::oauth2::is_service_gone`] for the mechanism and for
    /// why EDS 3.52 does not recover from it by itself.
    ///
    /// Separate from [`Self::SecretStore`], although both are `ERROR` and
    /// neither ever asks for consent. In this case the keyring is correct,
    /// and a message that tells its owner to unlock it points to the wrong
    /// fix. Classified as `ERROR` for the same reason as `SecretStore`: a new
    /// sign-in does not fix a dead peer. It only causes more and more
    /// requests.
    ServiceGone(String),
    /// The server refused, failed, or is unreachable.
    Client(Error),
    /// The account names a collection the server does not have.
    NoSuchCollection(Collection, String),
    /// The account names no collection and the server flags none default.
    NoDefaultCollection(Collection),
}

impl From<Error> for ConnectError {
    fn from(error: Error) -> Self {
        Self::Client(error)
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CredentialsRequired => f.write_str(&translate(
                // TRANSLATORS: shown when an account configured with a user
                // name has no password stored for it yet.
                c"the account has no password yet",
            )),
            Self::OAuth2(message) => f.write_str(message),
            Self::SecretStore(message) => f.write_str(message),
            Self::ServiceGone(message) => f.write_str(message),
            Self::Client(error) => error.fmt(f),
            Self::NoSuchCollection(kind, id) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is "address book" or "calendar"; %2$s is
                // the identifier this account is configured to use for it,
                // which the server does not have.
                c"the account names %1$s \"%2$s\", which the server does not have",
                &[kind.noun().as_str(), id.as_str()],
            )),
            Self::NoDefaultCollection(kind) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is "address book" or "calendar".
                c"the server offers no default %1$s for this account",
                &[kind.noun().as_str()],
            )),
        }
    }
}

impl std::error::Error for ConnectError {}

impl ConnectError {
    /// What `connect_sync` writes into `out_auth_result`.
    ///
    /// Only `REJECTED` makes Evolution discard the stored password and ask
    /// again. Thus it is only for the one case where a new request can help:
    /// the server said that the credentials were wrong. A 403 is *not* that
    /// case: the server accepted the credentials, but the account does not
    /// have permission. A server that is down is also not that case.
    ///
    /// `UNKNOWN` is only for
    /// [`Error::is_redirect_credentials_withheld`][jmap_client::Error::is_redirect_credentials_withheld].
    /// All other values, `ERROR` included, make the EDS function
    /// `ensure_connected_sync` schedule interactive consent. A redirect that
    /// withheld credentials is not a rejected password that a re-prompt can
    /// fix. It is a connection failure that gets the same redirect again.
    pub fn auth_result(&self) -> ESourceAuthenticationResult {
        match self {
            Self::CredentialsRequired | Self::OAuth2(_) => E_SOURCE_AUTHENTICATION_REQUIRED,
            Self::Client(error) if error.is_redirect_credentials_withheld() => {
                E_SOURCE_AUTHENTICATION_UNKNOWN
            }
            Self::Client(error) if is_wrong_password(error) => E_SOURCE_AUTHENTICATION_REJECTED,
            // All other cases, `SecretStore` and `ServiceGone` included.
            // Intentionally not `REQUIRED` for those two: a new consent does
            // not fix a dead secret store or a dead D-Bus peer. It only
            // causes the loop of requests that these two variants stop.
            _ => E_SOURCE_AUTHENTICATION_ERROR,
        }
    }

    /// Changes a server's 401 on an OAuth 2.0 bearer token into the same
    /// [`Self::OAuth2`] failure that a failed *fetch* of that token gets.
    ///
    /// Call this only when the credentials just tried were an OAuth 2.0
    /// access token. All callers know this from the same
    /// [`crate::oauth2::source_uses_oauth2`] read that chose the credential.
    /// This function does not change a 401 on a Basic or API-token attempt:
    /// there, [`is_wrong_password`] correctly means the password was wrong,
    /// and `REJECTED` (ask again, discard the stored secret) is the correct
    /// answer.
    ///
    /// For OAuth 2.0 it is not correct, for the reason that the doc of
    /// [`Self::OAuth2`] gives for a failed fetch. `REQUIRED` opens a new
    /// consent window and keeps the stored refresh token. A 401 immediately
    /// after a successful fetch does not make that token invalid. Here, a
    /// transient rejection and a really revoked grant look the same.
    /// `REQUIRED` does not overreact to the more frequent transient case, but
    /// `REJECTED` does.
    ///
    /// Mirrors `jmap_mail::connect::StoreError::reclassify_oauth2_rejection`.
    /// The reason is the same, applied to the four-valued EDS
    /// `ESourceAuthenticationResult` instead of the three-valued Camel one.
    pub fn reclassify_oauth2_rejection(self) -> Self {
        match self {
            Self::Client(error) if is_wrong_password(&error) => Self::OAuth2(error.to_string()),
            other => other,
        }
    }

    /// Allocates a `GError` that describes this failure. The caller gets
    /// ownership, as with [`crate::error::to_gerror`].
    pub fn to_gerror(&self) -> *mut GError {
        if let Self::Client(error) = self {
            return crate::error::to_gerror(error);
        }
        let message = cstring_lossy(&self.to_string());
        // SAFETY: the code is one of the values of the enum, and the call
        // copies the message.
        unsafe { e_client_error_create(self.client_error_code(), message.as_ptr()) }
    }

    fn client_error_code(&self) -> EClientError {
        match self {
            Self::CredentialsRequired | Self::OAuth2(_) => E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
            // Not an authentication problem, and editing the account does not
            // fix it. The secret store, or the D-Bus peer of the token fetch,
            // is unreachable.
            Self::SecretStore(_) | Self::ServiceGone(_) => E_CLIENT_ERROR_DBUS_ERROR,
            // Editing the account (or creating the collection on the server)
            // fixes both collection failures. A retry never fixes them. Thus
            // they are reported the same as a malformed host.
            _ => E_CLIENT_ERROR_INVALID_ARG,
        }
    }
}

/// Whether this failure means that the credentials were wrong. If so, a new
/// request to the user for them can help.
///
/// The EDS backends and the Camel provider must give the same answer to this
/// rule. Thus it is a function, not a match arm in each of them. The two
/// report it through unrelated enums (`ESourceAuthenticationResult` and
/// `CamelAuthenticationResult`), but the question is the same. A wrong
/// answer in either direction is a product failure. A re-prompt on a 403 is
/// a loop that no password stops. No re-prompt on a 401 gives an account
/// that is broken with no way to report it.
///
/// A 403 is intentionally excluded: the server *did* accept the credentials,
/// but the account does not have permission. A different password does not
/// fix this. An unreachable server is also excluded.
pub fn is_wrong_password(error: &Error) -> bool {
    error.is_unauthorized()
}

/// The credentials to authenticate with, from the source's user name and the
/// value that EDS got from libsecret.
///
/// `password` is `None` on the first attempt. Then a source that names a
/// user fails with [`ConnectError::CredentialsRequired`] and does not try
/// the server anonymously. Thus the prompt occurs before anything is sent. A
/// source that names no user is intentionally anonymous: this is `jmap-mockd`
/// and a development Stalwart. A real server answers it with a 401, which
/// also becomes a prompt.
pub fn credentials(
    user: Option<&str>,
    password: Option<&str>,
) -> Result<Credentials, ConnectError> {
    match (user, password) {
        (Some(user), Some(password)) => Ok(Credentials::basic(user, password)),
        (Some(_), None) => Err(ConnectError::CredentialsRequired),
        (None, _) => Ok(Credentials::none()),
    }
}

/// The credentials for the method of
/// [`crate::api_token::source_uses_api_token`]. This is the same
/// [`E_SOURCE_CREDENTIAL_PASSWORD`](crate::marshal::password) slot that Basic
/// reads, sent as a Bearer token and not with a user name. This method has
/// no user name, because a token identifies the account by itself. Thus the
/// only question is whether a prompt for the token has occurred. An empty
/// stored token is sent unchanged, and the server's 401 makes it a
/// re-prompt. An empty stored password on the Basic path works the same.
pub fn bearer_credentials(password: Option<&str>) -> Result<Credentials, ConnectError> {
    match password {
        Some(password) => Ok(Credentials::bearer(password)),
        None => Err(ConnectError::CredentialsRequired),
    }
}

/// How an account authenticates, from `[Authentication] Method`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScheme {
    /// A token from EDS's `EOAuth2Service`, sent as Bearer.
    OAuth2,
    /// The stored secret, sent as Bearer without a user name.
    ApiToken,
    /// The user name and the stored secret, sent as Basic.
    Password,
}

impl AuthScheme {
    /// The scheme of `source`. Every connection to the account uses this one rule.
    ///
    /// # Safety
    ///
    /// `source` must be a valid `ESource`. This function only reads it.
    pub unsafe fn of(source: *mut ESource) -> Self {
        // SAFETY: a valid source, by this function's contract.
        if unsafe { source_uses_oauth2(source) } {
            Self::OAuth2
        } else if unsafe { source_uses_api_token(source) } {
            Self::ApiToken
        } else {
            Self::Password
        }
    }
}

/// The credentials of `scheme` for `source`.
///
/// `password` gives the stored secret. Only the API-token and password schemes
/// call it, so an OAuth 2.0 account never reads libsecret for a password.
///
/// # Safety
///
/// `source` must be a valid `ESource`, and `cancellable` must be NULL or a
/// valid `GCancellable`, as [`access_token`] requires.
pub unsafe fn resolve_credentials(
    source: *mut ESource,
    scheme: AuthScheme,
    user: Option<&str>,
    password: impl FnOnce() -> Option<String>,
    cancellable: *mut GCancellable,
) -> Result<Credentials, ConnectError> {
    match scheme {
        // SAFETY: the contract of this function is the contract of `access_token`.
        AuthScheme::OAuth2 => unsafe { access_token(source, cancellable) }.map(Credentials::bearer),
        AuthScheme::ApiToken => bearer_credentials(password().as_deref()),
        AuthScheme::Password => credentials(user, password().as_deref()),
    }
}

/// Which of the server's collections this source stands for.
///
/// `candidates` is `(id, is_default)` for each collection of the account, in
/// the server's order. This function intentionally ignores the order. A
/// source that names nothing gets the collection that the *server* flags as
/// default, never "the first one". Do not guess the collection of a source,
/// because the user's data goes to the guessed collection.
///
/// This function checks a named collection against the list. Otherwise, a
/// typo in a hand-written `.source` shows as an empty collection. That looks
/// the same as a server that lost the contents.
pub fn resolve<'a, I>(
    kind: Collection,
    wanted: Option<&str>,
    candidates: I,
) -> Result<Id, ConnectError>
where
    I: IntoIterator<Item = (Option<&'a Id>, Option<bool>)>,
{
    let candidates = candidates.into_iter().filter_map(|(id, is_default)| {
        // A collection without a server id cannot be synced, even if it
        // is flagged as the default.
        id.map(|id| (id, is_default))
    });

    match wanted {
        Some(wanted) => candidates
            .map(|(id, _)| id)
            .find(|id| id.as_ref() == wanted)
            .cloned()
            .ok_or_else(|| ConnectError::NoSuchCollection(kind, wanted.to_owned())),
        None => candidates
            .filter(|(_, is_default)| *is_default == Some(true))
            .map(|(id, _)| id.clone())
            .next()
            .ok_or(ConnectError::NoDefaultCollection(kind)),
    }
}

/// All of `connect_sync` except the instance and the collection lookup. It
/// goes from the `ESource` that EDS gives the backend to the result of
/// `open`. It writes `out_auth_result` and `error` as the vfunc must.
///
/// A subclass calls this layer. It is here, not next to the other vfunc
/// bodies, because it is the only one whose input is an `ESource`. A test can
/// make an `ESource` with `e_source_new_with_uid`. A meta backend needs a
/// registry.
///
/// This function writes `out_auth_result` on **every** path, success
/// included. EDS reads it each time the vfunc returns. A stale value from a
/// previous attempt causes an account to never prompt or to prompt forever.
///
/// `cancellable` is [`observe`]d only for the duration of the connect. The
/// connection that `open` returns has no cancellation of its own. This is
/// intentional. A flag in the client belongs to the *account*, not to the
/// operation. Thus a connect that the user stopped leaves a client with a
/// latched cancellation that nothing can clear. Then all later operations on
/// that account refuse until a reconnect. Each later operation is stopped by
/// the scope that its own vfunc installs.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource`, `credentials` NULL or a valid
/// `ENamedParameters`, `cancellable` NULL or a valid `GCancellable`, and the
/// two out-parameters NULL or writable. An EDS vfunc receives these.
pub unsafe fn connect_with<T, F>(
    kind: Collection,
    source: *mut ESource,
    credentials: *const ENamedParameters,
    cancellable: *mut GCancellable,
    out_auth_result: *mut ESourceAuthenticationResult,
    error: *mut *mut GError,
    open: F,
) -> Option<T>
where
    F: FnOnce(&SourceConfig, Credentials) -> Result<T, ConnectError>,
{
    // A backend without a source cannot be configured, so no prompt helps.
    // This is not expected, because EDS constructs the backend *from* a
    // source. But a NULL dereference in `evolution-addressbook-factory` stops
    // all other accounts in the process too.
    if source.is_null() {
        tracing::error!(
            collection = %kind,
            "the collection backend has no account source to connect to"
        );
        // SAFETY: the out-parameters satisfy the contract of this function.
        unsafe {
            write_auth_result(out_auth_result, E_SOURCE_AUTHENTICATION_ERROR);
            set_raw_gerror(error, no_source_gerror(kind));
        }
        return None;
    }

    // SAFETY: `source` is a valid ESource, checked non-NULL above.
    let account_id = unsafe { crate::marshal::read_string(eds_sys::e_source_get_uid(source)) };

    // SAFETY: `source` is a valid ESource, checked non-NULL above.
    let config = match unsafe { SourceConfig::from_source(source) } {
        Ok(config) => config,
        Err(failure) => {
            // A misconfigured account: a password re-prompt cannot fix a
            // missing host or a plaintext origin. Thus this is never REJECTED.
            tracing::error!(
                collection = %kind,
                ?account_id,
                %failure,
                "source configuration error"
            );
            // SAFETY: as above.
            unsafe {
                write_auth_result(out_auth_result, E_SOURCE_AUTHENTICATION_ERROR);
                set_raw_gerror(error, failure.to_gerror());
            }
            return None;
        }
    };

    // SAFETY: `credentials` is NULL or a valid ENamedParameters, which
    // outlives the call.
    let password = unsafe { stored_password(credentials) };
    // SAFETY: `cancellable` is NULL or a valid GCancellable. EDS keeps it
    // alive for the duration of the vfunc, which outlives this scope.
    let _cancel = unsafe { observe(cancellable) };

    // The authentication scheme of this account is decided here, once, for
    // the same reason as the `out_auth_result` classification. An address
    // book and a calendar on one account must use the same login method.
    // The value is kept: a failure of `open` also needs it, to tell a 401 on
    // the token that this call fetched from a 401 on a usual password.
    // SAFETY: `source` is a valid ESource, checked non-NULL above.
    let scheme = unsafe { AuthScheme::of(source) };
    let uses_oauth2 = scheme == AuthScheme::OAuth2;
    let uses_api_token = scheme == AuthScheme::ApiToken;
    match scheme {
        AuthScheme::OAuth2 => tracing::debug!(
            collection = %kind,
            ?account_id,
            "authenticating with OAuth 2.0"
        ),
        AuthScheme::ApiToken => tracing::debug!(
            collection = %kind,
            ?account_id,
            "authenticating with an API token"
        ),
        AuthScheme::Password => tracing::debug!(
            collection = %kind,
            ?account_id,
            "authenticating with a password"
        ),
    }
    // SAFETY: `source` is a valid ESource, checked non-NULL above.
    // `cancellable` satisfies the contract of `access_token` through the
    // contract of this function.
    let resolved = unsafe {
        resolve_credentials(
            source,
            scheme,
            config.user.as_deref(),
            || password,
            cancellable,
        )
    };
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(failure) => {
            let escalates_to_consent = failure.auth_result() == E_SOURCE_AUTHENTICATION_REQUIRED;
            let reason = match &failure {
                ConnectError::OAuth2(_) => "oauth2_token_unavailable",
                ConnectError::SecretStore(_) => "secret_store_failure",
                ConnectError::ServiceGone(_) => "service_gone",
                ConnectError::CredentialsRequired => "password_required",
                _ => "credentials_error",
            };
            tracing::debug!(
                collection = %kind,
                ?account_id,
                uses_oauth2,
                uses_api_token,
                escalates_to_consent,
                reason,
                ?failure,
                "credentials resolution failed"
            );
            // SAFETY: as above.
            unsafe {
                write_auth_result(out_auth_result, failure.auth_result());
                set_raw_gerror(error, failure.to_gerror());
            }
            return None;
        }
    };

    match finish_connect(
        kind,
        account_id.as_deref(),
        uses_oauth2,
        open(&config, resolved),
    ) {
        Ok(opened) => {
            // SAFETY: as above.
            unsafe { write_auth_result(out_auth_result, ACCEPTED_AUTH_RESULT) };
            Some(opened)
        }
        Err(failure) => {
            // SAFETY: as above.
            unsafe {
                write_auth_result(out_auth_result, failure.auth_result());
                set_raw_gerror(error, failure.to_gerror());
            }
            None
        }
    }
}

/// What `connect_with` reports, from the answer of `open`.
///
/// This is a separate plain function so that a test can examine the
/// reclassification decision without a live `ESource`. The decision: apply
/// [`ConnectError::reclassify_oauth2_rejection`] only to the failure of an
/// OAuth 2.0 attempt, and keep all other outcomes from `open` unchanged.
/// `jmap_mail::service::finish_authenticate` is separate from `attempt` for
/// a `CamelService` in the same way.
fn finish_connect<T>(
    collection: Collection,
    account_id: Option<&str>,
    uses_oauth2: bool,
    outcome: Result<T, ConnectError>,
) -> Result<T, ConnectError> {
    let outcome = if uses_oauth2 {
        outcome.map_err(ConnectError::reclassify_oauth2_rejection)
    } else {
        outcome
    };
    match &outcome {
        Ok(_) => tracing::debug!(
            collection = %collection,
            ?account_id,
            uses_oauth2,
            "backend connected"
        ),
        Err(error) => {
            let escalates_to_consent =
                error.auth_result() == eds_sys::E_SOURCE_AUTHENTICATION_REQUIRED;
            let reason = if uses_oauth2 && matches!(error, ConnectError::OAuth2(_)) {
                "oauth2_consent_required"
            } else if matches!(error, ConnectError::SecretStore(_)) {
                "secret_store_failure"
            } else if matches!(error, ConnectError::ServiceGone(_)) {
                "service_gone"
            } else if matches!(error, ConnectError::CredentialsRequired) {
                "credentials_required"
            } else {
                "connect_error"
            };
            tracing::debug!(
                collection = %collection,
                ?account_id,
                uses_oauth2,
                escalates_to_consent,
                reason,
                ?error,
                "backend connect failed"
            );
        }
    }
    outcome
}

fn no_source_gerror(kind: Collection) -> *mut GError {
    invalid_arg_gerror(&translate_with(
        // TRANSLATORS: %1$s is "address book" or "calendar".
        c"the %1$s backend has no account to connect to",
        &[kind.noun().as_str()],
    ))
}

/// Writes an `out_auth_result`. The caller possibly did not request it.
///
/// # Safety
///
/// `dest` must be NULL or point at a writable `ESourceAuthenticationResult`.
pub unsafe fn write_auth_result(
    dest: *mut ESourceAuthenticationResult,
    value: ESourceAuthenticationResult,
) {
    if !dest.is_null() {
        // SAFETY: `dest` is writable by the contract above.
        unsafe { *dest = value };
    }
}

#[cfg(test)]
mod tests {
    use jmap_tracing_capture::capture;

    use super::*;

    fn id(s: &str) -> Id {
        Id::new(s)
    }

    /// The rule that all prompt behaviour uses, in one place: exactly one
    /// failure re-prompts.
    #[test]
    fn only_a_401_makes_evolution_ask_for_the_password_again() {
        assert_eq!(
            ConnectError::Client(Error::Http {
                status: 401,
                problem: None,
            })
            .auth_result(),
            E_SOURCE_AUTHENTICATION_REJECTED
        );
        for error in [
            ConnectError::Client(Error::Http {
                status: 403,
                problem: None,
            }),
            ConnectError::Client(Error::Transport("down".to_owned())),
            ConnectError::NoDefaultCollection(Collection::Calendar),
            ConnectError::NoSuchCollection(Collection::AddressBook, "Ab1".to_owned()),
            ConnectError::SecretStore("the login keyring is locked".to_owned()),
            ConnectError::ServiceGone("the name :1.4 has no owner".to_owned()),
        ] {
            assert_eq!(
                error.auth_result(),
                E_SOURCE_AUTHENTICATION_ERROR,
                "auth result for {error}"
            );
        }
        assert_eq!(
            ConnectError::CredentialsRequired.auth_result(),
            E_SOURCE_AUTHENTICATION_REQUIRED
        );
        assert_eq!(
            ConnectError::OAuth2("consent required".to_owned()).auth_result(),
            E_SOURCE_AUTHENTICATION_REQUIRED
        );
    }

    /// `ensure_connected_sync` (EDS 3.52.3 `e-book-meta-backend.c:3598-3630`)
    /// calls `e_backend_schedule_credentials_required`, the interactive
    /// consent escalation, for each `auth_result` except `UNKNOWN`. `UNKNOWN`
    /// is also the only value for which it propagates a real `GError` and
    /// does not clear it into an incorrect success. A new consent does not
    /// fix a redirect that withheld credentials. It only causes repeated
    /// requests against the same redirect: an endless OAuth2 popup loop.
    #[test]
    fn a_redirect_that_withheld_credentials_is_unknown_not_an_escalation() {
        assert_eq!(
            ConnectError::Client(Error::RedirectCredentialsWithheld {
                requested: "https://a.example/.well-known/jmap".to_owned(),
                followed: "https://b.example/.well-known/jmap".to_owned(),
            })
            .auth_result(),
            E_SOURCE_AUTHENTICATION_UNKNOWN
        );
    }

    /// A secret-store failure must not give `REQUIRED`. This test prevents
    /// the "try harder and harder to acquire a token" loop. The test also
    /// makes sure that the message gets to the user and is not discarded.
    #[test]
    fn a_secret_store_failure_does_not_reopen_consent() {
        let error = ConnectError::SecretStore("the login keyring is locked".to_owned());
        assert_ne!(error.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);
        assert_eq!(error.auth_result(), E_SOURCE_AUTHENTICATION_ERROR);
        assert_eq!(error.to_string(), "the login keyring is locked");

        let gerror = error.to_gerror();
        assert!(!gerror.is_null());
        unsafe {
            assert_eq!((*gerror).domain, eds_sys::e_client_error_quark());
            assert_eq!((*gerror).code, E_CLIENT_ERROR_DBUS_ERROR as i32);
            let message = crate::marshal::read_string((*gerror).message).unwrap();
            assert!(message.contains("the login keyring is locked"), "{message}");
            glib_sys::g_error_free(gerror);
        }
    }

    /// Each ConnectError variant must map to the related EClientError code,
    /// which the Evolution UI and retry loops use for routing.
    #[test]
    fn each_connect_error_variant_maps_to_the_expected_client_error_code() {
        for (error, expected) in [
            (
                ConnectError::CredentialsRequired,
                E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
            ),
            (
                ConnectError::OAuth2("need consent".into()),
                E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
            ),
            (
                ConnectError::SecretStore("keyring dead".into()),
                E_CLIENT_ERROR_DBUS_ERROR,
            ),
            (
                ConnectError::ServiceGone("the name :1.4 has no owner".into()),
                E_CLIENT_ERROR_DBUS_ERROR,
            ),
            (
                ConnectError::NoSuchCollection(Collection::AddressBook, "x".into()),
                E_CLIENT_ERROR_INVALID_ARG,
            ),
            (
                ConnectError::NoDefaultCollection(Collection::Calendar),
                E_CLIENT_ERROR_INVALID_ARG,
            ),
            (
                ConnectError::Client(Error::Transport("offline".into())),
                eds_sys::E_CLIENT_ERROR_REPOSITORY_OFFLINE,
            ),
            (
                ConnectError::Client(Error::Http {
                    status: 401,
                    problem: None,
                }),
                eds_sys::E_CLIENT_ERROR_AUTHENTICATION_FAILED,
            ),
            (
                ConnectError::Client(Error::Http {
                    status: 403,
                    problem: None,
                }),
                eds_sys::E_CLIENT_ERROR_PERMISSION_DENIED,
            ),
        ] {
            let gerror = error.to_gerror();
            assert!(!gerror.is_null(), "no GError for {error:?}");
            unsafe {
                assert_eq!(
                    (*gerror).domain,
                    eds_sys::e_client_error_quark(),
                    "domain for {error:?}"
                );
                assert_eq!((*gerror).code, expected as i32, "code for {error:?}");
                glib_sys::g_error_free(gerror);
            }
        }
    }

    /// A secret-store failure during connect stays SecretStore and does not
    /// become OAuth2. Thus it stays ERROR and is never REQUIRED.
    #[test]
    fn finish_connect_leaves_secret_store_error_unmodified_for_oauth2_account() {
        let store_error = || Err::<(), _>(ConnectError::SecretStore("keyring locked".to_owned()));
        let result = finish_connect(Collection::AddressBook, Some("acc-1"), true, store_error());
        assert!(
            matches!(result, Err(ConnectError::SecretStore(ref msg)) if msg == "keyring locked")
        );
        let err = result.unwrap_err();
        assert_eq!(err.auth_result(), E_SOURCE_AUTHENTICATION_ERROR);
    }

    /// The sibling of the test above: on an OAuth 2.0 account, a dead D-Bus
    /// peer stays `ServiceGone` through `finish_connect`. Thus it stays
    /// `ERROR` and never becomes the consent window that the captured trace
    /// showed. The reclassification only changes a 401. This test makes sure
    /// that an OAuth 2.0 account alone does not open consent again.
    #[test]
    fn finish_connect_leaves_a_dead_peer_unmodified_for_oauth2_account() {
        let dead_peer = || {
            Err::<(), _>(ConnectError::ServiceGone(
                "the name :1.4 has no owner".to_owned(),
            ))
        };
        let result = finish_connect(Collection::Calendar, Some("acc-1"), true, dead_peer());
        assert!(
            matches!(result, Err(ConnectError::ServiceGone(ref msg)) if msg == "the name :1.4 has no owner")
        );
        let err = result.unwrap_err();
        assert_eq!(err.auth_result(), E_SOURCE_AUTHENTICATION_ERROR);
    }

    /// The message names the collection because the user sees the message.
    /// If a backend reported the noun of the other backend, the user would
    /// edit the wrong account.
    #[test]
    fn the_message_names_the_collection_the_source_stands_for() {
        assert!(
            ConnectError::NoDefaultCollection(Collection::Calendar)
                .to_string()
                .contains("no default calendar")
        );
        assert!(
            ConnectError::NoSuchCollection(Collection::AddressBook, "Ab1".to_owned())
                .to_string()
                .contains("names address book \"Ab1\"")
        );
    }

    /// The one arm without its own collection or server text. A separate
    /// test makes sure that the translation retrofit did not silently change
    /// it into an empty string.
    #[test]
    fn credentials_required_names_the_missing_password() {
        assert_eq!(
            ConnectError::CredentialsRequired.to_string(),
            "the account has no password yet"
        );
    }

    #[test]
    fn a_named_collection_is_looked_up_and_a_missing_one_is_refused() {
        let list = [(Some(id("A")), Some(false)), (Some(id("B")), Some(true))];
        let candidates = || list.iter().map(|(id, d)| (id.as_ref(), *d));

        assert_eq!(
            resolve(Collection::Calendar, Some("A"), candidates()).unwrap(),
            id("A")
        );
        assert!(matches!(
            resolve(Collection::Calendar, Some("C"), candidates()),
            Err(ConnectError::NoSuchCollection(Collection::Calendar, ref w)) if w == "C"
        ));
    }

    /// Never "the first one", and never one that the server only listed
    /// before the default.
    #[test]
    fn an_unnamed_collection_resolves_to_the_flagged_default_only() {
        let list = [
            (Some(id("A")), None),
            (Some(id("B")), Some(false)),
            (Some(id("C")), Some(true)),
        ];
        assert_eq!(
            resolve(
                Collection::AddressBook,
                None,
                list.iter().map(|(id, d)| (id.as_ref(), *d))
            )
            .unwrap(),
            id("C")
        );

        let none_default = [(Some(id("A")), Some(false)), (Some(id("B")), None)];
        assert!(matches!(
            resolve(
                Collection::AddressBook,
                None,
                none_default.iter().map(|(id, d)| (id.as_ref(), *d))
            ),
            Err(ConnectError::NoDefaultCollection(Collection::AddressBook))
        ));
    }

    /// A collection that the server flags as default but gives no id cannot
    /// be synced. It must not hide a default collection that has an id.
    #[test]
    fn a_collection_with_no_id_is_not_a_candidate() {
        let list = [(None, Some(true)), (Some(id("B")), Some(true))];
        assert_eq!(
            resolve(
                Collection::Calendar,
                None,
                list.iter().map(|(id, d)| (id.as_ref(), *d))
            )
            .unwrap(),
            id("B")
        );
    }

    /// The reclassification only applies to a 401, and only changes it into
    /// `OAuth2`. It never changes other failures. This mirrors the
    /// `jmap_mail` test `only_the_401_shape_is_reclassified`.
    #[test]
    fn only_a_401_is_reclassified_to_oauth2_required() {
        let unauthorized = ConnectError::Client(Error::Http {
            status: 401,
            problem: None,
        });
        let reclassified = unauthorized.reclassify_oauth2_rejection();
        assert!(
            matches!(reclassified, ConnectError::OAuth2(_)),
            "expected OAuth2, got {reclassified}"
        );
        assert_eq!(reclassified.auth_result(), E_SOURCE_AUTHENTICATION_REQUIRED);

        for error in [
            ConnectError::Client(Error::Http {
                status: 403,
                problem: None,
            }),
            ConnectError::Client(Error::Transport("down".to_owned())),
        ] {
            let message = error.to_string();
            let reclassified = error.reclassify_oauth2_rejection();
            assert!(
                matches!(reclassified, ConnectError::Client(_)),
                "expected the error left alone, got {reclassified}"
            );
            assert_eq!(reclassified.to_string(), message);
        }
    }

    /// The gate that `connect_with` applies to the outcome of `open`. Only
    /// the 401 of an OAuth 2.0 attempt is reclassified. Success always stays
    /// unchanged. `jmap_mail::service::finish_authenticate_tests` tests the
    /// same two cases for mail.
    #[test]
    fn finish_connect_reclassifies_only_when_the_attempt_was_oauth2() {
        let unauthorized = || {
            Err::<(), _>(ConnectError::Client(Error::Http {
                status: 401,
                problem: None,
            }))
        };

        assert!(matches!(
            finish_connect(Collection::AddressBook, Some("acc-1"), true, unauthorized()),
            Err(ConnectError::OAuth2(_))
        ));
        assert!(matches!(
            finish_connect(Collection::Calendar, Some("acc-2"), false, unauthorized()),
            Err(ConnectError::Client(_))
        ));
        assert!(finish_connect(Collection::AddressBook, Some("acc-1"), true, Ok(())).is_ok());
        assert!(finish_connect(Collection::Calendar, Some("acc-2"), false, Ok(())).is_ok());
    }

    /// The API-token sibling of the user/password matrix of [`credentials`].
    /// This method has no user name. Thus the only question is whether a
    /// prompt for a token has occurred.
    #[test]
    fn a_stored_token_is_sent_as_bearer_and_an_absent_one_is_required() {
        assert!(matches!(
            bearer_credentials(Some("t0k3n")),
            Ok(Credentials::Bearer(ref token)) if token == "t0k3n"
        ));
        assert!(matches!(
            bearer_credentials(None),
            Err(ConnectError::CredentialsRequired)
        ));
    }

    #[test]
    fn finish_connect_traces_successful_connection_with_structured_fields() {
        let (result, entries) =
            capture(|| finish_connect(Collection::AddressBook, Some("account-abc"), true, Ok(())));
        assert!(result.is_ok());

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "collection" && value == "address book"),
            "expected collection='address book', got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "account_id" && value == "Some(\"account-abc\")"),
            "expected account_id=Some(\"account-abc\"), got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "uses_oauth2" && value == "true"),
            "expected uses_oauth2=true, got {entries:?}"
        );
    }

    #[test]
    fn finish_connect_traces_failed_connection_with_error_and_structured_fields() {
        let (result, entries) = capture(|| {
            finish_connect(
                Collection::Calendar,
                Some("account-xyz"),
                false,
                Err::<(), _>(ConnectError::CredentialsRequired),
            )
        });
        assert!(result.is_err());

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "collection" && value == "calendar"),
            "expected collection='calendar', got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "account_id" && value == "Some(\"account-xyz\")"),
            "expected account_id=Some(\"account-xyz\"), got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "uses_oauth2" && value == "false"),
            "expected uses_oauth2=false, got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "escalates_to_consent" && value == "true"),
            "expected escalates_to_consent=true, got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "reason" && value == "credentials_required"),
            "expected reason='credentials_required', got {entries:?}"
        );
        assert!(
            entries.iter().any(|(_, name, _)| name == "error"),
            "expected error field, got {entries:?}"
        );
    }

    #[test]
    fn finish_connect_traces_oauth2_reclassification_and_consent_escalation() {
        let unauthorized = || {
            Err::<(), _>(ConnectError::Client(Error::Http {
                status: 401,
                problem: None,
            }))
        };

        let (result, entries) = capture(|| {
            finish_connect(
                Collection::AddressBook,
                Some("oauth-acc"),
                true,
                unauthorized(),
            )
        });
        assert!(result.is_err());

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "collection" && value == "address book"),
            "expected collection='address book', got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "account_id" && value == "Some(\"oauth-acc\")"),
            "expected account_id=Some(\"oauth-acc\"), got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "uses_oauth2" && value == "true"),
            "expected uses_oauth2=true, got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "escalates_to_consent" && value == "true"),
            "expected escalates_to_consent=true, got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "reason" && value == "oauth2_consent_required"),
            "expected reason='oauth2_consent_required', got {entries:?}"
        );
    }

    /// `finish_connect` applies `reclassify_oauth2_rejection` only when
    /// `uses_oauth2` is true. Thus that path cannot give an `OAuth2(_)` error
    /// on a non-OAuth2 account. But the reason classification reads
    /// `uses_oauth2` and the shape of the error as two separate conditions.
    /// Never report a password account as waiting for OAuth2 consent only
    /// because the error has that variant. An operator who reads
    /// "oauth2_consent_required" on a password account tries the wrong fix.
    #[test]
    fn finish_connect_does_not_report_oauth2_consent_for_a_non_oauth2_account() {
        let (_, entries) = capture(|| {
            finish_connect(
                Collection::AddressBook,
                Some("password-acc"),
                false,
                Err::<(), _>(ConnectError::OAuth2("stray consent error".to_owned())),
            )
        });

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "reason" && value == "connect_error"),
            "expected reason='connect_error', got {entries:?}"
        );
    }

    /// The `GError` for a NULL source, without the unsafe `connect_with`
    /// entry point that makes it. It names the correct noun for each
    /// collection kind. Its code is `INVALID_ARG`, which the caller of
    /// `finish_connect` uses to route "misconfigured, do not re-prompt".
    #[test]
    fn no_source_gerror_names_the_collection_kind() {
        for (kind, noun) in [
            (Collection::AddressBook, "address book"),
            (Collection::Calendar, "calendar"),
        ] {
            let gerror = no_source_gerror(kind);
            assert!(!gerror.is_null(), "no GError for {kind:?}");
            unsafe {
                assert_eq!((*gerror).domain, eds_sys::e_client_error_quark());
                assert_eq!((*gerror).code, E_CLIENT_ERROR_INVALID_ARG as i32);
                let message = crate::marshal::read_string((*gerror).message).unwrap();
                assert!(message.contains(noun), "{message}");
                glib_sys::g_error_free(gerror);
            }
        }
    }
}
