// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The address books and calendars inside the accounts the layout resolved.
//!
//! [`CollectionLayout`] answers *which account* serves contacts and calendars;
//! it does not answer how many address books and how many calendars are in it,
//! and that is a second question with a second answer. JMAP puts contact cards
//! in `AddressBook`s and events in `Calendar`s (RFC 9610 §2,
//! draft-ietf-jmap-calendars §4), several of each per account, and Evolution
//! shows one source per address book and one per calendar — so the fan-out is
//! not three children, it is one mail account plus however many collections the
//! server lists.
//!
//! ## Only what the layout resolved, and only what the user asked for
//!
//! A listing is sent for a capability only when the layout found an account for
//! it. The saving is not the round trip: RFC 8620 §3.3 has a server answer a
//! `using` naming a capability it does not advertise with `unknownCapability`,
//! and that error fails the *whole request* rather than the one call. Asking a
//! contacts-less server for its address books anyway would therefore be a
//! request that comes back with nothing in it at all.
//!
//! The second gate is the user's: a part switched off on the account is not
//! listed either — see [`Parts`], which also has what a fan-out may and may not
//! conclude from a listing it never sent.
//!
//! ## What is left out, and why that is a decision
//!
//! - **`isSubscribed == Some(false)` is dropped.** Both objects carry it and
//!   both mean the same thing by it: this collection exists, and the user has
//!   said they do not want it. Creating a source for it puts a calendar the
//!   user removed back in their sidebar at every populate.
//! - **An absent `isSubscribed` is a subscription.** The property is optional
//!   in both specifications, and a server that says nothing has not said no —
//!   reading silence as "unsubscribed" would empty the sidebar of every server
//!   that omits it.
//! - **A collection with no `id` is dropped.** `[Resource] Identity` is how a
//!   child source names the collection it is for; without an id there is
//!   nothing to write there, and the child would be a source pointing at
//!   whatever the account's default happened to be.

use jmap_client::Client;
use jmap_proto::Id;
use jmap_proto::calendars::{Calendar, CalendarRights};
use jmap_proto::contacts::{AddressBook, AddressBookRights};

use crate::layout::CollectionLayout;
use crate::parts::Parts;

/// One address book or calendar, as a child source is made from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// The JMAP id, which is what `[Resource] Identity` carries and what every
    /// method call for this collection filters on.
    pub id: Id,
    /// The name to show. Never empty: a collection the server named nothing
    /// falls back to its id, because a blank row in Evolution's sidebar is one
    /// the user cannot tell from another blank row.
    pub name: String,
    /// The account's default collection for its kind — where a card or an event
    /// created without a collection lands.
    pub is_default: bool,
    /// `Calendar.color`, threaded straight through. `None` for an address
    /// book (JMAP defines no such property on `AddressBook`) and for a
    /// calendar the server named none.
    pub color: Option<String>,
    /// Whether `myRights` says this collection may be written to
    /// (`AddressBookRights`/`CalendarRights::is_writable`). `None` when the
    /// server sent no `myRights` at all, which is what keeps
    /// [`crate::children::Child::for_resource`] on the account-wide
    /// `read_only` fallback exactly as before this field existed.
    pub writable: Option<bool>,
}

/// Everything one JMAP login fans out into.
///
/// The [`CollectionLayout`] is the question "which account", the two vectors
/// are the question "which collections in it", and together they are the whole
/// child list a populate has to produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fanout {
    /// The parts the discovery was run under, kept so that what may be
    /// concluded from the vectors cannot drift from what was asked — see
    /// [`Fanout::listed`].
    pub parts: Parts,
    pub layout: CollectionLayout,
    /// In the order the server sorted them, which is the order the children are
    /// created in. Empty when the login offers no contacts *or* when the
    /// account holds no address book — a fresh account is not an error.
    pub address_books: Vec<Resource>,
    pub calendars: Vec<Resource>,
}

impl Fanout {
    /// Reads the session document, then lists the collections of whichever
    /// accounts it resolved — for the parts the account has switched on.
    ///
    /// A part that is off costs no request, so the vector for it comes back
    /// empty whether or not the server holds anything; [`Fanout::listed`] is
    /// how a caller tells that emptiness from an account that really holds no
    /// address book.
    ///
    /// The error is `jmap_client`'s own rather than a wrapper: everything that
    /// can fail here is one of its calls, and the GObject layer already maps
    /// that type onto Evolution's error codes.
    pub fn discover(client: &Client, parts: Parts) -> Result<Self, jmap_client::Error> {
        let layout = CollectionLayout::from_session(client.session());

        let address_books = match layout.contacts.as_ref().filter(|_| parts.contacts) {
            Some(account) => resources(client.address_books(&account.id)?),
            None => Vec::new(),
        };
        let calendars = match layout.calendars.as_ref().filter(|_| parts.calendars) {
            Some(account) => resources(client.calendars(&account.id)?),
            None => Vec::new(),
        };

        Ok(Self {
            parts,
            layout,
            address_books,
            calendars,
        })
    }
}

/// The name to show for a collection the server called `name`, which is never
/// blank — see [`Resource::name`] for why.
///
/// Public because a `create_resource_sync` has to apply the same rule to the
/// collection it has just made as a listing applies to the ones it finds: the
/// server answers a create with the object it created, and a server that
/// normalised the requested name to nothing would otherwise produce the one row
/// in Evolution's sidebar with no text in it.
pub fn shown_name(name: &str, id: &Id) -> String {
    match name.trim() {
        "" => id.to_string(),
        name => name.to_owned(),
    }
}

/// The child-worthy collections of a `/get` listing, in the order the server
/// asked for them to be shown.
fn resources<C: Collection>(listing: Vec<C>) -> Vec<Resource> {
    let mut resources: Vec<(u32, Resource)> = listing
        .into_iter()
        .filter(|collection| collection.is_subscribed() != Some(false))
        .filter_map(|collection| {
            let id = collection.id()?.clone();
            let name = shown_name(collection.name(), &id);
            Some((
                collection.sort_order(),
                Resource {
                    id,
                    name,
                    is_default: collection.is_default() == Some(true),
                    color: collection.color(),
                    writable: collection.writable(),
                },
            ))
        })
        .collect();

    // `sortOrder` first, then the name, which is what a JMAP client is told to
    // do with the equal ones (RFC 8621 §2 says it of `Mailbox` and both
    // collection specifications copy the property from it). The id is the last
    // tie-break, and it is there only so that two identically named collections
    // come back in the same order every populate — a child list that reshuffles
    // between runs is one Evolution recreates sources for.
    resources.sort_by(|(left_order, left), (right_order, right)| {
        left_order
            .cmp(right_order)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });

    resources
        .into_iter()
        .map(|(_, resource)| resource)
        .collect()
}

/// The properties an `AddressBook` and a `Calendar` share.
///
/// The two objects are defined in different documents and neither derives from
/// the other, but the four properties a child source is made from are spelled
/// identically in both — so the selection is written once rather than twice,
/// and a rule fixed for address books cannot silently not apply to calendars.
trait Collection {
    fn id(&self) -> Option<&Id>;
    fn name(&self) -> &str;
    /// Defaulted to 0, as both specifications define the property's absence.
    fn sort_order(&self) -> u32;
    fn is_default(&self) -> Option<bool>;
    fn is_subscribed(&self) -> Option<bool>;
    /// `Calendar.color`. `None` for every `AddressBook`, which has no such
    /// property to begin with.
    fn color(&self) -> Option<String>;
    /// `myRights.is_writable()`, or `None` when the server sent no `myRights`
    /// for this collection at all — the case [`Resource::writable`]'s own doc
    /// says falls back to the account-wide bit unchanged.
    fn writable(&self) -> Option<bool>;
}

impl Collection for AddressBook {
    fn id(&self) -> Option<&Id> {
        self.id.as_ref()
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn sort_order(&self) -> u32 {
        self.sort_order.unwrap_or(0)
    }
    fn is_default(&self) -> Option<bool> {
        self.is_default
    }
    fn is_subscribed(&self) -> Option<bool> {
        self.is_subscribed
    }
    fn color(&self) -> Option<String> {
        None
    }
    fn writable(&self) -> Option<bool> {
        self.my_rights.as_ref().map(AddressBookRights::is_writable)
    }
}

impl Collection for Calendar {
    fn id(&self) -> Option<&Id> {
        self.id.as_ref()
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn sort_order(&self) -> u32 {
        self.sort_order.unwrap_or(0)
    }
    fn is_default(&self) -> Option<bool> {
        self.is_default
    }
    fn is_subscribed(&self) -> Option<bool> {
        self.is_subscribed
    }
    fn color(&self) -> Option<String> {
        self.color.clone()
    }
    fn writable(&self) -> Option<bool> {
        self.my_rights.as_ref().map(CalendarRights::is_writable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(id: Option<&str>, name: &str) -> AddressBook {
        AddressBook {
            id: id.map(Id::new),
            name: name.to_owned(),
            ..AddressBook::default()
        }
    }

    fn names(listing: Vec<AddressBook>) -> Vec<String> {
        resources(listing)
            .into_iter()
            .map(|resource| resource.name)
            .collect()
    }

    #[test]
    fn a_collection_saying_nothing_about_subscription_is_kept() {
        // Both specifications make `isSubscribed` optional, so silence is the
        // shape of a plain server, not of a collection the user hid. Reading it
        // as "no" would empty the sidebar of every server that omits it.
        assert_eq!(names(vec![book(Some("AB1"), "Personal")]), ["Personal"]);
    }

    #[test]
    fn a_collection_with_no_id_is_dropped_rather_than_pointed_somewhere() {
        // Without an id there is nothing to write in `[Resource] Identity`, and
        // a child source with no identity is one the backend resolves to the
        // account's *default* collection — so keeping it would silently show
        // the wrong address book under the right name.
        let listing = vec![book(None, "Nameless"), book(Some("AB2"), "Personal")];
        assert_eq!(names(listing), ["Personal"]);
    }

    #[test]
    fn a_collection_the_server_named_nothing_is_shown_under_its_id() {
        // `name` is required by both specifications, so this is a server out of
        // spec — but the alternative to a fallback is a blank row in Evolution's
        // sidebar, which the user cannot tell from another blank row.
        let resources = resources(vec![book(Some("AB7"), "   ")]);
        assert_eq!(resources[0].name, "AB7");
    }

    #[test]
    fn equal_sort_orders_are_broken_by_name_and_then_by_id() {
        let mut same_name_late = book(Some("AB9"), "Shared");
        same_name_late.sort_order = Some(5);
        let mut same_name_early = book(Some("AB1"), "Shared");
        same_name_early.sort_order = Some(5);
        let mut first = book(Some("AB5"), "Personal");
        first.sort_order = Some(5);

        let resources = resources(vec![same_name_late, first, same_name_early]);
        assert_eq!(
            resources
                .iter()
                .map(|resource| resource.id.as_str())
                .collect::<Vec<_>>(),
            ["AB5", "AB1", "AB9"],
            "the same listing has to produce the same child order every \
             populate, or Evolution is handed a reshuffled account"
        );
    }

    #[test]
    fn the_accounts_default_collection_is_reported_as_one() {
        let mut default = book(Some("AB1"), "Personal");
        default.is_default = Some(true);
        let plain = book(Some("AB2"), "Shared");

        let resources = resources(vec![default, plain]);
        assert!(resources[0].is_default);
        assert!(
            !resources[1].is_default,
            "an absent isDefault is not the default"
        );
    }

    #[test]
    fn my_rights_absent_leaves_writable_none_and_present_is_read_through() {
        // Absent `myRights` is the case `Resource::writable`'s doc points at:
        // `None` here is what keeps `Child::for_resource` on the account-wide
        // fallback rather than concluding anything about this one collection.
        assert_eq!(
            resources(vec![book(Some("AB1"), "Personal")])[0].writable,
            None
        );

        let mut locked = book(Some("AB2"), "Read-only share");
        locked.my_rights = Some(AddressBookRights {
            may_write: Some(false),
            ..AddressBookRights::default()
        });
        assert_eq!(resources(vec![locked])[0].writable, Some(false));

        let calendar_own_only = Calendar {
            id: Some(Id::new("Cal1")),
            name: "Work".to_owned(),
            my_rights: Some(CalendarRights {
                may_write_all: Some(false),
                may_write_own: Some(true),
                ..CalendarRights::default()
            }),
            ..Calendar::default()
        };
        assert_eq!(resources(vec![calendar_own_only])[0].writable, Some(true));
    }

    #[test]
    fn a_calendars_color_is_carried_and_an_address_books_never_is() {
        // `AddressBook` has no `color` property to begin with, so its
        // `Resource` is always `None`; a `Calendar`'s is whatever the server
        // named, verbatim.
        let calendar = Calendar {
            id: Some(Id::new("Cal1")),
            name: "Work".to_owned(),
            color: Some("#ff8800".to_owned()),
            ..Calendar::default()
        };
        assert_eq!(
            resources(vec![calendar])[0].color,
            Some("#ff8800".to_owned())
        );

        assert_eq!(
            resources(vec![book(Some("AB1"), "Personal")])[0].color,
            None
        );
    }

    #[test]
    fn a_calendar_the_server_named_no_color_for_carries_none() {
        let calendar = Calendar {
            id: Some(Id::new("Cal1")),
            name: "Work".to_owned(),
            ..Calendar::default()
        };
        assert_eq!(resources(vec![calendar])[0].color, None);
    }
}
