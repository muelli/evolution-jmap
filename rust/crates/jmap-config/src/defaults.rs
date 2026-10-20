// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The account that a setup starts from.
//!
//! `EMailConfigServiceBackend` has a `setup_defaults` vfunc. Evolution calls it
//! before the *Receiving Email* page of the assistant opens: it gives all that
//! the page shows before the user types. [`from_identity`] is that decision.
//! It is ordinary Rust over the only answer the assistant has at that time
//! (the address from its identity page), so tests can examine it here. They
//! cannot examine the entries that it fills.
//!
//! The vfunc applies it through [`backend::setup`](crate::backend::setup),
//! which does not simply apply all of it. The address determines only the
//! fields *below*. The user can come back to the page, and the other answers
//! of the user must not be overwritten. That function tells why.
//!
//! ## Why the domain is the server, and why that is not a guess
//!
//! For IMAP, a default host is a guess, and `example.com` for
//! `vera@example.com` is usually wrong: mail servers are at `imap.`, at
//! `mail.` and at names that no rule gives. JMAP does not have that problem.
//! RFC 8620 §2.2 says that a client that knows only the address fetches
//! `https://<domain>/.well-known/jmap`. The server then gives the real
//! locations of its session, API and download URLs. Thus the domain is not a
//! guess about the server location. It is the address that the protocol
//! specifies for the request, and the account that this crate writes names
//! exactly that ([`origin`](jmap_backend_core::source::origin) builds the URL
//! that the client then fetches).
//!
//! For this reason, this crate can offer a default without an autoconfig
//! database. Also for this reason, the last test in `tests/defaults.rs` reads
//! the committed account back with the reader of the registry. The default is
//! useful only if the collection backend gets the origin that the address
//! named. A default that gives a different origin is worse than an empty entry.
//!
//! ## The login name, and the rule that this obeys
//!
//! [`account`](crate::account) says that the identity is "deliberately not
//! derived from" `[Authentication] User`. The address of the mail of a user
//! and the login name are frequently equal, but not always, so it is wrong to
//! assume that they are. That rule applies to what is *committed*, and it
//! stays true: [`apply`](crate::account::apply) writes the value of the entry
//! when the user pushes the button, and never reads the identity.
//!
//! Here the two are related in the only safe way: as an offer. To fill the
//! login entry with the address is correct for the JMAP servers that this
//! project knows, and wrong for servers that need a bare account name. In both
//! cases, the value is in an entry that the user sees and can edit. An empty
//! entry is the *other* possible default, and it is worse. An account with no
//! user name is an anonymous connection ([`credentials`] tells why that state
//! is valid). Thus an empty entry offers the configuration of the developer to
//! all other users.
//!
//! ## What is deliberately not answered
//!
//! - **The port.** Not named, so the default of the scheme applies. 443 is the
//!   correct port, but to write it is still wrong: an account that names a port
//!   keeps that port if the scheme changes.
//! - **The authentication method.** `"none"` in the EDS spelling, which means
//!   "ask for a password in the usual way". The session document tells which
//!   method the server offers. A dialog must not guess it before it connects.
//! - **The display name.** This crate does not write it at all.
//!   [`account`](crate::account) tells why.
//!
//! [`credentials`]: jmap_backend_core::connect::credentials

use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;

use crate::account::Account;

/// The account to offer a user who typed `identity` and nothing more.
///
/// The user can change each field later. No field comes from the network.
/// `check_complete` runs on each keystroke, and Evolution can call
/// `setup_defaults` (the vfunc that calls this) more than once. A value that
/// needs the server must come from the lookup step of the assistant
/// ([`complete`](crate::complete) says the same about itself).
///
/// While the user types, `identity` is frequently not an address yet. Then the
/// account has no server, not half an address in the server entry. This
/// function does not refuse it: [`check`](crate::complete::check) refuses it,
/// and names the address, which is the entry that the user is in.
pub fn from_identity(identity: &str) -> Account {
    Account {
        identity: identity.to_owned(),
        connection: Connection {
            host: domain_of(identity).unwrap_or_default().to_owned(),
            port: None,
            // Offered, not derived. The module comment tells the difference.
            user: (!identity.is_empty()).then(|| identity.to_owned()),
            auth_method: None,
            // The project rule (M3: TLS is necessary for non-localhost), so also
            // the initial state of the dialog. All that the user types later
            // (mostly the password) is typed into a dialog that shows TLS.
            secure: true,
        },
        parts: Parts {
            mail: true,
            contacts: true,
            calendars: true,
        },
        // A new account: no discovery has run against it yet.
        oauth2_registered: false,
    }
}

/// The domain part of an address, if it has one.
///
/// It starts after the *last* `@`. A local part cannot contain an unquoted `@`,
/// so in a string with two, only the tail can be a domain. This is not a
/// validity test: [`check`](crate::complete::check) is, and it refuses
/// `a@b@example.com` for all values of this function. This function only tells
/// if there is a value for the server entry yet.
///
/// A string that starts with `@` has no local part, so it names no person and
/// there is nothing to offer. For an address with a partly typed domain, the
/// result is the tail, which can be empty. An empty tail and `None` both give
/// an empty entry.
fn domain_of(identity: &str) -> Option<&str> {
    let (local, domain) = identity.rsplit_once('@')?;
    (!local.is_empty()).then_some(domain)
}
