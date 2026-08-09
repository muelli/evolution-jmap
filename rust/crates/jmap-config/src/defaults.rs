// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The account a setup starts from.
//!
//! `EMailConfigServiceBackend` has a `setup_defaults` vfunc, called once when
//! the user picks a backend on the assistant's *Receiving Email* page:
//! everything the page shows before it shows anything the user typed into it.
//! [`from_identity`] is that decision — ordinary Rust over the one answer the
//! assistant already has by then, which is the address from its identity page,
//! and so testable here, unlike the entries it will fill.
//!
//! ## Why the domain is the server, and why that is not a guess
//!
//! For IMAP a default host has to be guessed, and guessing `example.com` for
//! `vera@example.com` is usually wrong — mail servers live at `imap.` and
//! `mail.` and at names no rule produces. JMAP does not have that problem:
//! RFC 8620 §2.2 says a client that knows only the address fetches
//! `https://<domain>/.well-known/jmap`, and the server answers with wherever
//! its session, API and download URLs really are. So the domain is not a guess
//! at where the server is — it is the address the protocol itself specifies
//! for asking, and the account this crate writes names exactly that
//! ([`origin`](jmap_backend_core::source::origin) builds the URL the client
//! then fetches).
//!
//! Which is also why this crate can offer a default at all where an autoconfig
//! database would otherwise be needed, and why the last test in
//! `tests/defaults.rs` reads the committed account back with the registry's own
//! reader: the value of the default is that the origin the collection backend
//! ends up with is the one the address named, and a default that produced any
//! other origin would be worse than an empty entry.
//!
//! ## The login name, and the rule this does not break
//!
//! [`account`](crate::account) says the identity is "deliberately not derived
//! from" `[Authentication] User`, because the address a user's mail comes from
//! and the name they log in with are equal often enough to be assumed and
//! different often enough for the assumption to be wrong. That rule is about
//! what is *committed*, and it still holds: [`apply`](crate::account::apply)
//! writes whatever the entry says when the user presses the button, and never
//! looks at the identity.
//!
//! Here the two are related the only way they safely can be — as an offer.
//! Filling the login entry with the address is right for the JMAP servers this
//! project has met, wrong for the ones that want a bare account name, and in
//! both cases sitting in an entry the user is looking at and can edit. An empty
//! entry would have been the *other* default, and a worse one: an account with
//! no user name is an anonymous connection ([`credentials`] documents why that
//! is a legitimate state), so leaving it blank would offer the developer's
//! configuration to everybody else.
//!
//! ## What is deliberately left unanswered
//!
//! - **The port.** Unnamed, so the scheme's default applies. 443 is the right
//!   port and writing it down would still be wrong: an account that names one
//!   is an account that keeps naming it if the scheme ever changes underneath.
//! - **The authentication method.** `"none"` in EDS's spelling, which is its
//!   own "ask for a password the ordinary way" — the method the server offers
//!   is something a session document answers, not something a dialog guesses
//!   before it has ever connected.
//! - **The display name.** Not this crate's to write at all;
//!   [`account`](crate::account) says why.
//!
//! [`credentials`]: jmap_backend_core::connect::credentials

use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;

use crate::account::Account;

/// The account to offer a user who has typed `identity` and nothing else.
///
/// Every field is an answer the user can change afterwards; none of them is one
/// this reaches a network to find. `check_complete` runs on every
/// keystroke, and so does the vfunc that calls this — anything that had to ask
/// the server belongs in the assistant's own lookup step instead
/// ([`complete`](crate::complete) says the same of itself).
///
/// An `identity` that is not an address yet — the state the entry is in for as
/// long as the user is typing into it — yields an account with no server rather
/// than one with half an address in the server entry. It is not this function's
/// place to refuse it: [`check`](crate::complete::check) is what refuses, and
/// it names the address, which is the entry the user is in.
pub fn from_identity(identity: &str) -> Account {
    Account {
        identity: identity.to_owned(),
        connection: Connection {
            host: domain_of(identity).unwrap_or_default().to_owned(),
            port: None,
            // Offered, not derived — the module comment has the distinction.
            user: (!identity.is_empty()).then(|| identity.to_owned()),
            auth_method: None,
            // The project's rule (M3: TLS required for non-localhost), so also
            // the state the dialog opens in. Everything a user types into this
            // account afterwards — the password most of all — is typed into a
            // dialog that is already saying it will be sent over TLS.
            secure: true,
        },
        parts: Parts {
            mail: true,
            contacts: true,
            calendars: true,
        },
    }
}

/// The domain part of an address, if it has one.
///
/// From the *last* `@`, which is what an address means: a local part may not
/// contain an unquoted one, so a string with two has at most one reading in
/// which the tail is a domain. It is not a validity test —
/// [`check`](crate::complete::check) is, and refuses `a@b@example.com` whatever
/// this answers — only the question "is there something here to put in the
/// server entry yet".
///
/// A string that begins with the `@` has no local part and so names nobody at
/// any domain; there is nothing to offer. An address whose domain is still
/// being typed answers with the empty tail, which is the empty entry the
/// caller's `None` becomes anyway — one case, not two.
fn domain_of(identity: &str) -> Option<&str> {
    let (local, domain) = identity.rsplit_once('@')?;
    (!local.is_empty()).then_some(domain)
}
