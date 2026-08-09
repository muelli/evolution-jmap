// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which of the account's identities an address is allowed to go out through.
//!
//! [`Outgoing::identity`](crate::Outgoing::identity) is an id, and the only
//! thing a `CamelTransport` is handed is an address — Camel's `from` argument,
//! which the composer filled in from the account the user chose. This module is
//! the step between the two, and it is the whole of what "which identity"
//! means in this provider: the account's identities are read from the server
//! (RFC 8621 §6) and the one that owns the address is the one submitted
//! through.
//!
//! ## Not a permission check
//!
//! Nothing here decides whether the user may send as an address. RFC 8621 §7
//! has the *server* refuse a submission whose message `From` disagrees with the
//! identity, and it is the only side that can: it knows what the account is
//! entitled to and this process knows what a client was told. So a match found
//! here is a proposal, and the failure mode of being generous with it is a
//! refused send — never mail leaving as somebody else. That is why the
//! comparisons below lean towards matching rather than away from it.
//!
//! ## The wildcard
//!
//! RFC 8621 §6 gives an identity whose local part is the single character `*` —
//! `*@example.com` — the meaning "any address in this domain". A server hosting
//! a whole domain publishes exactly that and nothing else, so a client
//! comparing whole strings would tell such an account it cannot send at all.
//! The wildcard is a *fallback*, though: an identity that has the address
//! outright carries the user's name and signature, and the server writes the
//! message's `From` from it, so it wins wherever both would do.

use jmap_proto::mail::Identity;

/// The identity an address should be submitted through, out of the ones the
/// account has.
///
/// `None` when nothing covers the address — an answer, not a failure: an
/// account that has no identity for what the user is sending as is a thing to
/// tell them about, and it is [`MailSync::identity_for`](crate::MailSync::identity_for)
/// that has the address to name in the sentence.
///
/// The first of several equally good matches wins. Nothing distinguishes them
/// from here, and the point of pinning it is that the answer is the *same* one
/// every time: a lookup that picked differently on each attempt would put a
/// different signature on retries of one message.
pub(crate) fn best_match<'a>(identities: &'a [Identity], address: &str) -> Option<&'a Identity> {
    identities
        .iter()
        .filter_map(|identity| match_kind(&identity.email, address).map(|kind| (kind, identity)))
        // `max_by_key` on a plain iterator answers with the *last* maximum,
        // which would be the last exact match rather than the first; reversing
        // first makes it the first, and leaves the wildcard fallback ordered
        // the same way.
        .rev()
        .max_by_key(|(kind, _)| *kind)
        .map(|(_, identity)| identity)
}

/// How well one identity's address covers the address being sent from, ordered
/// worst to best so that [`Ord`] is the preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchKind {
    /// The identity is the domain's wildcard and the address is in it.
    Domain,
    /// The identity *is* this address.
    Address,
}

/// Only the ASCII case is folded, in both halves of the address.
///
/// For the domain that is exactly right: DNS is ASCII-case-insensitive, and a
/// name outside it reaches this crate as the A-label a URL carries.
///
/// For the local part it is the conservative half of a rule with no good
/// answer. RFC 5321 §2.4 makes a local part case-*sensitive* and reserves its
/// interpretation to the receiving host, so folding it is not something a
/// relaying client may do — but this is not a relay: both spellings are the
/// user's own address on their own account, and refusing to send because the
/// server wrote the identity with a capital would be a failure with nothing
/// behind it. Folding only ASCII stops there rather than going on to Unicode
/// case, where the fold is language-dependent and two addresses that fold
/// together are not reliably the same mailbox.
fn match_kind(identity: &str, address: &str) -> Option<MatchKind> {
    if identity.eq_ignore_ascii_case(address) {
        return Some(MatchKind::Address);
    }
    let (local, domain) = split(identity)?;
    // The single character, and not a prefix: `*alice@example.com` is an
    // address with an unusual name in it, and reading it as a wildcard would
    // send Bob's mail through Alice's identity.
    if local != "*" {
        return None;
    }
    // Whole domain, never a suffix — an account entitled to `example.com` is
    // not entitled to `notexample.com`.
    (split(address)?.1.eq_ignore_ascii_case(domain)).then_some(MatchKind::Domain)
}

/// An addr-spec's local part and domain.
///
/// The *last* `@`, per RFC 5321 §4.1.2: a quoted local part may contain one,
/// and the domain never does.
fn split(address: &str) -> Option<(&str, &str)> {
    let (local, domain) = address.rsplit_once('@')?;
    (!domain.is_empty()).then_some((local, domain))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(email: &str) -> Identity {
        Identity {
            email: email.to_owned(),
            ..Identity::default()
        }
    }

    #[test]
    fn an_address_with_no_domain_matches_nothing_but_itself() {
        // Not an address any envelope this provider builds can carry — but a
        // server may publish anything, and `postmaster` is a bare local part
        // SMTP does accept.
        let identities = [identity("*@example.com"), identity("postmaster")];
        assert_eq!(
            best_match(&identities, "postmaster").map(|found| found.email.as_str()),
            Some("postmaster")
        );
        assert!(best_match(&identities[..1], "postmaster").is_none());
    }

    #[test]
    fn an_identity_with_an_empty_domain_is_not_a_wildcard() {
        assert!(best_match(&[identity("*@")], "alice@").is_none());
    }
}
