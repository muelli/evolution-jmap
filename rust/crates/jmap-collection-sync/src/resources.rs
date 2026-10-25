// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The address books and calendars in the accounts that the layout resolved.
//!
//! [`CollectionLayout`] tells *which account* serves contacts and calendars. It
//! does not tell how many address books and calendars the account holds. JMAP
//! keeps contact cards in `AddressBook`s and events in `Calendar`s (RFC 9610
//! §2, draft-ietf-jmap-calendars §4), with many of each in one account.
//! Evolution shows one source for each address book and each calendar. Thus
//! the fan-out is not three children: it is one child for each collection that
//! the server lists.
//!
//! ## Only what the layout resolved, and only what the user switched on
//!
//! A listing for a capability is sent only when the layout found an account
//! for it. This is not only to save a round trip: RFC 8620 §3.3 makes a server
//! answer a `using` with a capability that it does not advertise with
//! `unknownCapability`. That error fails the *whole request*, not only one
//! call, so the request returns no data.
//!
//! The user's choice is the second gate: a part that the user switched off is
//! not listed. [`Parts`] also tells what a fan-out can conclude from a listing
//! that it did not send.
//!
//! ## What is not shown, and why
//!
//! - **A collection with `isSubscribed == Some(false)`.** Both objects have
//!   this property, with the same meaning: the collection exists, and the user
//!   does not want it. A source for it puts a collection that the user removed
//!   back in the sidebar at each populate.
//! - **But a shared collection is shown, whatever `isSubscribed` says.** The
//!   calendars draft (-33 §4) makes a share start unsubscribed. Only a client of
//!   the sharee can subscribe, and Evolution has no control for it. Stalwart
//!   refuses the subscription (`forbidden`) to a sharee without write rights,
//!   which the draft allows (§4.3, "The server MAY forbid users from
//!   subscribing"). If this module used the flag, each read-only share would
//!   stay hidden.
//! - **An absent `isSubscribed` is a subscription.** The property is optional
//!   in both specifications. If absence meant "unsubscribed", the sidebar would
//!   be empty for each server that omits it.
//! - **A collection with no `id` is not shown.** A child source names its
//!   collection in `[Resource] Identity`. Without an id, there is nothing to
//!   write there, and the child would use the account's default collection.

use jmap_client::Client;
use jmap_proto::Id;
use jmap_proto::calendars::{Calendar, CalendarRights};
use jmap_proto::contacts::{AddressBook, AddressBookRights};

use crate::children::ChildKind;
use crate::layout::{CollectionLayout, ServiceAccount};
use crate::parts::Parts;

/// One address book or calendar, as a child source is made from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// The JMAP id. `[Resource] Identity` holds it (after the account id, for a
    /// shared collection), and each method call for this collection uses it.
    pub id: Id,
    /// The name to show. It is never empty: if the server gives no name, the
    /// id is used, because the user cannot tell two blank rows in the sidebar
    /// apart.
    pub name: String,
    /// Whether this is the account's default collection for its kind: the
    /// collection for a card or an event that is created without a collection.
    pub is_default: bool,
    /// `Calendar.color`, unchanged. `None` for an address book (JMAP defines no
    /// such property on `AddressBook`) and for a calendar without a color.
    pub color: Option<String>,
    /// Whether `myRights` permits writes to this collection
    /// (`AddressBookRights`/`CalendarRights::is_writable`). `None` when the
    /// server sent no `myRights`. Then [`crate::children::Child::for_resource`]
    /// uses only the account-wide `read_only`.
    pub writable: Option<bool>,
    /// `myRights.mayDelete` (`AddressBookRights`/`CalendarRights::may_delete`),
    /// unchanged. `None` when the server sent no `myRights`, or sent one without
    /// `mayDelete`. [`crate::children::Child::for_resource`] reads `None` as
    /// "deletable", which is also the EDS default for a child.
    pub deletable: Option<bool>,
}

/// Everything one JMAP login fans out into.
///
/// The [`CollectionLayout`] answers "which account". The vectors answer "which
/// collections are in it". Together they give all the children of a populate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fanout {
    /// The parts that the discovery used. [`Fanout::listed`] uses them, so its
    /// conclusions agree with what was asked.
    pub parts: Parts,
    pub layout: CollectionLayout,
    /// Sorted by `sortOrder`, then name, then id. The children are created in
    /// this order. Empty when the login offers no contacts, when the contacts
    /// part is off, *or* when the account holds no address book. A new account
    /// is not an error.
    pub address_books: Vec<Resource>,
    pub calendars: Vec<Resource>,
    /// The accounts shared with this login that offer contacts or calendars
    /// ([`CollectionLayout::shared_accounts`]), each with its listings. Each
    /// such account in the session is here, also if not listed. Thus a missing
    /// account means that the share is gone.
    pub shared: Vec<SharedListing>,
}

/// The collections of one shared account, as far as they were listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedListing {
    pub account: ServiceAccount,
    /// `None` when the address books were not listed: the account does not
    /// offer contacts, the user switched contacts off, or the listing failed.
    /// Only `Some` lets a populate conclude that a collection is gone.
    pub address_books: Option<Vec<Resource>>,
    /// As [`SharedListing::address_books`], for calendars.
    pub calendars: Option<Vec<Resource>>,
}

impl SharedListing {
    /// What was listed of `kind`, if it was.
    pub fn listing(&self, kind: ChildKind) -> Option<&[Resource]> {
        match kind {
            ChildKind::AddressBook => self.address_books.as_deref(),
            ChildKind::Calendar => self.calendars.as_deref(),
        }
    }
}

impl Fanout {
    /// Reads the session document, then lists the collections of the accounts
    /// that it resolved, for the parts that are switched on.
    ///
    /// A part that is off sends no request, so its vector is empty, also if
    /// the server holds collections. Use [`Fanout::listed`] to tell this from
    /// an account that holds no collection of that kind.
    ///
    /// The error is the `jmap_client` error, not a wrapper: each call that can
    /// fail here is a `jmap_client` call. The GObject layer maps that type to
    /// Evolution's error codes.
    pub fn discover(client: &Client, parts: Parts) -> Result<Self, jmap_client::Error> {
        let layout = CollectionLayout::from_session(client.session());

        let address_books = match layout.contacts.as_ref().filter(|_| parts.contacts) {
            Some(account) => resources(client.address_books(&account.id)?, Subscription::Honoured),
            None => Vec::new(),
        };
        let calendars = match layout.calendars.as_ref().filter(|_| parts.calendars) {
            Some(account) => resources(client.calendars(&account.id)?, Subscription::Honoured),
            None => Vec::new(),
        };

        // A failed listing of a shared account does not stop the populate. The
        // owner can withdraw the grant after the session fetch. The listing
        // stays `None`, so the existing children of the share stay.
        let shared = CollectionLayout::shared_accounts(client.session())
            .into_iter()
            .map(|service| {
                let account = service.account;
                let address_books = (service.contacts && parts.contacts)
                    .then(|| client.address_books(&account.id))
                    .and_then(|listing| shared_resources(&account, "address books", listing));
                let calendars = (service.calendars && parts.calendars)
                    .then(|| client.calendars(&account.id))
                    .and_then(|listing| shared_resources(&account, "calendars", listing));
                SharedListing {
                    account,
                    address_books,
                    calendars,
                }
            })
            .collect();

        Ok(Self {
            parts,
            layout,
            address_books,
            calendars,
            shared,
        })
    }
}

/// The name to show for a collection that the server calls `name`. It is never
/// blank (see [`Resource::name`]).
///
/// It is public because `create_resource_sync` must apply the same rule to the
/// collection that it creates. The server can change the requested name to a
/// blank name, and the sidebar must not show a row without text.
pub fn shown_name(name: &str, id: &Id) -> String {
    if name.trim().is_empty() {
        id.to_string()
    } else {
        name.to_owned()
    }
}

/// The collections of a shared account's listing that get a child, or `None`
/// with a warning when the listing failed.
fn shared_resources<C: Collection>(
    account: &ServiceAccount,
    what: &str,
    listing: Result<Vec<C>, jmap_client::Error>,
) -> Option<Vec<Resource>> {
    match listing {
        Ok(listing) => Some(resources(listing, Subscription::Ignored)),
        Err(error) => {
            tracing::warn!(
                account_id = %account.id,
                %error,
                "could not list the {what} of a shared account; keeping its children"
            );
            None
        }
    }
}

/// Whether a listing removes the collections with `isSubscribed: false`: yes
/// for the user's own accounts, no for shared accounts (see the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subscription {
    Honoured,
    Ignored,
}

/// The collections of a `/get` listing that get a child, in display order.
fn resources<C: Collection>(listing: Vec<C>, subscription: Subscription) -> Vec<Resource> {
    let mut resources: Vec<(u32, Resource)> = listing
        .into_iter()
        .filter(|collection| {
            subscription == Subscription::Ignored || collection.is_subscribed() != Some(false)
        })
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
                    deletable: collection.deletable(),
                },
            ))
        })
        .collect();

    // Sort by `sortOrder`, then by name for equal values. RFC 8621 §2 states
    // this for `Mailbox`, and both collection specifications copy the property.
    // The id is the last tie-break, so that two collections with the same name
    // have the same order at each populate.
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
/// Different documents define the two objects, but the shared properties have
/// the same names in both. Thus the selection is written once, and a rule for
/// address books also applies to calendars.
trait Collection {
    fn id(&self) -> Option<&Id>;
    fn name(&self) -> &str;
    /// 0 when absent, as both specifications define.
    fn sort_order(&self) -> u32;
    fn is_default(&self) -> Option<bool>;
    fn is_subscribed(&self) -> Option<bool>;
    /// `Calendar.color`. `None` for each `AddressBook`, which has no such
    /// property.
    fn color(&self) -> Option<String>;
    /// `myRights.is_writable()`, or `None` when the server sent no `myRights`
    /// for this collection (see [`Resource::writable`]).
    fn writable(&self) -> Option<bool>;
    /// `myRights.mayDelete`, or `None` when the server sent no `myRights`, or
    /// sent one without `mayDelete` (see [`Resource::deletable`]).
    fn deletable(&self) -> Option<bool>;
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
    fn deletable(&self) -> Option<bool> {
        self.my_rights.as_ref().and_then(|rights| rights.may_delete)
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
    fn deletable(&self) -> Option<bool> {
        self.my_rights.as_ref().and_then(|rights| rights.may_delete)
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
        resources(listing, Subscription::Honoured)
            .into_iter()
            .map(|resource| resource.name)
            .collect()
    }

    #[test]
    fn a_collection_saying_nothing_about_subscription_is_kept() {
        // Both specifications make `isSubscribed` optional. An absent value does
        // not mean that the user hid the collection.
        assert_eq!(names(vec![book(Some("AB1"), "Personal")]), ["Personal"]);
    }

    #[test]
    fn a_collection_with_no_id_is_dropped_rather_than_pointed_somewhere() {
        // Without an id, `[Resource] Identity` stays empty, and the backend
        // uses the account's *default* collection. That shows the wrong address
        // book under the correct name.
        let listing = vec![book(None, "Nameless"), book(Some("AB2"), "Personal")];
        assert_eq!(names(listing), ["Personal"]);
    }

    #[test]
    fn a_collection_the_server_named_nothing_is_shown_under_its_id() {
        // Both specifications require `name`, so this server is not compliant.
        // Without the id as name, the sidebar would show a blank row.
        let resources = resources(vec![book(Some("AB7"), "   ")], Subscription::Honoured);
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

        let resources = resources(
            vec![same_name_late, first, same_name_early],
            Subscription::Honoured,
        );
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

        let resources = resources(vec![default, plain], Subscription::Honoured);
        assert!(resources[0].is_default);
        assert!(
            !resources[1].is_default,
            "an absent isDefault is not the default"
        );
    }

    #[test]
    fn my_rights_absent_leaves_writable_none_and_present_is_read_through() {
        // Absent `myRights` gives `None`, so `Child::for_resource` uses the
        // account-wide `read_only` only.
        assert_eq!(
            resources(vec![book(Some("AB1"), "Personal")], Subscription::Honoured)[0].writable,
            None
        );

        let mut locked = book(Some("AB2"), "Read-only share");
        locked.my_rights = Some(AddressBookRights::default().with_may_write(false));
        assert_eq!(
            resources(vec![locked], Subscription::Honoured)[0].writable,
            Some(false)
        );

        let calendar_own_only = Calendar {
            id: Some(Id::new("Cal1")),
            name: "Work".to_owned(),
            my_rights: Some(
                CalendarRights::default()
                    .with_may_write_all(false)
                    .with_may_write_own(true),
            ),
            ..Calendar::default()
        };
        assert_eq!(
            resources(vec![calendar_own_only], Subscription::Honoured)[0].writable,
            Some(true)
        );
    }

    #[test]
    fn my_rights_absent_leaves_deletable_none_and_present_is_read_through() {
        // As the `writable` test, for `mayDelete`: an absent `myRights`, or one
        // without `mayDelete`, gives `None`, not a value.
        assert_eq!(
            resources(vec![book(Some("AB1"), "Personal")], Subscription::Honoured)[0].deletable,
            None
        );

        let mut undeletable = book(Some("AB2"), "Locked share");
        undeletable.my_rights = Some(AddressBookRights::default().with_may_delete(false));
        assert_eq!(
            resources(vec![undeletable], Subscription::Honoured)[0].deletable,
            Some(false)
        );

        let calendar = Calendar {
            id: Some(Id::new("Cal1")),
            name: "Work".to_owned(),
            my_rights: Some(CalendarRights::default().with_may_delete(true)),
            ..Calendar::default()
        };
        assert_eq!(
            resources(vec![calendar], Subscription::Honoured)[0].deletable,
            Some(true)
        );
    }

    #[test]
    fn a_calendars_color_is_carried_and_an_address_books_never_is() {
        // `AddressBook` has no `color` property, so its color is always `None`.
        // A `Calendar` keeps the server's value unchanged.
        let calendar = Calendar {
            id: Some(Id::new("Cal1")),
            name: "Work".to_owned(),
            color: Some("#ff8800".to_owned()),
            ..Calendar::default()
        };
        assert_eq!(
            resources(vec![calendar], Subscription::Honoured)[0].color,
            Some("#ff8800".to_owned())
        );

        assert_eq!(
            resources(vec![book(Some("AB1"), "Personal")], Subscription::Honoured)[0].color,
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
        assert_eq!(
            resources(vec![calendar], Subscription::Honoured)[0].color,
            None
        );
    }

    #[test]
    fn a_shared_collection_is_kept_whatever_its_subscription_says() {
        let mut unsubscribed = book(Some("AB1"), "Team");
        unsubscribed.is_subscribed = Some(false);

        assert_eq!(
            resources(vec![unsubscribed.clone()], Subscription::Honoured),
            [],
            "the user's own unsubscribed collection stays hidden"
        );
        assert_eq!(
            resources(vec![unsubscribed], Subscription::Ignored)[0].name,
            "Team",
            "a share starts unsubscribed and Evolution cannot subscribe to it"
        );
    }

    #[test]
    fn a_calendars_order_default_and_rights_are_read_from_the_calendar() {
        let late = Calendar {
            id: Some(Id::new("Cal1")),
            name: "Alpha".to_owned(),
            sort_order: Some(2),
            ..Calendar::default()
        };
        let early = Calendar {
            id: Some(Id::new("Cal2")),
            name: "Beta".to_owned(),
            sort_order: Some(1),
            is_default: Some(true),
            my_rights: Some(
                CalendarRights::default()
                    .with_may_write_all(false)
                    .with_may_write_own(false)
                    .with_may_delete(false),
            ),
            ..Calendar::default()
        };

        let resources = resources(vec![late, early], Subscription::Honoured);

        assert_eq!(
            resources
                .iter()
                .map(|resource| resource.id.as_str())
                .collect::<Vec<_>>(),
            ["Cal2", "Cal1"],
            "a lower sortOrder comes first, also before a lower name"
        );
        assert!(resources[0].is_default);
        assert_eq!(resources[0].writable, Some(false));
        assert_eq!(resources[0].deletable, Some(false));
    }
}
