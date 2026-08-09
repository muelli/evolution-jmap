// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a setup refuses to commit.
//!
//! `EMailConfigServiceBackend` has a `check_complete` vfunc, and Evolution
//! greys out the assistant's *Next* until every backend on the page answers
//! yes. [`check`] is what this project's answer will be made of — the deciding
//! part of that vfunc, which is ordinary Rust over an [`Account`] and so can be
//! tested here, unlike the widget that will ask it.
//!
//! ## Why a setup checks at all, when the backends check anyway
//!
//! Every JMAP backend already refuses a bad server: `origin()` is called at
//! connect time and turns a missing host, a host that is not a host, and
//! plaintext to somewhere that is not this machine into an `E_CLIENT_ERROR`.
//! So nothing here prevents a wrong account from being *wrong* — it prevents it
//! from being *committed*, which is a different and better failure:
//!
//! - The refusal happens in the entry the mistake was typed into, while the
//!   user is still looking at it, rather than minutes later in a dialog raised
//!   by `evolution-source-registry` about an account they thought they had
//!   finished setting up.
//! - It happens once, rather than once per child: an account with an unusable
//!   server fans out to an address book and a calendar that each fail on their
//!   own schedule, in their own error dialogs.
//! - And for the TLS rule specifically it is the difference between a warning
//!   and a wall. The rule is a security decision this project made (M3:
//!   "TLS required for non-localhost"), and a setup that happily writes an
//!   account it knows will be refused has told the user their password is
//!   safe to type into it.
//!
//! ## The check and the reader are one decision, held in two places
//!
//! The server half of [`check`] is not a second opinion about hosts: it calls
//! [`origin`] — the same function
//! [the collection backend's reader](../../jmap_backend_collection/collection_source/fn.server_of.html)
//! calls, and the address book, calendar and mail backends with it — and keeps
//! its error. A rule spelled out again here would be a rule to fix twice.
//!
//! What is not shared is the *absence* of a host, because the two sides come by
//! it differently: the reader gets `NULL` out of a keyfile through
//! [`read_string`](jmap_backend_core::marshal::read_string), which is the
//! absent-or-non-empty form `origin` documents it takes, while a setup gets the
//! empty string out of an entry the user has not filled in. Mapping the one to
//! the other is this module's single line of translation, and
//! `tests/complete.rs` pins it by committing each case and reading it back with
//! the registry's own reader, so "the setup accepted what the registry will
//! reject" is a red test rather than an unusable account.
//!
//! ## What is deliberately not checked
//!
//! - **The user name.** [`credentials`](jmap_backend_core::connect::credentials)
//!   turns an absent user into an anonymous connection, which is exactly how
//!   `jmap-mockd` and a local development server are reached. Insisting on a
//!   user name would refuse the account this project is developed against.
//! - **Which parts are switched on.** An account with mail, contacts and
//!   calendars all off is not a mistake at commit time: the three mail sources
//!   are written either way ([`crate::mail`] says why), the children are the
//!   collection backend's to add and remove as the switches move, and the
//!   switches are on the account editor the user can open again.
//! - **Whether the server is *there*.** That is a network round trip, not a
//!   completeness check — `check_complete` runs on every keystroke.
//!   Reaching the server belongs in the assistant's own "look up account
//!   details" step, which is a later increment and a different vfunc.
//!
//! [`origin`]: jmap_backend_core::source::origin

use std::fmt;

use jmap_backend_core::source::{SourceError, origin};

use crate::account::Account;

/// Why an account cannot be committed yet.
///
/// The [`Display`](fmt::Display) text is written to be read by the person who
/// typed the answer in, in the place they typed it: it names the field, not the
/// function. `check_complete` itself has nowhere to put a message — the
/// vfunc answers a boolean — so this exists for the tooltip, the status label,
/// and for the log line when a commit is refused from somewhere without a
/// display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incomplete {
    /// The account names no address for its identity.
    ///
    /// Whitespace counts as nothing here, so an entry holding only spaces is
    /// reported as the unanswered question it looks like rather than as an
    /// address that happens not to parse.
    MissingIdentity,
    /// The identity is not an address.
    ///
    /// Held verbatim, including any surrounding whitespace: [`check`] reports
    /// what the user typed and does not repair it, because
    /// [`apply`](crate::account::apply) writes the same string unchanged and an
    /// accepted `" vera@example.com"` is a `From:` header with a space in it.
    InvalidIdentity(String),
    /// The server the account names is not one any backend would connect to —
    /// [`origin`]'s own verdict, kept rather than restated.
    ///
    /// [`origin`]: jmap_backend_core::source::origin
    Server(SourceError),
}

impl fmt::Display for Incomplete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingIdentity => f.write_str("the account has no email address"),
            Self::InvalidIdentity(identity) => {
                write!(f, "\"{identity}\" is not an email address")
            }
            Self::Server(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Incomplete {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingIdentity | Self::InvalidIdentity(_) => None,
            Self::Server(error) => Some(error),
        }
    }
}

/// Whether `account` is one a setup may commit, and if not, the first reason it
/// may not.
///
/// The identity before the server, which is the order the assistant asks in:
/// Evolution's identity page comes before its server settings page, and a check
/// that reported the second question's problem while the first was still blank
/// would be pointing at a page the user has not reached.
pub fn check(account: &Account) -> Result<(), Incomplete> {
    if account.identity.trim().is_empty() {
        return Err(Incomplete::MissingIdentity);
    }
    if !is_address(&account.identity) {
        return Err(Incomplete::InvalidIdentity(account.identity.clone()));
    }

    let connection = &account.connection;
    // The one translation: an unfilled entry is the empty string here and NULL
    // in the keyfile the registry will read, and `origin` takes the latter.
    let host = (!connection.host.is_empty()).then_some(&*connection.host);
    origin(host, connection.port.unwrap_or(0), connection.secure).map_err(Incomplete::Server)?;

    Ok(())
}

/// Whether `identity` is an address rather than a name, a domain or a sentence.
///
/// Deliberately not an RFC 5322 parser, and not a way in to writing one: what
/// this catches is the handful of things a person types into an email-address
/// entry that are not an email address, and the cost of a wrong answer is
/// asymmetric — a rule that rejected a real address would be a user who cannot
/// create their account at all, so anything this is unsure about it accepts and
/// leaves to the server to reject at login.
///
/// The quoted local part RFC 5322 allows (`"ve ra"@example.com`) is one of the
/// things this rejects; it is also one no JMAP server this project has met
/// issues, and the whitespace rule that rejects it is what catches the paste
/// that brought a newline with it.
fn is_address(identity: &str) -> bool {
    let Some((local, domain)) = identity.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !identity.contains(char::is_whitespace)
}
