// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The child sources a populate creates, and the name each is created under.
//!
//! [`Fanout`] answers what a login holds. This answers what `populate` does
//! with it: `e_collection_backend_new_child (backend, resource_id)` takes a
//! *resource id* and hands back the child `ESource` — so a backend does not
//! invent child uids, it names resources and EDS assigns and remembers the
//! uids. The same string has to come back out of the `dup_resource_id` vfunc
//! for that child, because that pairing is how EDS recognises, on the next
//! populate, that this resource already has a source and must not get a second
//! one.
//!
//! ## Why a resource id is not just the JMAP id
//!
//! Ids in JMAP are scoped to an account *and an object type* (RFC 8620 §1.2):
//! nothing stops a server from having an `AddressBook` with id `a` and a
//! `Calendar` with id `a`, and on a server that numbers its objects from one
//! that is the expected case rather than a corner one. The resource id
//! namespace, though, is flat — it is every child of this one collection —
//! so handing EDS the bare JMAP id would have the calendar named `a` resolve
//! to the address book's child source, and the account would come up one
//! source short with a calendar's data in a contacts backend.
//!
//! So the kind is part of the name: `addressbook:<id>` and `calendar:<id>`.
//! The separator cannot be ambiguous either, which is why parsing splits at the
//! *first* colon and keeps the rest whole — the id charset in RFC 8620 §1.2 has
//! no colon in it, but nothing here is in a position to insist on that, and a
//! server that sends one should get a wrong-looking source rather than a
//! silently mismatched one.
//!
//! ## What is deliberately not here
//!
//! **The mail children.** Whether the mail account, identity and transport
//! sources are created by this backend's populate or by the account-setup
//! module (M7), and which of the `[Mail Account]`/`[Mail Identity]`/
//! `[Mail Submission]`/`[Mail Transport]` extensions sits on which of them, is
//! Evolution convention rather than anything the installed headers state, and
//! this machine has no reference account to read it off. Guessing it here would
//! put a shape into the child list that the tests could only confirm was the
//! shape we guessed. [`Fanout::layout`] carries the mail service either way;
//! the child list stops where the certainty stops.

use jmap_proto::Id;

use crate::layout::ServiceAccount;
use crate::resources::{Fanout, Resource};

/// Which of Evolution's backends a child source is for.
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

    /// The name `e_collection_backend_new_child` is called with for
    /// `collection`, and the string `dup_resource_id` has to return for the
    /// child it produced.
    pub fn resource_id(self, collection: &Id) -> String {
        format!("{}:{}", self.prefix(), collection)
    }
}

/// One `ESource` a populate creates, with everything its keyfile needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    /// What `e_collection_backend_new_child` is called with, and what
    /// `dup_resource_id` must give back for this child. Unique across the whole
    /// child list, which the bare JMAP id would not be.
    pub resource_id: String,
    pub kind: ChildKind,
    /// `ESource:display-name` — the collection's name, as the server states it.
    pub display_name: String,
    /// The `accountId` this child's method calls carry. Not a constant of the
    /// collection: contacts and calendars can resolve to different JMAP
    /// accounts, and each child talks to its own.
    pub account_id: Id,
    /// The collection this child is for, which is what `[Resource] Identity`
    /// carries.
    pub collection_id: Id,
    /// The account's default collection of this kind.
    pub is_default: bool,
    /// The *account* is read-only, so nothing in this child may be created,
    /// changed or removed. Per-collection rights (`myRights` on the collection
    /// object) are a second, finer question that is not read yet, so a
    /// writable-account child is not thereby known to be writable.
    pub read_only: bool,
}

impl Fanout {
    /// The children this login warrants, in the order they are created in.
    ///
    /// Address books first, then calendars, each in the order
    /// [`Fanout::discover`] put them in — which is the server's own sort order.
    /// A collection the server listed twice becomes one child: two children
    /// with one resource id is not two sources, it is one source created and
    /// then overwritten.
    pub fn children(&self) -> Vec<Child> {
        let books = self
            .layout
            .contacts
            .iter()
            .flat_map(|account| children(ChildKind::AddressBook, account, &self.address_books));
        let calendars = self
            .layout
            .calendars
            .iter()
            .flat_map(|account| children(ChildKind::Calendar, account, &self.calendars));

        let mut seen: Vec<String> = Vec::new();
        books
            .chain(calendars)
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

/// Reads `resource_id` back into the child it names.
///
/// `None` is a resource id this backend did not write — a child of some other
/// collection backend, or one written by a future version of this one — and the
/// `dup_resource_id` vfunc turns that into the `NULL` that tells EDS it does not
/// know this child.
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

/// One kind's worth of children, all in `account`.
fn children<'a>(
    kind: ChildKind,
    account: &'a ServiceAccount,
    resources: &'a [Resource],
) -> impl Iterator<Item = Child> + 'a {
    resources.iter().map(move |resource| Child {
        resource_id: kind.resource_id(&resource.id),
        kind,
        display_name: resource.name.clone(),
        account_id: account.id.clone(),
        collection_id: resource.id.clone(),
        is_default: resource.is_default,
        read_only: account.read_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::layout::{CollectionLayout, MailService};

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
        }
    }

    /// A fan-out of one account serving both collection kinds.
    fn fanout(address_books: Vec<Resource>, calendars: Vec<Resource>) -> Fanout {
        Fanout {
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
        // Ids are scoped per account *and per object type* (RFC 8620 §1.2), so
        // this is not a broken server, it is what a server that numbers its
        // objects from one looks like. The resource id namespace is flat, so
        // the bare id would have the calendar resolve to the address book's
        // child and the account come up one source short.
        let fanout = fanout(vec![resource("a", "Personal")], vec![resource("a", "Work")]);

        let children = fanout.children();
        assert_eq!(children.len(), 2);
        assert_ne!(children[0].resource_id, children[1].resource_id);
        assert_eq!(children[0].collection_id, children[1].collection_id);
    }

    #[test]
    fn a_resource_id_reads_back_as_the_child_it_names() {
        // The round trip is not a nicety: `dup_resource_id` is asked for this
        // string again on the next populate, and a child whose id does not come
        // back is a child EDS creates a second source for.
        for (kind, id) in [
            (ChildKind::AddressBook, "AB1"),
            (ChildKind::Calendar, "Cal1"),
            // Out of spec — the id charset has no colon in it — but a wrong
            // looking source beats one silently pointed at another collection.
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
        // `dup_resource_id` is called for children this backend may never have
        // created; claiming one is claiming a source that belongs elsewhere.
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
        // Two children under one resource id are not two sources: the second
        // `new_child` call resolves to the first one's source.
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
        // Contacts and calendars resolve independently in `CollectionLayout`,
        // and a child that carries the wrong `accountId` fails every call.
        let fanout = Fanout {
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
    fn a_login_whose_collections_were_never_listed_has_no_children_of_that_kind() {
        // `Fanout::discover` leaves the vector empty both when the login offers
        // no contacts and when the account holds no address book; neither is an
        // error and neither is a child.
        let fanout = Fanout {
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
        // Not an oversight: which sources the mail service becomes, and which
        // of the four mail extensions sits on which of them, is Evolution
        // convention that the installed headers do not state and this machine
        // cannot check. The module comment says so; this says so where it would
        // be noticed if someone added mail children without settling it.
        let fanout = Fanout {
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
}
