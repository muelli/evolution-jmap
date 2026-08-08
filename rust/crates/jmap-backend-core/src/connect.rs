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
use jmap_client::transport::CancelFlag;
use jmap_client::{Credentials, Error};
use jmap_proto::Id;

use crate::cancel::CancelBridge;
use crate::error::{cstring_lossy, set_raw_gerror};
use crate::marshal::password as stored_password;
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
    /// How to name this collection in a message shown to the user.
    pub fn noun(self) -> &'static str {
        match self {
            Self::AddressBook => "address book",
            Self::Calendar => "calendar",
        }
    }
}

impl fmt::Display for Collection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.noun())
    }
}

/// A connection that could not be opened.
#[derive(Debug)]
pub enum ConnectError {
    /// The account names a user but EDS has no password for it yet.
    CredentialsRequired,
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
            Self::CredentialsRequired => f.write_str("the account has no password yet"),
            Self::Client(error) => error.fmt(f),
            Self::NoSuchCollection(kind, id) => write!(
                f,
                "the account names {kind} \"{id}\", which the server does not have"
            ),
            Self::NoDefaultCollection(kind) => {
                write!(f, "the server offers no default {kind} for this account")
            }
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
            Self::CredentialsRequired => E_SOURCE_AUTHENTICATION_REQUIRED,
            Self::Client(error) if is_wrong_password(error) => E_SOURCE_AUTHENTICATION_REJECTED,
            _ => E_SOURCE_AUTHENTICATION_ERROR,
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
            Self::CredentialsRequired => E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
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
    F: FnOnce(&SourceConfig, Option<&str>, CancelFlag) -> Result<T, ConnectError>,
{
    // A backend without a source cannot be configured, so no prompt helps. It
    // should not happen — EDS constructs the backend *from* a source — but a
    // NULL dereference in `evolution-addressbook-factory` takes every other
    // account in the process down with it.
    if source.is_null() {
        // SAFETY: the out-parameters satisfy the contract by this function's.
        unsafe {
            write_auth_result(out_auth_result, E_SOURCE_AUTHENTICATION_ERROR);
            set_raw_gerror(error, no_source_gerror(kind));
        }
        return None;
    }

    // SAFETY: `source` is a valid ESource, checked non-NULL above.
    let config = match unsafe { SourceConfig::from_source(source) } {
        Ok(config) => config,
        Err(failure) => {
            // A misconfigured account: re-prompting for a password cannot fix
            // a missing host or a plaintext origin, so this is never REJECTED.
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
    // alive for the duration of the vfunc, which outlives the bridge.
    let bridge = unsafe { CancelBridge::new(cancellable) };

    match open(&config, password.as_deref(), bridge.flag().clone()) {
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

fn no_source_gerror(kind: Collection) -> *mut GError {
    let message = cstring_lossy(&format!("the {kind} backend has no account to connect to"));
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
}
