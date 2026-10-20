// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of an account that the user has switched on, and what a
//! switched-off part means for the children.
//!
//! An Evolution account is not all-or-nothing. `ESourceCollection` has the
//! flags `mail-enabled`, `contacts-enabled` and `calendar-enabled`, and the user
//! can switch off each one separately (for example: "use this account for mail,
//! but my contacts are in a different place"). EDS gives the same three flags to
//! a backend as `ECollectionBackendParts` / `e_collection_backend_get_part_enabled()`.
//! A populate that ignores them fetches, creates and shows data that the user
//! does not want.
//!
//! [`Parts`] holds the three flags. This module makes the two decisions that
//! they control:
//! - what a populate *asks the server for*;
//! - what a populate does with the children that a previous populate created.
//!
//! ## A switched-off part is not asked about
//!
//! [`Fanout::discover`] sends `AddressBook/get` only when the contacts part is
//! on, and `Calendar/get` only when the calendar part is on. This is the same
//! gate that `EWebDAVCollectionBackend` puts in front of its discovery: it
//! returns before it contacts the server when neither part is enabled. Thus a
//! switched-off part also gets no children, because no listing exists to create
//! them from. [`Fanout::children`] applies the same rule a second time, so a
//! fan-out that is built by hand cannot avoid it.
//!
//! This backend does not create these children at all, and does not create them
//! disabled. The reason: EDS binds the `enabled` property of each child to its
//! part (`collection_backend_bind_child_enabled()`). Thus a child created for a
//! switched-off part is immediately a switched-off child. The user sees the same
//! result, but it costs a request, a `.source` file and a credential prompt for
//! data that the user does not want.
//!
//! ## A switched-off part is also not *deleted*
//!
//! This half can lose data, and here this backend is intentionally different
//! from the WebDAV backend of EDS.
//!
//! A collection backend must remove the children of collections that the server
//! no longer has. If not, an address book deleted in the web UI stays in the
//! sidebar permanently. `EWebDAVCollectionBackend` does this as follows:
//! 1. It lists all existing children.
//! 2. It removes from that list each child that discovery finds again.
//! 3. It calls `e_source_remove_sync()` on the children that remain.
//!
//! The problem: the list has children of *both* kinds, but discovery finds only
//! the enabled kinds. Thus, when contacts are switched off, each address book
//! child remains on the list and is removed, together with its uid, its
//! `.source` file and its offline cache. When the user switches contacts on
//! again, discovery finds the same address books as new sources.
//!
//! [`Fanout::is_obsolete`] does not do this. A cached child of the user's own
//! account is obsolete only when this populate *asked the question that it
//! answers*:
//! - its part is on;
//! - the login resolved an account for its kind;
//! - the listing did not contain it.
//!
//! A child of a switched-off part is dormant, not obsolete. The EDS binding has
//! already switched it off, and when the user switches the part on again, the
//! child returns with its cache. The same rule applies to a login that no
//! longer advertises a capability: an absent answer is not a deletion.
//!
//! A shared child also needs its part to be on. It is obsolete when its account
//! is no longer in the session (the owner withdrew the last grant), or when its
//! account was listed and the listing did not contain it. A shared listing that
//! fails is `None`, and thus removes nothing.
//!
//! This module does not handle a *failed* discovery, because a failed discovery
//! has no [`Fanout`]: [`Fanout::discover`] returns the error instead. A populate
//! that gets an error knows nothing new and must remove nothing. The WebDAV
//! backend states the same rule ("prevent lost of already known calendars when
//! the discover failed"). Here, the missing object enforces it.

use crate::children::{ChildKind, parse_resource_id, split_shared_identity};
use crate::resources::Fanout;

/// The parts of an account that the user has switched on.
///
/// The names come from `ESourceCollection`: `mail-enabled`, `contacts-enabled`
/// and `calendar-enabled`. EDS uses the singular for the last one. Here it is
/// [`calendars`](Parts::calendars), to agree with
/// [`CollectionLayout`](crate::CollectionLayout), which has one account for each
/// *kind* of collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    /// `ESourceCollection:mail-enabled`. Only [`Parts::any`] reads it: the
    /// populate creates no mail children (see [`crate::children`]). The mail
    /// service costs no request, because it comes from the session document
    /// that each populate fetches. The field exists because EDS has the flag,
    /// and a [`Parts`] with only two of the three EDS parts is a trap.
    pub mail: bool,
    /// `ESourceCollection:contacts-enabled`.
    pub contacts: bool,
    /// `ESourceCollection:calendar-enabled`.
    pub calendars: bool,
}

impl Parts {
    /// All parts: the value for a collection source that does not state its parts.
    pub const ALL: Self = Self {
        mail: true,
        contacts: true,
        calendars: true,
    };

    /// No parts: the value for a *disabled* collection source.
    pub const NONE: Self = Self {
        mail: false,
        contacts: false,
        calendars: false,
    };

    /// The parts of the collection source, by the two rules of
    /// `e_collection_backend_get_part_enabled()`.
    ///
    /// `source_enabled` is the `enabled` property of the collection `ESource`. A
    /// disabled account has no enabled parts, whatever its extension contains.
    /// `collection` is the three flags of the `[Collection]` extension. `None`
    /// is a source without that extension. EDS answers `TRUE` for it, so the
    /// result is [`ALL`](Parts::ALL), not [`NONE`](Parts::NONE).
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

    /// Whether a part is on, as `e_collection_backend_get_part_enabled()` with
    /// all bits set. For an account with no enabled part, a populate has nothing
    /// to do and sends no request to the server.
    pub fn any(self) -> bool {
        self.mail || self.contacts || self.calendars
    }
}

impl Fanout {
    /// Whether this populate listed the collections of `kind` in the user's own
    /// account.
    ///
    /// True only when the part is on *and* the login resolved an account for
    /// that kind. [`Fanout::discover`] uses the same two conditions for the
    /// listing, so the question and the conclusions from its answer cannot
    /// drift apart. False means that the vector for that kind is empty because
    /// nothing was asked, not because the account has no collections.
    pub fn listed(&self, kind: ChildKind) -> bool {
        self.parts.wants(kind) && self.layout.serves(kind)
    }

    /// Whether the cached child named `resource_id` should be removed.
    ///
    /// A populate asks this for each child that it has. Here, a wrong answer
    /// destroys data: `true` means `e_source_remove_sync()`, which removes the
    /// uid, the `.source` file and the offline cache of the child.
    ///
    /// The result is `true` only when the resource id is one of ours, its part
    /// is on, and one of these is true:
    /// - for a child of the user's own account: its collections *were* listed
    ///   ([`Fanout::listed`]) and the listing did not contain it;
    /// - for a shared child: its account is no longer in the session, or the
    ///   listing of its account was sent, succeeded and did not contain it.
    ///
    /// All other children are kept: a resource id that this backend did not
    /// write, a switched-off part, and a kind that the login no longer offers.
    pub fn is_obsolete(&self, resource_id: &str) -> bool {
        let Some((kind, identity)) = parse_resource_id(resource_id) else {
            return false;
        };
        if let Some((account, _)) = split_shared_identity(identity.as_str()) {
            // A listing that was not sent, or that failed, proves nothing.
            return self.parts.wants(kind)
                && self
                    .shared
                    .iter()
                    .find(|listing| listing.account.id.as_str() == account)
                    .is_none_or(|listing| {
                        listing.listing(kind).is_some()
                            && self
                                .children()
                                .iter()
                                .all(|child| child.resource_id != resource_id)
                    });
        }
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
    use crate::resources::{Resource, SharedListing};

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
            deletable: None,
        }
    }

    /// A login that serves both collection kinds, discovered under `parts`. The
    /// caller gives the vectors as [`Fanout::discover`] would give them.
    fn fanout(parts: Parts, address_books: &[&str], calendars: &[&str]) -> Fanout {
        Fanout {
            shared: Vec::new(),
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
        // source has no [Collection] extension. Thus no extension means "yes".
        // If it meant "no", a populate would do nothing.
        assert_eq!(Parts::from_collection(true, None), Parts::ALL);
    }

    #[test]
    fn a_disabled_account_has_no_enabled_parts() {
        // `e_collection_backend_get_part_enabled()` checks the `enabled`
        // property of the collection source before it examines the extension.
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
        // `any()` gates the whole populate. A mail-only account is a usual JMAP
        // login for mail, not an empty account.
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
        // Nothing was asked, so nothing is created. `discover` gates the
        // vectors. `children()` applies the same gate, so a fan-out built in a
        // different way cannot give a child of a switched-off part.
        let parts = Parts {
            calendars: false,
            ..Parts::ALL
        };
        let fanout = Fanout {
            shared: Vec::new(),
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
        // The part is on, but the session document resolved no contacts
        // account. Thus no `AddressBook/get` was sent, and the empty vector
        // means "not known", not "an empty account".
        let fanout = Fanout {
            shared: Vec::new(),
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
        // An own child is removed only when its part is on, its account answered,
        // and the answer did not contain it. If not, an address book deleted in
        // the web UI stays in the sidebar permanently.
        let fanout = fanout(Parts::ALL, &["AB1"], &["Cal1"]);

        assert!(fanout.is_obsolete("addressbook:AB2"));
        assert!(fanout.is_obsolete("calendar:Cal9"));
        assert!(!fanout.is_obsolete("addressbook:AB1"));
        assert!(!fanout.is_obsolete("calendar:Cal1"));
    }

    #[test]
    fn a_child_of_a_switched_off_part_is_dormant_and_not_obsolete() {
        // The WebDAV backend removes these children. Its user then loses the
        // uid and the offline cache of each address book when contacts are
        // switched off. EDS binds the `enabled` property of the child to the
        // part. To keep the child costs a hidden `.source` file, and the user
        // gets back the full cache when the part is switched on again.
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
        // For a login that no longer advertises `urn:ietf:params:jmap:contacts`,
        // no `AddressBook/get` is sent. If that meant "the server has no address
        // books", an upgrade of the server could delete each contacts child.
        let fanout = Fanout {
            shared: Vec::new(),
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
        // `is_obsolete` gets every child of the collection, also the mail
        // children. `true` for one of those deletes a source that is not ours.
        let fanout = fanout(Parts::ALL, &["AB1"], &["Cal1"]);

        for foreign in ["", "AB1", "mail:A1", "task:T1", "addressbook:"] {
            assert!(
                !fanout.is_obsolete(foreign),
                "{foreign:?} was taken for a child of ours and removed"
            );
            assert!(parse_resource_id(foreign).is_none(), "sanity: {foreign:?}");
        }
    }

    fn shared(id: &str, calendars: Option<&[&str]>) -> SharedListing {
        SharedListing {
            account: account(id),
            address_books: None,
            calendars: calendars.map(|ids| ids.iter().map(|id| resource(id)).collect()),
        }
    }

    #[test]
    fn a_shared_collection_the_owner_stopped_sharing_is_obsolete() {
        let mut fanout = fanout(Parts::ALL, &[], &[]);
        fanout.shared = vec![shared("A2", Some(&["C1"]))];

        assert!(!fanout.is_obsolete("calendar:A2/C1"));
        assert!(
            fanout.is_obsolete("calendar:A2/C9"),
            "the account was listed and no longer has it"
        );
        assert!(
            fanout.is_obsolete("calendar:A3/C1"),
            "the account left the session: its last grant was withdrawn"
        );
    }

    #[test]
    fn a_shared_child_whose_listing_was_not_sent_or_failed_is_kept() {
        let mut unlisted = fanout(Parts::ALL, &[], &[]);
        unlisted.shared = vec![shared("A2", None)];
        assert!(!unlisted.is_obsolete("calendar:A2/C1"));
        assert!(
            !unlisted.is_obsolete("addressbook:A2/AB1"),
            "address books of A2 were not listed either"
        );

        // A switched-off part keeps the child dormant, as for the user's own
        // collections, also when the account is gone.
        let mut off = fanout(
            Parts {
                calendars: false,
                ..Parts::ALL
            },
            &[],
            &[],
        );
        off.shared = Vec::new();
        assert!(!off.is_obsolete("calendar:A3/C1"));
    }

    #[test]
    fn a_malformed_shared_identity_is_judged_as_the_users_own() {
        // `/C1` is not a share. It is a collection id in the user's own
        // account, and the listing of the user's calendars did not contain it.
        let fanout = fanout(Parts::ALL, &[], &["C1"]);
        assert!(fanout.is_obsolete("calendar:/C1"));
        assert!(!fanout.is_obsolete("calendar:C1"));
    }
}
