// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The account identity that an address can send through.
//!
//! [`Outgoing::identity`](crate::Outgoing::identity) is an id, but a
//! `CamelTransport` gets only an address: Camel's `from` argument, from the
//! account that the user selected in the composer. This module finds the
//! identity for that address among the account's identities (RFC 8621 §6).
//!
//! ## Not a permission check
//!
//! This module does not decide whether the user can send as an address. Under
//! RFC 8621 §7, the server refuses a submission whose `From` does not agree
//! with the identity. Only the server knows the account's permissions. Thus, a
//! match here is a proposal, and a too-generous match gives a refused send,
//! never mail from a different person. For this reason, the comparisons
//! prefer a match.
//!
//! ## The wildcard
//!
//! In RFC 8621 §6, an identity with the local part `*` (`*@example.com`)
//! means "any address in this domain". A server can publish only that
//! identity for a full domain, so an exact string comparison can find no
//! identity. The wildcard is a fallback: an identity with the exact address
//! has the user's name and signature, so it wins over the wildcard.

use jmap_proto::mail::Identity;

/// The account identity for a submission from `address`.
///
/// `None` if no identity covers the address.
/// [`MailSync::identity_for`](crate::MailSync::identity_for) converts this into
/// an error that names the address.
///
/// If several matches are equally good, the first wins. Thus, each retry of one
/// message gets the same identity and signature.
pub fn best_match<'a>(identities: &'a [Identity], address: &str) -> Option<&'a Identity> {
    identities
        .iter()
        .filter_map(|identity| match_kind(&identity.email, address).map(|kind| (kind, identity)))
        // `max_by_key` returns the last maximum. The reversal makes it return
        // the first exact match, or else the first wildcard match.
        .rev()
        .max_by_key(|(kind, _)| *kind)
        .map(|(_, identity)| identity)
}

/// How well an identity's address covers the sender address. The variants go
/// from worst to best, so [`Ord`] is the preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchKind {
    /// The identity is the domain wildcard, and the address is in that domain.
    Domain,
    /// The identity has this address.
    Address,
}

/// The comparison ignores ASCII case only, in the local part and the domain.
///
/// For the domain, this is correct: DNS ignores ASCII case, and a non-ASCII
/// domain arrives as an A-label.
///
/// For the local part, RFC 5321 §2.4 makes case significant, and only the
/// receiving host interprets it. But this is not a relay: both spellings are
/// the user's own address. A refused send because of a capital letter in the
/// identity helps nobody. Unicode case folding depends on the language, so the
/// comparison does not use it.
fn match_kind(identity: &str, address: &str) -> Option<MatchKind> {
    if identity.eq_ignore_ascii_case(address) {
        return Some(MatchKind::Address);
    }
    let (local, domain) = split(identity)?;
    // Only a local part of exactly `*` is a wildcard. `*alice@example.com` is
    // a usual address, not a wildcard.
    if local != "*" {
        return None;
    }
    // Compare the full domain, not a suffix: `example.com` does not cover
    // `notexample.com`.
    (split(address)?.1.eq_ignore_ascii_case(domain)).then_some(MatchKind::Domain)
}

/// An addr-spec's local part and domain.
///
/// The split is at the last `@` (RFC 5321 §4.1.2): a quoted local part can
/// contain `@`, but a domain cannot.
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
        // This provider's envelopes cannot hold this address, but a server can
        // publish it. SMTP accepts `postmaster` without a domain.
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
