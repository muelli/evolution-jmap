// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Creating a collection *on the server*, and the child source it becomes.
//!
//! [`Fanout::discover`](crate::Fanout::discover) is this crate's read direction:
//! ask the server what it holds, decide which children that is. This is the one
//! write direction it has — Evolution's "New Address Book"/"New Calendar" asked
//! for a collection that does not exist yet, so it is made, and then the same
//! [`Child`] is derived for it that the next discovery would have derived.
//!
//! It lives here, beside the discovery, rather than in the collection backend,
//! for the reason the crate split exists at all: what a child *is* is decided
//! without the EDS headers, so it can be tested against a running `jmap-mockd`
//! in the ordinary workspace test run. What the backend keeps is the two ends
//! that need EDS — reading the kind and name off the scratch `ESource`, and
//! writing the child's settings back onto it.
//!
//! ## The account is resolved, never assumed
//!
//! [`create_collection`] asks [`CollectionLayout`] which JMAP account serves the
//! kind in question, exactly as a discovery does, and refuses when it serves
//! none. Sending the create to the session's primary account instead would, on a
//! server whose contacts and calendars sit in different accounts, put the new
//! address book in an account whose collections this backend never lists — a
//! collection that exists on the server and never appears in Evolution, which is
//! a worse outcome than a create that said no.

use jmap_client::Client;
use jmap_proto::Id;
use jmap_proto::calendars::Calendar;
use jmap_proto::contacts::AddressBook;

use crate::children::{Child, ChildKind};
use crate::layout::CollectionLayout;
use crate::resources::{Resource, shown_name};

/// What a create asks for: one collection of one kind, under one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requested {
    pub kind: ChildKind,
    /// The name to create it under — in Evolution's case whatever the user typed
    /// in the dialog.
    ///
    /// May be empty, and is sent as such rather than refused: a name is the
    /// server's to validate, and refusing one here would answer a name the
    /// server would have accepted with an error of this code's own. What is
    /// never empty is the name written back onto the child — see
    /// [`shown_name`](crate::resources::shown_name).
    pub display_name: String,
}

/// Why a collection could not be created.
#[derive(Debug)]
pub enum CreateFailure {
    /// The login holds no JMAP account serving collections of that kind, so
    /// there is nowhere to create one.
    Unserved(ChildKind),
    /// The server refused the create, or could not be reached.
    Client(jmap_client::Error),
}

impl From<jmap_client::Error> for CreateFailure {
    fn from(error: jmap_client::Error) -> Self {
        Self::Client(error)
    }
}

impl std::fmt::Display for CreateFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unserved(kind) => write!(
                f,
                "this account's JMAP server offers no {} to create one in",
                match kind {
                    ChildKind::AddressBook => "contacts",
                    ChildKind::Calendar => "calendars",
                }
            ),
            Self::Client(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CreateFailure {}

/// Creates `requested` on the server and answers the child source it is to
/// become.
///
/// The `Child` is built by [`Child::for_resource`] from the object the *server*
/// answered with, not from what was asked for: a server may normalise the name,
/// may flag the new collection default, and — for a calendar — may assign it a
/// colour. All three reach the child that way, so a created child is the child
/// the next discovery would have written rather than a near-miss of it.
///
/// The create explicitly asks for `isSubscribed: true`. Both specifications
/// leave a freshly created collection's subscription state up to the server,
/// and a real server (confirmed against Stalwart, which is not merely
/// theoretical) defaults it to *unsubscribed* rather than leaving the property
/// absent — and [`crate::resources`]'s own discovery filter drops
/// `isSubscribed == Some(false)` on purpose, treating it exactly like a
/// collection the user removed. Left to that default, "New Address Book"/"New
/// Calendar" would create a collection that then vanishes from the sidebar
/// until some other client happens to subscribe it. A user who just asked to
/// create a collection has unambiguously asked to see it, so subscription is
/// requested, not assumed from the server's silence.
pub fn create_collection(client: &Client, requested: &Requested) -> Result<Child, CreateFailure> {
    let layout = CollectionLayout::from_session(client.session());
    let kind = requested.kind;
    let account = layout.account_for(requested.kind).ok_or_else(|| {
        let error = CreateFailure::Unserved(requested.kind);
        tracing::warn!(?kind, %error, "collection create failed");
        error
    })?;

    let account_id = account.id.to_string();
    let display_name = requested.display_name.as_str();
    tracing::debug!(account_id, ?kind, display_name, "creating collection");

    let resource = match requested.kind {
        ChildKind::AddressBook => {
            let created = client
                .address_book_create(
                    &account.id,
                    &AddressBook {
                        name: requested.display_name.clone(),
                        is_subscribed: Some(true),
                        ..AddressBook::default()
                    },
                )
                .inspect_err(|error| {
                    tracing::warn!(account_id, ?kind, %error, "collection create failed");
                })?;
            let writable = created
                .my_rights
                .as_ref()
                .map(|rights| rights.is_writable());
            created_resource(
                created.id,
                &created.name,
                created.is_default,
                None,
                writable,
            )
        }
        ChildKind::Calendar => {
            let created = client
                .calendar_create(
                    &account.id,
                    &Calendar {
                        name: requested.display_name.clone(),
                        is_subscribed: Some(true),
                        ..Calendar::default()
                    },
                )
                .inspect_err(|error| {
                    tracing::warn!(account_id, ?kind, %error, "collection create failed");
                })?;
            let color = created.color.clone();
            let writable = created
                .my_rights
                .as_ref()
                .map(|rights| rights.is_writable());
            created_resource(
                created.id,
                &created.name,
                created.is_default,
                color,
                writable,
            )
        }
    }
    .ok_or_else(|| CreateFailure::Client(missing_id(requested.kind)))?;

    Ok(Child::for_resource(requested.kind, account, &resource))
}

/// The [`Resource`] a just-created collection is, or `None` if the server named
/// it no id.
///
/// A `/set` create that reports no `id` for the object it created is a server
/// breaking RFC 8620 §5.3, and not a corner to shrug at: the id is what
/// `[Resource] Identity` carries, and a child written without one is a child
/// whose cache file EDS deletes on the next start. So it is refused here rather
/// than turned into an empty identity two layers down.
fn created_resource(
    id: Option<Id>,
    name: &str,
    is_default: Option<bool>,
    color: Option<String>,
    writable: Option<bool>,
) -> Option<Resource> {
    let id = id?;
    Some(Resource {
        name: shown_name(name, &id),
        is_default: is_default == Some(true),
        color,
        writable,
        id,
    })
}

/// The error for a `/set` create the server answered with no id.
fn missing_id(kind: ChildKind) -> jmap_client::Error {
    jmap_client::Error::Protocol(format!(
        "the server created {} and reported no id for it",
        match kind {
            ChildKind::AddressBook => "an address book",
            ChildKind::Calendar => "a calendar",
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_created_collection_takes_the_servers_name_not_the_requested_one() {
        // The server answers a create with the object it created; a server that
        // normalised the name is telling the client what the collection is
        // actually called.
        let resource = created_resource(Some(Id::new("AB9")), "Work Contacts", None, None, None)
            .expect("the server named an id");
        assert_eq!(resource.name, "Work Contacts");
        assert_eq!(resource.id, Id::new("AB9"));
        assert!(!resource.is_default);
    }

    #[test]
    fn a_created_collection_the_server_named_nothing_shows_its_id() {
        // The same never-blank rule a listing applies — a blank row in the
        // sidebar is one the user cannot tell from another blank row.
        let resource = created_resource(Some(Id::new("Cal9")), "   ", None, None, None)
            .expect("the server named an id");
        assert_eq!(resource.name, "Cal9");
    }

    #[test]
    fn a_create_the_server_reported_no_id_for_is_no_resource() {
        // `[Resource] Identity` is what this becomes, and a child written
        // without one is a child EDS deletes the cache file of.
        assert!(created_resource(None, "Work", None, None, None).is_none());
    }

    #[test]
    fn the_servers_default_flag_and_calendar_colour_reach_the_resource() {
        let resource = created_resource(
            Some(Id::new("Cal9")),
            "Work",
            Some(true),
            Some("#ff8800".to_owned()),
            None,
        )
        .expect("the server named an id");
        assert!(resource.is_default);
        assert_eq!(resource.color, Some("#ff8800".to_owned()));
    }

    #[test]
    fn the_servers_my_rights_reach_the_resource_as_writable() {
        // A create's response can carry `myRights` right away (RFC 8620 §5.3
        // confirms whichever properties the server chooses in a create's
        // response), and the same reading `resources()` gives a discovered
        // collection has to apply here too, or a freshly created read-only
        // share would show as writable until the next populate corrected it.
        let resource = created_resource(
            Some(Id::new("AB9")),
            "Read-only share",
            None,
            None,
            Some(false),
        )
        .expect("the server named an id");
        assert_eq!(resource.writable, Some(false));
    }
}
