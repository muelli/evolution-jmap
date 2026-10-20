// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The child sources that a populate creates, and the name of each one.
//!
//! [`Fanout`] tells what a login holds. This module tells what `populate` does
//! with it. `e_collection_backend_new_child (backend, resource_id)` takes a
//! *resource id* and returns the child `ESource`. Thus a backend does not make
//! child uids: it names resources, and EDS assigns and keeps the uids. The
//! `dup_resource_id` vfunc must return the same string for that child. On the
//! next populate, EDS uses this pair to find that the resource already has a
//! source, and does not make a second one.
//!
//! ## Why a resource id is not only the JMAP id
//!
//! JMAP ids are unique per account *and per object type* (RFC 8620 §1.2). A
//! server can have an `AddressBook` with id `a` and a `Calendar` with id `a`.
//! On a server that numbers its objects from one, this is the usual case. But
//! the resource id namespace is flat: it holds all children of this one
//! collection. With the bare JMAP id, the calendar `a` resolves to the child
//! source of the address book. The account then has one source too few, and a
//! contacts backend gets the data of a calendar.
//!
//! Thus the kind is part of the name: `addressbook:<id>` and `calendar:<id>`.
//! Parsing splits at the *first* colon and keeps the rest whole. The id charset
//! in RFC 8620 §1.2 has no colon, but this code cannot enforce that. If a
//! server sends a colon, the source must look wrong, not silently point at a
//! different collection.
//!
//! ## What is deliberately not here
//!
//! **The mail sources.** They are not children of the collection backend. If
//! `dup_resource_id` returns `NULL` for a cached child, EDS deletes its file,
//! and reference backends return `NULL` for mail extensions. Thus the account
//! setup (`jmap_config::mail`) writes them in the registry directory, with the
//! account as parent. `jmap-backend-collection`'s `prepare_mail` module tells
//! the full reason. [`Fanout::layout`] holds the mail service, but the child
//! list has only address books and calendars.

use jmap_proto::Id;

use crate::layout::ServiceAccount;
use crate::resources::{Fanout, Resource};

/// The Evolution backend that a child source is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    AddressBook,
    Calendar,
}

impl ChildKind {
    /// The part of a resource id before the colon.
    pub(crate) fn prefix(self) -> &'static str {
        match self {
            Self::AddressBook => "addressbook",
            Self::Calendar => "calendar",
        }
    }

    /// The resource id of `collection` in the primary account. This code
    /// gives it to `e_collection_backend_new_child`, and `dup_resource_id`
    /// must return it for the child that results.
    pub fn resource_id(self, collection: &Id) -> String {
        format!("{}:{}", self.prefix(), collection)
    }

    /// The resource id of `collection` in the shared `account`:
    /// `<kind>:<accountId>/<collectionId>`. The part after the colon is the
    /// `[Resource] Identity` of the child. The book and calendar backends split
    /// it into the account and the collection. JMAP ids cannot contain a `/`
    /// (RFC 8620 §1.2), so the split has only one result.
    pub fn shared_resource_id(self, account: &Id, collection: &Id) -> String {
        format!("{}:{account}/{collection}", self.prefix())
    }
}

/// The account and collection of a shared child's identity
/// (`<accountId>/<collectionId>`), or `None` for a primary child's bare id.
///
/// This is the rule that `SourceConfig` in the backends uses: exactly one `/`,
/// with text on each side. All other values stay whole, as a primary id.
pub fn split_shared_identity(identity: &str) -> Option<(&str, &str)> {
    let (account, collection) = identity.split_once('/')?;
    (!account.is_empty() && !collection.is_empty() && !collection.contains('/'))
        .then_some((account, collection))
}

/// One `ESource` that a populate creates, with all that its keyfile needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    /// The argument to `e_collection_backend_new_child`, and the value that
    /// `dup_resource_id` must return for this child. It is unique in the full
    /// child list. The bare JMAP id is not.
    pub resource_id: String,
    pub kind: ChildKind,
    /// `ESource:display-name`. For the user's own collections, this is the
    /// name that the server gives. For a shared one, it also has the owner.
    pub display_name: String,
    /// The `accountId` that the method calls of this child use. It is not a
    /// constant of the collection: contacts and calendars can be in different
    /// JMAP accounts, and each child uses its own.
    pub account_id: Id,
    /// The JMAP id of the collection that this child is for.
    pub collection_id: Id,
    /// True for the default collection of this kind in the account.
    pub is_default: bool,
    /// [`Resource::color`], unchanged. It is `None` for each address book
    /// child, and for a calendar that has no color from the server.
    pub color: Option<String>,
    /// True when no item in this child may be created, changed or removed.
    /// The value is the account-wide bit, which the `myRights` of the
    /// collection can narrow but never widen:
    ///
    /// - A writable account with a collection whose `myRights` says not
    ///   writable gives a read-only child.
    /// - A read-only account always gives read-only children.
    ///
    /// See [`Child::for_resource`].
    pub read_only: bool,
    /// The value for the `remote-deletable` `ESource` property of EDS.
    ///
    /// Unlike [`Child::read_only`], this is a real property
    /// (`e_server_side_source_set_remote_deletable`), not a fact that the
    /// backend examines at call time. It makes Evolution disable "Delete".
    /// Without it, Evolution offers "Delete" and then shows the `forbidden`
    /// error of the server after the click.
    ///
    /// No account-wide bit applies: `myRights.mayDelete` is per collection.
    /// For the user's own collections, an absent `mayDelete` gives `true`.
    /// For a shared one, it gives `false` (see [`Child::for_shared_resource`]).
    pub remote_deletable: bool,
}

impl Fanout {
    /// The children of this login, in the order of creation.
    ///
    /// The order is: address books, then calendars, then the shared children.
    /// Each group keeps the order of [`Fanout::discover`], which is the sort
    /// order from the server. A collection that the server lists two times
    /// becomes one child, and the first listing wins. Two children with one
    /// resource id are not two sources: they are one source that is created
    /// and then overwritten.
    ///
    /// A kind whose [`Parts`](crate::Parts) flag is off has no children here.
    /// For a discovered fan-out, this is already true because the listing was
    /// not sent. This method applies the same rule to a fan-out made by hand,
    /// so "the user switched contacts off" has only one meaning.
    pub fn children(&self) -> Vec<Child> {
        let books = self
            .layout
            .account_for(ChildKind::AddressBook)
            .filter(|_| self.parts.wants(ChildKind::AddressBook))
            .into_iter()
            .flat_map(|account| children(ChildKind::AddressBook, account, &self.address_books));
        let calendars = self
            .layout
            .account_for(ChildKind::Calendar)
            .filter(|_| self.parts.wants(ChildKind::Calendar))
            .into_iter()
            .flat_map(|account| children(ChildKind::Calendar, account, &self.calendars));

        // Shared children come after the user's own, one account at a time,
        // in the order of `Fanout::shared`.
        let shared = self.shared.iter().flat_map(|listing| {
            [ChildKind::AddressBook, ChildKind::Calendar]
                .into_iter()
                .filter(|kind| self.parts.wants(*kind))
                .flat_map(move |kind| {
                    listing
                        .listing(kind)
                        .unwrap_or_default()
                        .iter()
                        .map(move |resource| {
                            Child::for_shared_resource(kind, &listing.account, resource)
                        })
                })
        });

        let mut seen: Vec<String> = Vec::new();
        books
            .chain(calendars)
            .chain(shared)
            .filter(|child| {
                let fresh = !seen.contains(&child.resource_id);
                if fresh {
                    seen.push(child.resource_id.clone());
                }
                fresh
            })
            .collect()
    }
}

impl Child {
    /// The child source for one collection of `account`.
    ///
    /// This is public because discovery is not the only source of collections.
    /// A `create_resource_sync` makes one on the server, and must write the same
    /// child that the next populate writes. A second copy of this mapping can
    /// give a created source that differs from the discovered one in a field
    /// that nothing compares. These fields decide if EDS keeps the cache file,
    /// and which server the child uses.
    pub fn for_resource(kind: ChildKind, account: &ServiceAccount, resource: &Resource) -> Self {
        Self {
            resource_id: kind.resource_id(&resource.id),
            kind,
            display_name: resource.name.clone(),
            account_id: account.id.clone(),
            collection_id: resource.id.clone(),
            is_default: resource.is_default,
            color: resource.color.clone(),
            // Narrows, never widens. Absent `myRights` (`writable` is `None`)
            // gives the account-wide bit.
            read_only: account.read_only || resource.writable == Some(false),
            // Absent `mayDelete` (`deletable` is `None`) gives deletable.
            remote_deletable: resource.deletable.unwrap_or(true),
        }
    }
}

impl Child {
    /// The child source for one collection of the shared `account`.
    ///
    /// The display name has the owner, `<collection> (<account name>)`, so the
    /// user does not confuse a shared "Calendar" with the user's own. Rights
    /// narrow as for the user's own collections. Evolution offers deletion
    /// only when `myRights.mayDelete` is true. A deletion removes the
    /// collection of the owner for all users. Thus, when the server gives no
    /// rights, the permissive default of the user's own collections does not
    /// apply.
    pub fn for_shared_resource(
        kind: ChildKind,
        account: &ServiceAccount,
        resource: &Resource,
    ) -> Self {
        Self {
            resource_id: kind.shared_resource_id(&account.id, &resource.id),
            display_name: format!("{} ({})", resource.name, account.name),
            remote_deletable: resource.deletable == Some(true),
            ..Self::for_resource(kind, account, resource)
        }
    }

    /// The `[Resource] Identity` of this child: the resource id without its
    /// kind. This is the bare collection id for the user's own collections,
    /// and `<accountId>/<collectionId>` for a shared one.
    pub fn identity(&self) -> &str {
        self.resource_id
            .split_once(':')
            .map_or(self.collection_id.as_str(), |(_, identity)| identity)
    }
}

/// Reads `resource_id` back into the kind and the identity of its child.
///
/// The identity is [`Child::identity`]: for a shared child, it is
/// `<accountId>/<collectionId>`. `None` is a resource id that this backend did
/// not write, for example from a different collection backend or a future
/// version of this one. [`Fanout::is_obsolete`] keeps such a child.
pub fn parse_resource_id(resource_id: &str) -> Option<(ChildKind, Id)> {
    let (prefix, collection) = resource_id.split_once(':')?;
    let kind = match prefix {
        "addressbook" => ChildKind::AddressBook,
        "calendar" => ChildKind::Calendar,
        _ => return None,
    };
    if collection.is_empty() {
        return None;
    }
    Some((kind, Id::new(collection)))
}

/// The children of one kind, all in `account`.
fn children<'a>(
    kind: ChildKind,
    account: &'a ServiceAccount,
    resources: &'a [Resource],
) -> impl Iterator<Item = Child> + 'a {
    resources
        .iter()
        .map(move |resource| Child::for_resource(kind, account, resource))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::layout::{CollectionLayout, MailService};
    use crate::parts::Parts;
    use crate::resources::SharedListing;

    fn account(id: &str) -> ServiceAccount {
        ServiceAccount {
            id: Id::new(id),
            name: "Vera Vibes".to_owned(),
            read_only: false,
        }
    }

    fn resource(id: &str, name: &str) -> Resource {
        Resource {
            id: Id::new(id),
            name: name.to_owned(),
            is_default: false,
            color: None,
            writable: None,
            deletable: None,
        }
    }

    /// A fan-out with one account for both collection kinds.
    fn fanout(address_books: Vec<Resource>, calendars: Vec<Resource>) -> Fanout {
        Fanout {
            shared: Vec::new(),
            parts: Parts::ALL,
            layout: CollectionLayout {
                mail: None,
                contacts: Some(account("A1")),
                calendars: Some(account("A1")),
            },
            address_books,
            calendars,
        }
    }

    fn resource_ids(fanout: &Fanout) -> Vec<String> {
        fanout
            .children()
            .into_iter()
            .map(|child| child.resource_id)
            .collect()
    }

    #[test]
    fn every_collection_is_a_child_named_after_its_kind_and_its_id() {
        let fanout = fanout(
            vec![resource("AB1", "Personal")],
            vec![resource("Cal1", "Work")],
        );

        assert_eq!(resource_ids(&fanout), ["addressbook:AB1", "calendar:Cal1"]);
    }

    #[test]
    fn an_address_book_and_a_calendar_with_one_id_are_two_children() {
        // Ids are unique per account *and per object type* (RFC 8620 §1.2).
        // A server that numbers its objects from one gives this result. The
        // resource id namespace is flat, so a bare id loses one source.
        let fanout = fanout(vec![resource("a", "Personal")], vec![resource("a", "Work")]);

        let children = fanout.children();
        assert_eq!(children.len(), 2);
        assert_ne!(children[0].resource_id, children[1].resource_id);
        assert_eq!(children[0].collection_id, children[1].collection_id);
    }

    #[test]
    fn a_resource_id_reads_back_as_the_child_it_names() {
        // `Fanout::is_obsolete` reads cached resource ids back. A wrong read
        // can remove a child that exists, or keep a child that is gone.
        for (kind, id) in [
            (ChildKind::AddressBook, "AB1"),
            (ChildKind::Calendar, "Cal1"),
            // Not in the id charset, but the rest after the first colon stays
            // whole, so the source does not point at a different collection.
            (ChildKind::Calendar, "odd:id"),
        ] {
            let resource_id = kind.resource_id(&Id::new(id));
            assert_eq!(
                parse_resource_id(&resource_id),
                Some((kind, Id::new(id))),
                "{resource_id} did not survive the round trip"
            );
        }
    }

    #[test]
    fn a_resource_id_this_backend_did_not_write_is_not_read_as_one() {
        // A child that this backend did not create belongs to a different
        // backend, and `Fanout::is_obsolete` must keep it.
        for foreign in ["AB1", "", ":", "addressbook", "addressbook:", "task:1"] {
            assert_eq!(
                parse_resource_id(foreign),
                None,
                "{foreign:?} was read as a child of ours"
            );
        }
    }

    #[test]
    fn a_collection_listed_twice_is_one_child() {
        // Two children with one resource id are not two sources: the second
        // `new_child` call resolves to the source of the first.
        let fanout = fanout(
            vec![resource("AB1", "Personal"), resource("AB1", "Personal too")],
            Vec::new(),
        );

        let children = fanout.children();
        assert_eq!(resource_ids(&fanout), ["addressbook:AB1"]);
        assert_eq!(
            children[0].display_name, "Personal",
            "the first listing wins, so the child list does not depend on \
             which duplicate the server sent last"
        );
    }

    #[test]
    fn each_child_carries_the_account_that_serves_its_kind() {
        // `CollectionLayout` resolves contacts and calendars separately. A
        // child with the wrong `accountId` fails each call.
        let fanout = Fanout {
            shared: Vec::new(),
            parts: Parts::ALL,
            layout: CollectionLayout {
                mail: None,
                contacts: Some(account("A1")),
                calendars: Some(account("A2")),
            },
            address_books: vec![resource("AB1", "Personal")],
            calendars: vec![resource("Cal1", "Work")],
        };

        let children = fanout.children();
        assert_eq!(children[0].account_id, Id::new("A1"));
        assert_eq!(children[1].account_id, Id::new("A2"));
    }

    #[test]
    fn a_read_only_account_makes_read_only_children_and_only_its_own() {
        let mut contacts = account("A1");
        contacts.read_only = true;
        let fanout = Fanout {
            shared: Vec::new(),
            parts: Parts::ALL,
            layout: CollectionLayout {
                mail: None,
                contacts: Some(contacts),
                calendars: Some(account("A2")),
            },
            address_books: vec![resource("AB1", "Personal")],
            calendars: vec![resource("Cal1", "Work")],
        };

        let children = fanout.children();
        assert!(children[0].read_only);
        assert!(
            !children[1].read_only,
            "the read-only account is the contacts one; the calendar is in \
             another account and says nothing about it"
        );
    }

    #[test]
    fn a_collections_own_my_rights_narrows_a_writable_account_to_read_only() {
        // A writable account with one collection whose `myRights` says not
        // writable gives a read-only child for only that collection.
        let mut locked = resource("AB1", "Locked share");
        locked.writable = Some(false);
        let fanout = fanout(vec![locked, resource("AB2", "Personal")], Vec::new());

        let children = fanout.children();
        assert!(children[0].read_only, "myRights said not writable");
        assert!(
            !children[1].read_only,
            "absent myRights falls back to the writable account, unchanged"
        );
    }

    #[test]
    fn a_collections_own_my_rights_never_widens_a_read_only_account() {
        // A collection whose `myRights` says writable does not override a
        // read-only account: the account bit is the upper limit.
        let mut read_only_account = account("A1");
        read_only_account.read_only = true;
        let mut writable_looking = resource("AB1", "Says writable");
        writable_looking.writable = Some(true);
        let fanout = Fanout {
            shared: Vec::new(),
            parts: Parts::ALL,
            layout: CollectionLayout {
                mail: None,
                contacts: Some(read_only_account),
                calendars: None,
            },
            address_books: vec![writable_looking],
            calendars: Vec::new(),
        };

        assert!(fanout.children()[0].read_only);
    }

    #[test]
    fn a_collections_own_my_rights_decides_whether_it_is_remote_deletable() {
        // `myRights.mayDelete` is per collection, with no account-wide bit.
        // `false` and `true` stay unchanged, and an absent value gives deletable.
        let mut undeletable = resource("AB1", "Locked share");
        undeletable.deletable = Some(false);
        let fanout = fanout(vec![undeletable, resource("AB2", "Personal")], Vec::new());

        let children = fanout.children();
        assert!(!children[0].remote_deletable, "myRights said not deletable");
        assert!(
            children[1].remote_deletable,
            "absent myRights stays deletable"
        );
    }

    #[test]
    fn the_default_collection_and_the_name_reach_the_child() {
        let mut default = resource("AB1", "Personal");
        default.is_default = true;
        let fanout = fanout(vec![default, resource("AB2", "Shared")], Vec::new());

        let children = fanout.children();
        assert!(children[0].is_default);
        assert_eq!(children[0].display_name, "Personal");
        assert!(!children[1].is_default);
    }

    #[test]
    fn a_calendars_color_reaches_its_child() {
        let mut colored = resource("Cal1", "Work");
        colored.color = Some("#ff8800".to_owned());
        let fanout = fanout(Vec::new(), vec![colored, resource("Cal2", "Home")]);

        let children = fanout.children();
        assert_eq!(children[0].color, Some("#ff8800".to_owned()));
        assert_eq!(children[1].color, None);
    }

    #[test]
    fn a_login_whose_collections_were_never_listed_has_no_children_of_that_kind() {
        // With no contacts account in the layout, a listed address book is
        // not a child. `Fanout::discover` never makes this fan-out.
        let fanout = Fanout {
            shared: Vec::new(),
            parts: Parts::ALL,
            layout: CollectionLayout {
                mail: None,
                contacts: None,
                calendars: Some(account("A1")),
            },
            address_books: vec![resource("AB1", "Cannot happen, but not a child")],
            calendars: Vec::new(),
        };

        assert!(fanout.children().is_empty());
    }

    #[test]
    fn the_mail_account_is_not_one_of_these_children() {
        // Deliberate: the account setup writes the mail sources outside the
        // backend cache (see the module comment).
        let fanout = Fanout {
            shared: Vec::new(),
            parts: Parts::ALL,
            layout: CollectionLayout {
                mail: Some(MailService {
                    account: account("A1"),
                    can_send: true,
                }),
                contacts: None,
                calendars: None,
            },
            address_books: Vec::new(),
            calendars: Vec::new(),
        };

        assert!(fanout.children().is_empty());
    }

    fn shared(
        id: &str,
        books: Option<Vec<Resource>>,
        calendars: Option<Vec<Resource>>,
    ) -> SharedListing {
        SharedListing {
            account: ServiceAccount {
                id: Id::new(id),
                name: "bob@example.com".to_owned(),
                read_only: false,
            },
            address_books: books,
            calendars,
        }
    }

    #[test]
    fn a_shared_collection_becomes_a_child_named_with_its_owner_after_the_users_own() {
        let mut locked = resource("C1", "Team");
        locked.writable = Some(false);
        let mut fanout = fanout(vec![resource("AB1", "Personal")], vec![]);
        fanout.shared = vec![shared(
            "A2",
            Some(vec![resource("AB1", "Contacts")]),
            Some(vec![locked]),
        )];

        let children = fanout.children();
        assert_eq!(
            children
                .iter()
                .map(|child| (child.resource_id.as_str(), child.display_name.as_str()))
                .collect::<Vec<_>>(),
            [
                ("addressbook:AB1", "Personal"),
                ("addressbook:A2/AB1", "Contacts (bob@example.com)"),
                ("calendar:A2/C1", "Team (bob@example.com)"),
            ],
            "the owner's AB1 is not the user's AB1: ids are per account"
        );

        let calendar = &children[2];
        assert_eq!(calendar.account_id, Id::new("A2"));
        assert_eq!(calendar.collection_id, Id::new("C1"));
        assert_eq!(calendar.identity(), "A2/C1");
        assert!(calendar.read_only, "myRights says it may not be written");
        assert_eq!(children[0].identity(), "AB1");
    }

    #[test]
    fn a_shared_collection_is_remote_deletable_only_when_my_rights_grants_it() {
        let with = |id, deletable| {
            let mut calendar = resource(id, "Team");
            calendar.deletable = deletable;
            calendar
        };
        let mut fanout = fanout(vec![], vec![]);
        fanout.shared = vec![shared(
            "A2",
            None,
            Some(vec![
                with("C1", Some(true)),
                with("C2", Some(false)),
                with("C3", None),
            ]),
        )];

        assert_eq!(
            fanout
                .children()
                .iter()
                .map(|child| (child.resource_id.as_str(), child.remote_deletable))
                .collect::<Vec<_>>(),
            [
                ("calendar:A2/C1", true),
                ("calendar:A2/C2", false),
                // Unlike the user's own, absent rights do not offer the
                // owner's collection for deletion.
                ("calendar:A2/C3", false),
            ]
        );
    }

    #[test]
    fn a_shared_kind_the_user_switched_off_or_that_was_not_listed_has_no_children() {
        let mut fanout = fanout(vec![], vec![]);
        fanout.parts = Parts {
            contacts: false,
            ..Parts::ALL
        };
        fanout.shared = vec![
            shared("A2", Some(vec![resource("AB1", "Contacts")]), None),
            shared("A3", None, Some(vec![resource("C1", "Team")])),
        ];

        assert_eq!(
            fanout
                .children()
                .iter()
                .map(|child| child.resource_id.as_str())
                .collect::<Vec<_>>(),
            ["calendar:A3/C1"]
        );
    }

    #[test]
    fn a_shared_identity_is_split_only_when_both_halves_are_there() {
        assert_eq!(split_shared_identity("A2/C1"), Some(("A2", "C1")));
        for whole in ["C1", "/C1", "A2/", "A/B/C", ""] {
            assert_eq!(split_shared_identity(whole), None, "{whole:?}");
        }
    }
}
