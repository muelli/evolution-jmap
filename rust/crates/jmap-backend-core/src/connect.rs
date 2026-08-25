// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening the connection `connect_sync` needs, with no GObject in sight.
//!
//! `connect_sync` is the one vfunc that has to answer two questions at once:
//! did this work, and — if not — should Evolution ask the user for a password
//! again? The second answer is `out_auth_result`, and getting it wrong is the
//! difference between a prompt that fixes the account and an account that is
//! permanently broken with no way to say so. [`ConnectError::auth_result`] is
//! that decision, kept next to the failures it classifies.
//!
//! It is here rather than in a backend because the decision must not differ
//! between them. An address book and a calendar reach this code by different
//! routes and resolve different collections, but "a 401 is the only failure
//! worth re-prompting for" is a property of EDS's credentials machinery, not
//! of contacts — and the way a rule like that goes wrong is by being written
//! twice and then corrected once. What the backends keep for themselves is
//! [`Collection`]-shaped: which capability they ask the session for, and which
//! list they look their identifier up in.

use std::fmt;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_REQUIRED, E_CLIENT_ERROR_INVALID_ARG,
    E_SOURCE_AUTHENTICATION_ACCEPTED, E_SOURCE_AUTHENTICATION_ERROR,
    E_SOURCE_AUTHENTICATION_REJECTED, E_SOURCE_AUTHENTICATION_REQUIRED, EClientError,
    ENamedParameters, ESource, ESourceAuthenticationResult, e_client_error_create,
};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_client::{Credentials, Error};
use jmap_proto::Id;

use crate::api_token::source_uses_api_token;
use crate::cancel::observe;
use crate::error::{cstring_lossy, set_raw_gerror};
use crate::i18n::{translate, translate_with};
use crate::marshal::password as stored_password;
use crate::oauth2::{access_token, source_uses_oauth2};
use crate::source::SourceConfig;

/// What `connect_sync` writes into `out_auth_result` when it succeeds.
pub const ACCEPTED_AUTH_RESULT: ESourceAuthenticationResult = E_SOURCE_AUTHENTICATION_ACCEPTED;

/// The kind of server-side collection one EDS source stands for.
///
/// The two backends are otherwise identical here, and the distinction survives
/// only because it reaches the user: "the account names calendar \"Cal-1\",
/// which the server does not have" is a sentence someone can act on, and "the
/// account names collection \"Cal-1\"" is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collection {
    AddressBook,
    Calendar,
}

impl Collection {
    /// How to name this collection in a message shown to the user, in the
    /// user's language.
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
    /// The account authenticates with OAuth 2.0 and no access token could be
    /// had — nobody has consented to it yet, or the refresh did not work. The
    /// string is EDS's own message for the failure.
    ///
    /// Classified as [`E_SOURCE_AUTHENTICATION_REQUIRED`] rather than
    /// `REJECTED` on purpose, and the distinction matters more here than on
    /// the password path: `REQUIRED` is what opens the consent window, while
    /// `REJECTED` additionally tells EDS to throw the stored secret away. For
    /// OAuth 2.0 that secret is the *refresh* token, which a network blip
    /// during the exchange has not invalidated — discarding it would turn a
    /// transient failure into a re-consent the user has to click through.
    OAuth2(String),
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
    /// again, so it is reserved for the one case where asking again can help:
    /// the server said the credentials were wrong. A 403 is *not* that case —
    /// the credentials were accepted and the account is not allowed — and a
    /// server that is down is not either.
    pub fn auth_result(&self) -> ESourceAuthenticationResult {
        match self {
            Self::CredentialsRequired | Self::OAuth2(_) => E_SOURCE_AUTHENTICATION_REQUIRED,
            Self::Client(error) if is_wrong_password(error) => E_SOURCE_AUTHENTICATION_REJECTED,
            _ => E_SOURCE_AUTHENTICATION_ERROR,
        }
    }

    /// Turns a server's 401 on an OAuth 2.0 bearer token into the same
    /// [`Self::OAuth2`] failure a failed *fetch* of that token already gets.
    ///
    /// Call only where the credentials just tried were themselves an OAuth
    /// 2.0 access token — every caller already knows that from the same
    /// [`crate::oauth2::source_uses_oauth2`] read that chose the credential in
    /// the first place. A 401 on a Basic or API-token attempt is left alone:
    /// there, [`is_wrong_password`] correctly means the password was wrong,
    /// and `REJECTED` (ask again, discard what is stored) is the right
    /// answer.
    ///
    /// For OAuth 2.0 it is not, for the same reason [`Self::OAuth2`]'s own doc
    /// gives a failed fetch: `REQUIRED` opens a fresh consent window without
    /// discarding the stored refresh token, which a 401 moments after that
    /// token was successfully fetched has not invalidated — a transient
    /// rejection and a genuinely revoked grant look identical from here, and
    /// `REQUIRED` is the answer that does not overreact to the more common,
    /// transient case the way `REJECTED` would.
    ///
    /// Mirrors `jmap_mail::connect::StoreError::reclassify_oauth2_rejection` —
    /// same reasoning, applied to EDS's four-valued
    /// `ESourceAuthenticationResult` instead of Camel's three-valued one.
    pub fn reclassify_oauth2_rejection(self) -> Self {
        match self {
            Self::Client(error) if is_wrong_password(&error) => Self::OAuth2(error.to_string()),
            other => other,
        }
    }

    /// Allocates a `GError` describing this failure. Ownership passes to the
    /// caller, as with [`crate::error::to_gerror`].
    pub fn to_gerror(&self) -> *mut GError {
        if let Self::Client(error) = self {
            return crate::error::to_gerror(error);
        }
        let message = cstring_lossy(&self.to_string());
        // SAFETY: the code is one of the enum's own values and the message is
        // copied by the call.
        unsafe { e_client_error_create(self.client_error_code(), message.as_ptr()) }
    }

    fn client_error_code(&self) -> EClientError {
        match self {
            Self::CredentialsRequired | Self::OAuth2(_) => E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
            // Both collection failures are fixed by editing the account (or by
            // creating the collection on the server), never by retrying, so
            // they are reported the same way a malformed host is.
            _ => E_CLIENT_ERROR_INVALID_ARG,
        }
    }
}

/// Whether this failure means the credentials themselves were wrong, and so
/// whether asking the user for them again can help.
///
/// The one rule the EDS backends and the Camel provider must not answer
/// differently, which is why it is a function rather than a match arm in each
/// of them: the two report it through enums that have nothing in common
/// (`ESourceAuthenticationResult` and `CamelAuthenticationResult`), but the
/// question in front of the enum is the same one, and getting it wrong in
/// either direction is a product failure. Re-prompting on a 403 is a loop no
/// password ends; not re-prompting on a 401 is an account that is broken with
/// no way to say so.
///
/// A 403 is deliberately excluded: the credentials *were* accepted and the
/// account is not allowed to do this, which a different password does not fix.
/// So is an unreachable server.
pub fn is_wrong_password(error: &Error) -> bool {
    matches!(error, Error::Http { status: 401, .. })
}

/// What to authenticate as, given the source's user name and whatever EDS got
/// out of libsecret.
///
/// `password` is `None` on the first attempt: a source that names a user then
/// fails with [`ConnectError::CredentialsRequired`] rather than trying the
/// server anonymously, so the prompt happens before anything is sent. A source
/// that names no user is anonymous on purpose — that is `jmap-mockd` and a
/// development Stalwart, and a real server answers it with the 401 that turns
/// into a prompt anyway.
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

/// What to authenticate as for [`crate::api_token::source_uses_api_token`]'s
/// method — the same [`E_SOURCE_CREDENTIAL_PASSWORD`](crate::marshal::password)
/// slot Basic reads, sent as a Bearer token instead of paired with a user
/// name. There is no user-name half to this method — a token identifies the
/// account by itself — so the only question is whether one has been prompted
/// for yet; an empty stored token is sent as-is and left to the server's 401
/// to turn into a re-prompt, exactly as an empty stored password is on the
/// Basic path.
pub fn bearer_credentials(password: Option<&str>) -> Result<Credentials, ConnectError> {
    match password {
        Some(password) => Ok(Credentials::bearer(password)),
        None => Err(ConnectError::CredentialsRequired),
    }
}

/// Which of the server's collections this source stands for.
///
/// `candidates` is `(id, is_default)` for each collection the account offers,
/// in the order the server listed them — which is deliberately not used: a
/// source that names nothing gets the collection the *server* flags default,
/// never "the first one". Which one a source means is not something to guess
/// at, because the guess is where the user's data gets written.
///
/// A named collection is checked against the list rather than trusted: a typo
/// in a hand-written `.source` would otherwise present as a collection that is
/// merely empty, which is indistinguishable from a server that lost the
/// contents.
pub fn resolve<'a, I>(
    kind: Collection,
    wanted: Option<&str>,
    candidates: I,
) -> Result<Id, ConnectError>
where
    I: IntoIterator<Item = (Option<&'a Id>, Option<bool>)>,
{
    let candidates = candidates.into_iter().filter_map(|(id, is_default)| {
        // A collection the server did not name cannot be synced, whether or
        // not it claims to be the default one.
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

/// The whole of `connect_sync` except the instance and the collection lookup:
/// from the `ESource` EDS hands the backend to whatever `open` makes of it,
/// with `out_auth_result` and `error` written the way the vfunc has to write
/// them.
///
/// This is the layer a subclass calls, and it is here rather than next to the
/// other vfunc bodies because it is the only one whose input is an `ESource` —
/// which a test can build with `e_source_new_with_uid`, where a meta backend
/// would need a registry.
///
/// `out_auth_result` is written on **every** path, success included: EDS reads
/// it whenever the vfunc returns, and a stale value from a previous attempt is
/// how an account ends up either never prompting or prompting forever.
///
/// `cancellable` is [`observe`]d for the length of the connect and no longer,
/// and the connection `open` returns carries no cancellation of its own. That
/// is deliberate: a flag built into the client would belong to the *account*
/// rather than to the operation, so a connect the user stopped would leave a
/// client that had latched a cancellation nothing could clear — every later
/// operation on that account refusing, until it was reconnected. What stops an
/// operation after this one is the scope that operation's own vfunc installs.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource`, `credentials` NULL or a valid
/// `ENamedParameters`, `cancellable` NULL or a valid `GCancellable`, and the
/// two out-parameters NULL or writable — which is what an EDS vfunc receives.
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
    // A backend without a source cannot be configured, so no prompt helps. It
    // should not happen — EDS constructs the backend *from* a source — but a
    // NULL dereference in `evolution-addressbook-factory` takes every other
    // account in the process down with it.
    if source.is_null() {
        tracing::error!(
            collection = %kind,
            "the collection backend has no account source to connect to"
        );
        // SAFETY: the out-parameters satisfy the contract by this function's.
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
            // A misconfigured account: re-prompting for a password cannot fix
            // a missing host or a plaintext origin, so this is never REJECTED.
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
    // SAFETY: `cancellable` is NULL or a valid GCancellable that EDS keeps
    // alive for the duration of the vfunc, which outlives this scope.
    let _cancel = unsafe { observe(cancellable) };

    // Which authentication scheme this account uses is decided here, once, for
    // the same reason the `out_auth_result` classification is: an address book
    // and a calendar on one account must not disagree about how to log in.
    // Kept, not just asked once: `open`'s own failure needs to know it too,
    // to tell a 401 on the token this call just fetched apart from one on an
    // ordinary password.
    // SAFETY: `source` is a valid ESource, checked non-NULL above.
    let uses_oauth2 = unsafe { source_uses_oauth2(source) };
    let uses_api_token = unsafe { source_uses_api_token(source) };
    // SAFETY: `source` is a valid ESource, checked non-NULL above, and
    // `cancellable` satisfies `access_token`'s contract by this function's.
    let resolved = unsafe {
        if uses_oauth2 {
            tracing::debug!(
                collection = %kind,
                ?account_id,
                "authenticating with OAuth 2.0"
            );
            access_token(source, cancellable).map(Credentials::bearer)
        } else if uses_api_token {
            tracing::debug!(
                collection = %kind,
                ?account_id,
                "authenticating with an API token"
            );
            bearer_credentials(password.as_deref())
        } else {
            tracing::debug!(
                collection = %kind,
                ?account_id,
                "authenticating with a password"
            );
            self::credentials(config.user.as_deref(), password.as_deref())
        }
    };
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(failure) => {
            tracing::debug!(
                collection = %kind,
                ?account_id,
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

/// What `connect_with` reports, given what `open` itself answered.
///
/// Split out so the reclassification decision — apply
/// [`ConnectError::reclassify_oauth2_rejection`] only to an OAuth 2.0
/// attempt's failure, leave every other outcome exactly as `open` gave it —
/// is a plain function a test can drive without a live `ESource`, the same
/// way `jmap_mail::service::finish_authenticate` is split out of `attempt`
/// for a `CamelService`.
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
            tracing::debug!(
                collection = %collection,
                ?account_id,
                uses_oauth2,
                ?error,
                "backend connect failed"
            );
        }
    }
    outcome
}

fn no_source_gerror(kind: Collection) -> *mut GError {
    let message = cstring_lossy(&translate_with(
        // TRANSLATORS: %1$s is "address book" or "calendar".
        c"the %1$s backend has no account to connect to",
        &[kind.noun().as_str()],
    ));
    // SAFETY: the code is one of the enum's own values and the message is
    // copied by the call.
    unsafe { e_client_error_create(E_CLIENT_ERROR_INVALID_ARG, message.as_ptr()) }
}

/// Writes an `out_auth_result` the caller may not have asked for.
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
    use super::*;

    fn id(s: &str) -> Id {
        Id::new(s)
    }

    /// The rule the whole prompt behaviour rests on, in one place: exactly one
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
    }

    /// The collection is named in the message because the message is what the
    /// user sees; a backend that reported the other one's noun would send
    /// someone editing the wrong account.
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

    /// The one arm with no collection or server text of its own to lean on —
    /// pinned down separately so the translation retrofit could not quietly
    /// turn it into an empty string.
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

    /// Never "the first one", and never one the server merely happened to list
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

    /// A server that flags a collection default without giving it an id has
    /// named nothing that can be synced, and must not shadow one that has one.
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

    /// The reclassification only fires for the one shape it exists for — a
    /// 401 — and only turns it into `OAuth2`, never touching any other
    /// failure, mirroring `jmap_mail`'s
    /// `only_the_401_shape_is_reclassified`.
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

    /// The gate `connect_with` applies around `open`'s outcome: only an
    /// OAuth 2.0 attempt's 401 is reclassified, and success passes through
    /// unchanged either way — the same two cases
    /// `jmap_mail::service::finish_authenticate_tests` pins for the mail
    /// side.
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

    /// The API-token sibling of [`credentials`]'s own user/password matrix:
    /// there is no user-name half to this method, so the only question is
    /// whether a token has been prompted for yet.
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

    struct CapturingSubscriber {
        captured: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    struct Recorder<'a>(&'a std::sync::Mutex<Vec<(String, String)>>);

    impl tracing::field::Visit for Recorder<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .unwrap()
                .push((field.name().to_owned(), format!("{value:?}")));
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0
                .lock()
                .unwrap()
                .push((field.name().to_owned(), value.to_owned()));
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            self.0
                .lock()
                .unwrap()
                .push((field.name().to_owned(), value.to_string()));
        }
    }

    impl tracing::Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            event.record(&mut Recorder(&self.captured));
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[test]
    fn finish_connect_traces_successful_connection_with_structured_fields() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            captured: captured.clone(),
        };

        let result = tracing::subscriber::with_default(subscriber, || {
            finish_connect(Collection::AddressBook, Some("account-abc"), true, Ok(()))
        });
        assert!(result.is_ok());

        let entries = captured.lock().unwrap();
        assert!(
            entries.contains(&("collection".to_owned(), "address book".to_owned())),
            "expected collection='address book', got {entries:?}"
        );
        assert!(
            entries.contains(&("account_id".to_owned(), "Some(\"account-abc\")".to_owned())),
            "expected account_id=Some(\"account-abc\"), got {entries:?}"
        );
        assert!(
            entries.contains(&("uses_oauth2".to_owned(), "true".to_owned())),
            "expected uses_oauth2=true, got {entries:?}"
        );
    }

    #[test]
    fn finish_connect_traces_failed_connection_with_error_and_structured_fields() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            captured: captured.clone(),
        };

        let result = tracing::subscriber::with_default(subscriber, || {
            finish_connect(
                Collection::Calendar,
                Some("account-xyz"),
                false,
                Err::<(), _>(ConnectError::CredentialsRequired),
            )
        });
        assert!(result.is_err());

        let entries = captured.lock().unwrap();
        assert!(
            entries.contains(&("collection".to_owned(), "calendar".to_owned())),
            "expected collection='calendar', got {entries:?}"
        );
        assert!(
            entries.contains(&("account_id".to_owned(), "Some(\"account-xyz\")".to_owned())),
            "expected account_id=Some(\"account-xyz\"), got {entries:?}"
        );
        assert!(
            entries.contains(&("uses_oauth2".to_owned(), "false".to_owned())),
            "expected uses_oauth2=false, got {entries:?}"
        );
        assert!(
            entries.iter().any(|(k, _)| k == "error"),
            "expected error field, got {entries:?}"
        );
    }
}
