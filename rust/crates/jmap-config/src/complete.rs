// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a setup refuses to commit.
//!
//! `EMailConfigServiceBackend` has a `check_complete` vfunc. Evolution disables
//! *Next* in the assistant until each backend on the page answers yes.
//! [`check`] is the deciding part of the answer of this project to that vfunc.
//! It is ordinary Rust on an [`Account`], thus the tests here can examine it
//! without the widget that calls it.
//!
//! ## Why a setup checks, although the backends also check
//!
//! Each JMAP backend already refuses a bad server. `origin()` runs at connect
//! time and changes these into an `E_CLIENT_ERROR`: a missing host, a host that
//! is not a host, and plaintext to a different machine. Thus this module does
//! not stop a wrong account from being *wrong*. It stops a commit of it, which
//! is a different and better failure:
//!
//! - The refusal occurs in the entry that has the mistake, while the user
//!   looks at it. It does not occur minutes later, in a dialog from
//!   `evolution-source-registry`, about an account that the user thinks is
//!   complete.
//! - It occurs once, not once for each child. An account with an unusable
//!   server makes an address book and a calendar, and each fails at its own
//!   time, in its own error dialog.
//! - For the TLS rule, it is the difference between a warning and a block. The
//!   rule is a security decision of this project (M3: "TLS required for
//!   non-localhost"). A setup that writes an account that it knows the backend
//!   refuses tells the user that it is safe to type a password into it.
//!
//! ## The check and the reader are one decision, in two places
//!
//! The server half of [`check`] does not make a second decision about hosts.
//! It calls [`origin`] and keeps its error.
//! [The reader of the collection backend](../../jmap_backend_collection/collection_source/fn.server_of.html)
//! and the address book, calendar and mail backends call the same function. A
//! rule written again here would be a rule to fix two times.
//!
//! The *absence* of a host is not shared, because the two sides get it in
//! different ways. The reader gets `NULL` from a keyfile through
//! [`read_string`](jmap_backend_core::marshal::read_string). That is the
//! absent-or-non-empty form that `origin` documents as its input. A setup gets
//! the empty string from an entry that the user did not fill in. The mapping
//! from one to the other is the single line of translation in this module.
//! `tests/complete.rs` pins it: it commits each case and reads it back with the
//! reader of the registry. Thus "the setup accepted what the registry refuses"
//! is a failed test, not an unusable account.
//!
//! ## What is deliberately not checked
//!
//! - **The user name.** [`credentials`](jmap_backend_core::connect::credentials)
//!   changes an absent user into an anonymous connection. This is how
//!   `jmap-mockd` and a local development server are reached. A required user
//!   name would refuse the account that this project uses for development.
//! - **Which parts are enabled.** An account with mail, contacts and calendars
//!   all disabled is not a mistake at commit time. The three mail sources are
//!   written in all cases ([`crate::mail`] tells why). The collection backend
//!   adds and removes the children when the switches change. The switches are
//!   in the account editor, which the user can open again.
//! - **Whether the server is *there*.** That is a network round trip, not a
//!   completeness check: `check_complete` runs on each keystroke. The
//!   "Look Up Account Details" step of the assistant contacts the server. It is
//!   a different vfunc, which `config_lookup.rs` implements.
//!
//! ## What *is* checked about OAuth 2.0, and why it is the exception
//!
//! [`Incomplete::OAuth2NotRegistered`] looks like the network check that the
//! list above excludes, but it is not a network check. It asks whether the
//! extension of [`crate::oauth2`] already holds a client id. That is local,
//! stored state, as is an empty host entry. It has its own variant, and is not
//! part of "the server is there", for this reason: a *missing* client is not
//! an unreachable server. It is an account that never tries, because
//! `EOAuth2Service::get_client_id` has no value to return.
//!
//! [`origin`]: jmap_backend_core::source::origin

use std::fmt;

use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::source::{SourceError, origin};

use crate::account::Account;
use jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD;

/// Why an account cannot be committed yet.
///
/// The [`Display`](fmt::Display) text names the field, not the function. It is
/// deliberately *not* the text of the status label. It is for developers
/// (English, not marked for translation): a log line gets the same text when a
/// commit is refused from a place without a display. [`status_message`] is the
/// translated form of the same decision, for users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incomplete {
    /// The account names no address for its identity.
    ///
    /// Whitespace counts as nothing here. Thus an entry with only spaces is
    /// reported as an unanswered question, not as an address that does not
    /// parse.
    MissingIdentity,
    /// The identity is not an address.
    ///
    /// Held verbatim, with any whitespace around it. [`check`] reports what
    /// the user typed and does not repair it. [`apply`](crate::account::apply)
    /// writes the same string without change, and an accepted
    /// `" vera@example.com"` is a `From:` header with a space in it.
    InvalidIdentity(String),
    /// No backend connects to the server that the account names. This is the
    /// decision of [`origin`], kept without change.
    ///
    /// [`origin`]: jmap_backend_core::source::origin
    Server(SourceError),
    /// The account selects OAuth 2.0 (`[Authentication] Method` is
    /// [`OAUTH2_AUTH_METHOD`]) but has no registered `[JMAP OAuth2]` client yet.
    ///
    /// The Authentication combo of `insert_widgets` lets the user select OAuth
    /// 2.0 directly, but it does not start registration. Only "Look Up Account
    /// Details" (`config_lookup.rs`) does that. A manual selection without that
    /// step makes `EOAuth2Service::get_client_id` return NULL, and the account
    /// can never fetch a token. This variant checks that fact. It is not a
    /// guess about a possible manual client-id entry in the future.
    OAuth2NotRegistered,
}

impl fmt::Display for Incomplete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingIdentity => f.write_str("the account has no email address"),
            Self::InvalidIdentity(identity) => {
                write!(f, "\"{identity}\" is not an email address")
            }
            Self::Server(error) => error.fmt(f),
            Self::OAuth2NotRegistered => {
                f.write_str("OAuth 2.0 is selected but no client is registered for it")
            }
        }
    }
}

impl std::error::Error for Incomplete {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingIdentity | Self::InvalidIdentity(_) | Self::OAuth2NotRegistered => None,
            Self::Server(error) => Some(error),
        }
    }
}

/// The text of the status label for `account`: empty when it can be committed,
/// else a translated description of the [`Incomplete`] reason.
///
/// `check_complete` returns a boolean and cannot show a message. The status
/// label of `insert_widgets` can, and this is all of the text that it shows.
/// This text is translated, not the `Display` of [`Incomplete`], because
/// account-setup text is for users. The two identity messages use the wording
/// of this crate and are marked here. For the `Server` case, this uses the
/// [`Display`](fmt::Display) of [`SourceError`] directly, which is also
/// translated. `SourceError` has its own `translate`/`translate_with` calls
/// (`jmap-backend-core/src/source.rs`), shared with the connection failures of
/// all other backends. Thus `to_string()` already returns the text for users.
/// A second wrapper would translate it two times.
pub fn status_message(account: &Account) -> String {
    match check(account) {
        Ok(()) => String::new(),
        Err(Incomplete::MissingIdentity) => translate(
            // TRANSLATORS: shown on the account setup page when no email address has been typed in yet.
            c"This account has no email address yet.",
        ),
        Err(Incomplete::InvalidIdentity(identity)) => translate_with(
            // TRANSLATORS: %1$s is exactly what was typed into the email address field.
            c"\"%1$s\" is not an email address.",
            &[&identity],
        ),
        Err(Incomplete::Server(error)) => error.to_string(),
        Err(Incomplete::OAuth2NotRegistered) => translate(
            // TRANSLATORS: shown when OAuth 2.0 is picked as the authentication method on the manual server-settings page; this account cannot be set up this way. "Look Up Account Details" is the name of the button on the earlier identity page that sets OAuth 2.0 up automatically instead.
            c"OAuth 2.0 can't be set up manually. Go back and use \"Look Up Account Details\" instead.",
        ),
    }
}

/// Whether a setup can commit `account`, and if not, the first reason.
///
/// The identity comes before the server, in the order of the assistant. The
/// identity page of Evolution comes before its server settings page. A check
/// that reported a server problem while the identity was still blank would
/// point to a page that the user has not seen yet.
pub fn check(account: &Account) -> Result<(), Incomplete> {
    if account.identity.trim().is_empty() {
        return Err(Incomplete::MissingIdentity);
    }
    if !is_address(&account.identity) {
        return Err(Incomplete::InvalidIdentity(account.identity.clone()));
    }

    let connection = &account.connection;
    // The one translation: an unfilled entry is the empty string here and NULL
    // in the keyfile that the registry reads. `origin` takes NULL.
    let host = (!connection.host.is_empty()).then_some(&*connection.host);
    origin(host, connection.port.unwrap_or(0), connection.secure).map_err(Incomplete::Server)?;

    let oauth2_method = OAUTH2_AUTH_METHOD
        .to_str()
        .expect("OAUTH2_AUTH_METHOD is a fixed ASCII string");
    if connection.auth_method.as_deref() == Some(oauth2_method) && !account.oauth2_registered {
        return Err(Incomplete::OAuth2NotRegistered);
    }

    Ok(())
}

/// Whether `identity` is an address, not a name, a domain or a sentence.
///
/// This is deliberately not an RFC 5322 parser, and not a start of one. It
/// finds the few usual inputs in an email-address entry that are not an email
/// address. The cost of a wrong answer is asymmetric: if a rule refused a real
/// address, the user could not create the account. Thus this function accepts
/// all that it is not sure about, and the server can refuse it at login.
///
/// This function refuses the quoted local part that RFC 5322 allows
/// (`"ve ra"@example.com`). No JMAP server that this project knows issues one.
/// The whitespace rule that refuses it also finds a paste with a newline in it.
fn is_address(identity: &str) -> bool {
    let Some((local, domain)) = identity.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !identity.contains(char::is_whitespace)
}

/// Tests [`status_message`] with plain [`Account`] fixtures. Different from the
/// suite in `tests/complete.rs`, these need no `ESource`: the account is a Rust
/// struct, and `status_message` never calls EDS.
#[cfg(test)]
mod tests {
    use jmap_collection_sync::Parts;
    use jmap_collection_sync::child_source::Connection;

    use super::*;

    /// The start account for each case: complete, in the order of checks of
    /// the module docs.
    fn complete_account() -> Account {
        Account {
            identity: "vera@example.com".to_owned(),
            connection: Connection {
                host: "jmap.example.com".to_owned(),
                port: Some(8443),
                user: Some("vera".to_owned()),
                auth_method: None,
                secure: true,
            },
            parts: Parts::ALL,
            oauth2_registered: false,
        }
    }

    #[test]
    fn a_complete_account_has_no_status_message() {
        assert_eq!(status_message(&complete_account()), "");
    }

    #[test]
    fn a_missing_identity_is_reported() {
        let account = Account {
            identity: String::new(),
            ..complete_account()
        };
        assert_eq!(
            status_message(&account),
            "This account has no email address yet."
        );
    }

    #[test]
    fn an_invalid_identity_is_reported() {
        let account = Account {
            identity: "not an address".to_owned(),
            ..complete_account()
        };
        assert_eq!(
            status_message(&account),
            "\"not an address\" is not an email address."
        );
    }

    #[test]
    fn oauth2_with_no_registered_client_is_reported() {
        let account = Account {
            connection: Connection {
                auth_method: Some(OAUTH2_AUTH_METHOD.to_str().unwrap().to_owned()),
                ..complete_account().connection
            },
            oauth2_registered: false,
            ..complete_account()
        };
        assert_eq!(check(&account), Err(Incomplete::OAuth2NotRegistered));
        assert_eq!(
            status_message(&account),
            "OAuth 2.0 can't be set up manually. Go back and use \"Look Up Account Details\" instead."
        );
    }

    #[test]
    fn oauth2_with_a_registered_client_has_no_status_message() {
        let account = Account {
            connection: Connection {
                auth_method: Some(OAUTH2_AUTH_METHOD.to_str().unwrap().to_owned()),
                ..complete_account().connection
            },
            oauth2_registered: true,
            ..complete_account()
        };
        assert_eq!(status_message(&account), "");
    }

    #[test]
    fn source_is_none_except_for_a_server_error() {
        use std::error::Error;

        assert!(Incomplete::MissingIdentity.source().is_none());
        assert!(
            Incomplete::InvalidIdentity(String::new())
                .source()
                .is_none()
        );
        assert!(Incomplete::OAuth2NotRegistered.source().is_none());

        let account = Account {
            connection: Connection {
                host: String::new(),
                secure: false,
                ..complete_account().connection
            },
            ..complete_account()
        };
        let incomplete = check(&account).unwrap_err();
        assert!(matches!(incomplete, Incomplete::Server(_)));
        assert_eq!(
            incomplete.source().unwrap().to_string(),
            incomplete.to_string()
        );
    }

    #[test]
    fn an_unusable_server_is_reported() {
        let account = Account {
            connection: Connection {
                host: String::new(),
                secure: false,
                ..complete_account().connection
            },
            ..complete_account()
        };
        assert_eq!(
            status_message(&account),
            check(&account).unwrap_err().to_string()
        );
        assert!(!status_message(&account).is_empty());
    }
}
