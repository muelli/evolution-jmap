// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which parts of an account the user has switched on, and what a switched-off
//! part means for the children.
//!
//! An Evolution account is not all-or-nothing: `ESourceCollection` carries
//! `mail-enabled`, `contacts-enabled` and `calendar-enabled`, and the user turns
//! them off one at a time — "use this account for mail, but I have my contacts
//! elsewhere". EDS exposes the same three to a backend as
//! `ECollectionBackendParts` / `e_collection_backend_get_part_enabled()`, and a
//! populate that ignores them fetches, creates and shows what the user said they
//! did not want.
//!
//! [`Parts`] is those three flags, and this module is the two decisions they
//! drive: what a populate *asks the server for*, and what it does with the
//! children a previous populate already created.
//!
//! ## A switched-off part is not asked about
//!
//! [`Fanout::discover`] sends `AddressBook/get` only when the contacts part is
//! on, and `Calendar/get` only when the calendar part is on — the same gate
//! `EWebDAVCollectionBackend` puts in front of its own discovery, which returns
//! before contacting anything when neither part is enabled. The children of a
//! switched-off part are therefore not created either, since there is nothing
//! listed to create them from ([`Fanout::children`] holds that line a second
//! time, so a hand-built fan-out cannot route around it).
//!
//! Not creating them, rather than creating them disabled, is a choice: EDS binds
//! every child's `enabled` to its part
//! (`collection_backend_bind_child_enabled()`), so a child created for a
//! switched-off part would immediately be a switched-off child — the same thing
//! the user sees, at the cost of a request, a `.source` file and a credential
//! prompt for data they said they did not want.
//!
//! ## But a switched-off part is not *deleted* either
//!
//! The other half is the one that can lose data, and it is where this backend
//! deliberately parts company with EDS's WebDAV one.
//!
//! A collection backend has to remove the children of collections that are gone
//! from the server, or an address book deleted in the web UI stays in the
//! sidebar forever. `EWebDAVCollectionBackend` does that by listing every
//! existing child up front, striking off the ones discovery found again, and
//! calling `e_source_remove_sync()` on the leftovers. The trouble is that it
//! fills that list with children of *both* kinds while discovering only the
//! enabled ones, so with contacts switched off every address book child becomes
//! a leftover and is removed — its uid, its `.source` file and its offline cache
//! with it. Switching contacts back on then rediscovers the same address books
//! as brand new sources.
//!
//! [`Fanout::is_obsolete`] does not do that. A cached child is obsolete only
//! when this populate *asked the question its answer would be*: its part is on,
//! the login resolved an account for its kind, and the listing came back without
//! it. A child of a switched-off part is dormant, not obsolete — EDS's binding
//! has already switched it off, and switching the part back on brings it back
//! with its cache intact. The same reasoning covers the login that stopped
//! advertising a capability: silence is not a deletion.
//!
//! Nothing here handles a *failed* discovery, because a failed discovery has no
//! [`Fanout`] to ask — [`Fanout::discover`] returns the error instead, and a
//! populate that got an error has learned nothing and must remove nothing. That
//! is the same rule ("prevent lost of already known calendars when the discover
//! failed", as the WebDAV backend puts it), reached by not having the object.

use crate::children::{ChildKind, parse_resource_id};
use crate::resources::Fanout;

/// Which of an account's three parts the user has switched on.
///
/// The names are `ESourceCollection`'s: `mail-enabled`, `contacts-enabled` and
/// `calendar-enabled` (EDS spells the last one singular; it is spelled
/// [`calendars`](Parts::calendars) here to match
/// [`CollectionLayout`](crate::CollectionLayout), which has one account per
/// *kind* of collection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    /// `ESourceCollection:mail-enabled`. Nothing turns on it yet: this backend
    /// creates no mail children (see [`crate::children`]), and the mail service
    /// costs no request of its own — it is read out of the session document
    /// every populate fetches anyway. It is here because the flag exists and a
    /// [`Parts`] that quietly had two thirds of EDS's three parts would be a
    /// trap for whoever writes the mail children.
    pub mail: bool,
    /// `ESourceCollection:contacts-enabled`.
    pub contacts: bool,
    /// `ESourceCollection:calendar-enabled`.
    pub calendars: bool,
}

impl Parts {
    /// Every part, which is what a collection source that says nothing means.
    pub const ALL: Self = Self {
        mail: true,
        contacts: true,
        calendars: true,
    };

    /// No part, which is what a *disabled* collection source means.
    pub const NONE: Self = Self {
        mail: false,
        contacts: false,
        calendars: false,
    };

    /// What the collection source says, by
    /// `e_collection_backend_get_part_enabled()`'s own two rules.
    ///
    /// `source_enabled` is the collection `ESource`'s own `enabled`: a disabled
    /// account has no enabled parts, whatever its extension says. `collection`
    /// is the `[Collection]` extension's three flags, and `None` is a source
    /// that has no such extension — which EDS answers `TRUE` for, so it is
    /// [`ALL`](Parts::ALL) rather than [`NONE`](Parts::NONE).
    pub fn from_collection(source_enabled: bool, collection: Option<Self>) -> Self {
        match (source_enabled, collection) {
            (false, _) => Self::NONE,
            (true, None) => Self::ALL,
            (true, Some(parts)) => parts,
        }
    }

    /// Whether children of `kind` are wanted at all.
    pub fn wants(self, kind: ChildKind) -> bool {
        match kind {
            ChildKind::AddressBook => self.contacts,
            ChildKind::Calendar => self.calendars,
        }
    }

    /// Whether any part is on — `e_collection_backend_get_part_enabled()` with
    /// every bit set. A populate of an account with nothing enabled has nothing
    /// to do, including nothing to ask the server.
    pub fn any(self) -> bool {
        self.mail || self.contacts || self.calendars
    }
}

impl Fanout {
    /// Whether this populate actually listed the collections of `kind`.
    ///
    /// True only when the part is on *and* the login resolved an account for
    /// that kind — the same pair [`Fanout::discover`] gates the listing on, so
    /// what was asked and what may be concluded from the answer cannot drift
    /// apart. False means this fan-out's vector for that kind is empty because
    /// nothing was asked, not because the account holds nothing.
    pub fn listed(&self, kind: ChildKind) -> bool {
        self.parts.wants(kind) && self.layout.serves(kind)
    }

    /// Whether the cached child named `resource_id` should be removed.
    ///
    /// The question a populate asks of every child it already has, and the one
    /// place a wrong answer destroys data rather than merely showing the wrong
    /// thing: `true` means `e_source_remove_sync()`, which takes the child's
    /// uid, its `.source` file and its offline cache.
    ///
    /// So it is `true` in exactly one case — the resource id is one of ours,
    /// its collections *were* listed ([`Fanout::listed`]), and the listing did
    /// not contain it. A resource id this backend did not write, and a child
    /// whose kind was not listed (its part is off, or the login no longer offers
    /// it), are both kept.
    pub fn is_obsolete(&self, resource_id: &str) -> bool {
        let Some((kind, _)) = parse_resource_id(resource_id) else {
            return false;
        };
        self.listed(kind)
            && self
                .children()
                .iter()
                .all(|child| child.resource_id != resource_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use jmap_proto::Id;

    use crate::layout::{CollectionLayout, ServiceAccount};
    use crate::resources::Resource;

    fn account(id: &str) -> ServiceAccount {
        ServiceAccount {
            id: Id::new(id),
            name: "Vera Vibes".to_owned(),
            read_only: false,
        }
    }

    fn resource(id: &str) -> Resource {
        Resource {
            id: Id::new(id),
            name: format!("Collection {id}"),
            is_default: false,
            color: None,
            writable: None,
        }
    }

    /// A login serving both collection kinds, discovered under `parts` — with
    /// the vectors gated the way [`Fanout::discover`] would have gated them.
    fn fanout(parts: Parts, address_books: &[&str], calendars: &[&str]) -> Fanout {
        Fanout {
            parts,
            layout: CollectionLayout {
                mail: None,
                contacts: Some(account("A1")),
                calendars: Some(account("A1")),
            },
            address_books: address_books.iter().copied().map(resource).collect(),
            calendars: calendars.iter().copied().map(resource).collect(),
        }
    }

    #[test]
    fn a_source_that_says_nothing_about_its_parts_has_all_of_them() {
        // `e_collection_backend_get_part_enabled()` returns TRUE when the
        // source has no [Collection] extension, so the absence is "yes" and not
        // "no" — reading it the other way would populate nothing at all.
        assert_eq!(Parts::from_collection(true, None), Parts::ALL);
    }

    #[test]
    fn a_disabled_account_has_no_enabled_parts() {
        // The first thing `e_collection_backend_get_part_enabled()` checks is
        // the collection source's own `enabled`, before it looks at the
        // extension at all.
        let mail_only = Parts {
            mail: true,
            ..Parts::NONE
        };

        assert_eq!(Parts::from_collection(false, Some(mail_only)), Parts::NONE);
        assert_eq!(Parts::from_collection(false, None), Parts::NONE);
        assert!(!Parts::NONE.any(), "and there is then nothing to populate");
    }

    #[test]
    fn an_enabled_account_is_taken_at_its_extensions_word() {
        let books_only = Parts {
            contacts: true,
            ..Parts::NONE
        };

        assert_eq!(Parts::from_collection(true, Some(books_only)), books_only);
        assert!(books_only.wants(ChildKind::AddressBook));
        assert!(!books_only.wants(ChildKind::Calendar));
        assert!(books_only.any());
    }

    #[test]
    fn a_mail_only_account_still_has_something_to_populate() {
        // `any()` gates the populate as a whole, and a mail-only account is the
        // ordinary shape of a JMAP login used for mail — not an empty one.
        let mail_only = Parts {
            mail: true,
            ..Parts::NONE
        };

        assert!(mail_only.any());
        assert!(!mail_only.wants(ChildKind::AddressBook));
        assert!(!mail_only.wants(ChildKind::Calendar));
    }

    #[test]
    fn a_switched_off_part_is_not_listed_and_warrants_no_children() {
        // Nothing was asked, so nothing is known and nothing is created. The
        // vectors are gated by `discover`; `children()` holds the same line so
        // that a fan-out assembled any other way cannot produce a child of a
        // part the user switched off.
        let parts = Parts {
            calendars: false,
            ..Parts::ALL
        };
        let fanout = Fanout {
            calendars: vec![resource("Cal1")],
            ..fanout(parts, &["AB1"], &[])
        };

        assert!(fanout.listed(ChildKind::AddressBook));
        assert!(!fanout.listed(ChildKind::Calendar));
        assert_eq!(
            fanout
                .children()
                .iter()
                .map(|child| child.resource_id.clone())
                .collect::<Vec<_>>(),
            ["addressbook:AB1"]
        );
    }

    #[test]
    fn a_kind_the_login_does_not_serve_was_not_listed_either() {
        // The part is on, so the user wants their contacts — but the session
        // document resolved no account for them, so no `AddressBook/get` was
        // sent and the empty vector is ignorance rather than an empty account.
        let fanout = Fanout {
            layout: CollectionLayout {
                mail: None,
                contacts: None,
                calendars: Some(account("A1")),
            },
            ..fanout(Parts::ALL, &[], &["Cal1"])
        };

        assert!(!fanout.listed(ChildKind::AddressBook));
        assert!(fanout.listed(ChildKind::Calendar));
    }

    #[test]
    fn a_collection_the_server_no_longer_lists_is_obsolete() {
        // The one case that removes a source: its part is on, its account
        // answered, and the answer did not contain it. Without this an address
        // book deleted in the web UI stays in the sidebar forever.
        let fanout = fanout(Parts::ALL, &["AB1"], &["Cal1"]);

        assert!(fanout.is_obsolete("addressbook:AB2"));
        assert!(fanout.is_obsolete("calendar:Cal9"));
        assert!(!fanout.is_obsolete("addressbook:AB1"));
        assert!(!fanout.is_obsolete("calendar:Cal1"));
    }

    #[test]
    fn a_child_of_a_switched_off_part_is_dormant_and_not_obsolete() {
        // The WebDAV backend removes these, and its user loses the uid and the
        // offline cache of every address book to a "don't use this account for
        // contacts" tick. EDS has already bound the child's `enabled` to the
        // part, so keeping it costs a hidden `.source` file and buys back the
        // whole cache when the part is switched on again.
        let parts = Parts {
            contacts: false,
            ..Parts::ALL
        };
        let fanout = fanout(parts, &[], &["Cal1"]);

        assert!(
            !fanout.is_obsolete("addressbook:AB1"),
            "switching contacts off is not deleting the contacts"
        );
        assert!(
            fanout.is_obsolete("calendar:Cal9"),
            "the calendars were listed, so their absence still means gone"
        );
    }

    #[test]
    fn a_child_of_a_kind_the_login_stopped_serving_is_kept() {
        // A login that no longer advertises `urn:ietf:params:jmap:contacts` is
        // one no `AddressBook/get` was sent for. Reading that silence as "the
        // server has no address books" would delete every contacts child of an
        // account whose server was mid-upgrade.
        let fanout = Fanout {
            layout: CollectionLayout {
                mail: None,
                contacts: None,
                calendars: Some(account("A1")),
            },
            ..fanout(Parts::ALL, &[], &["Cal1"])
        };

        assert!(!fanout.is_obsolete("addressbook:AB1"));
    }

    #[test]
    fn a_child_this_backend_did_not_create_is_never_obsolete() {
        // `is_obsolete` will be asked about every child of the collection,
        // including the mail ones once they exist. Answering `true` for one of
        // those is deleting a source that is not ours to delete.
        let fanout = fanout(Parts::ALL, &["AB1"], &["Cal1"]);

        for foreign in ["", "AB1", "mail:A1", "task:T1", "addressbook:"] {
            assert!(
                !fanout.is_obsolete(foreign),
                "{foreign:?} was taken for a child of ours and removed"
            );
            assert!(parse_resource_id(foreign).is_none(), "sanity: {foreign:?}");
        }
    }
}
