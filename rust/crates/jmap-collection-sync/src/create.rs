// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Creation of a collection *on the server*, and the child source that it
//! becomes.
//!
//! [`Fanout::discover`](crate::Fanout::discover) is the read direction of this
//! crate: it asks the server what it holds and decides which children that
//! gives. This module is a write direction ([`crate::delete`] is the other).
//! Evolution's "New Address Book"/"New Calendar" asks for a collection that does
//! not exist yet. This module creates it, and then derives the same [`Child`]
//! for it that the next discovery derives.
//!
//! It is here, next to the discovery, and not in the collection backend, for the
//! reason that the crate split exists: this crate decides what a child *is*
//! without the EDS headers. Thus the ordinary workspace test run can test it
//! against a running `jmap-mockd`. The backend keeps the two ends that need EDS:
//! - it reads the kind and the name from the scratch `ESource`;
//! - it writes the settings of the child back onto that `ESource`.
//!
//! ## The account is resolved, never assumed
//!
//! [`create_collection`] asks [`CollectionLayout`] which JMAP account serves the
//! kind, as a discovery does, and refuses when no account serves it. The
//! alternative is to send the create to the primary account of the session. On a
//! server that keeps contacts and calendars in different accounts, this puts the
//! new address book in an account that this backend never lists. The collection
//! then exists on the server but never appears in Evolution, which is worse than
//! a refused create.

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
    /// The name for the new collection. In Evolution, this is the text that the
    /// user typed in the dialog.
    ///
    /// It can be empty, and is then sent as it is, not refused. The server
    /// validates names. If this code refused a name, it could refuse a name that
    /// the server accepts. The name written back onto the child is never empty:
    /// see [`shown_name`](crate::resources::shown_name).
    pub display_name: String,
}

/// Why a collection could not be created.
#[derive(Debug)]
pub enum CreateFailure {
    /// The login has no JMAP account that serves collections of that kind, so
    /// no place exists to create one.
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

/// Creates `requested` on the server and returns the child source that it
/// becomes.
///
/// [`Child::for_resource`] builds the `Child` from the object in the *server's*
/// response, not from the request. A server can normalise the name, mark the new
/// collection as the default, and give a calendar a colour. All three get to the
/// child, so a created child is the same child that the next discovery writes.
///
/// One exception: when the response has no `name`, the requested name is used,
/// not the id. RFC 8620 §5.3 lets a `created` response omit each property that
/// the server did not set. A name that the server accepted without change is
/// such a property, so a real server can omit `name`.
///
/// The create asks for `isSubscribed: true`. Both specifications let the server
/// decide the subscription state of a new collection. A real server (Stalwart,
/// confirmed) sets it to *unsubscribed* and does not omit the property. The
/// discovery filter in [`crate::resources`] intentionally drops
/// `isSubscribed == Some(false)`, as for a collection that the user removed.
/// With that default, "New Address Book"/"New Calendar" creates a collection
/// that disappears from the sidebar until a different client subscribes it. A
/// user who asks to create a collection wants to see it, so the create requests
/// the subscription.
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
            let deletable = created
                .my_rights
                .as_ref()
                .and_then(|rights| rights.may_delete);
            created_resource(
                created.id,
                &created.name,
                display_name,
                created.is_default,
                None,
                writable,
                deletable,
            )
        }
        ChildKind::Calendar => {
            let created = client
                .calendar_create(
                    &account.id,
                    &Calendar::new(requested.display_name.clone()).is_subscribed(true),
                )
                .inspect_err(|error| {
                    tracing::warn!(account_id, ?kind, %error, "collection create failed");
                })?;
            let color = created.color.clone();
            let writable = created
                .my_rights
                .as_ref()
                .map(|rights| rights.is_writable());
            let deletable = created
                .my_rights
                .as_ref()
                .and_then(|rights| rights.may_delete);
            created_resource(
                created.id,
                &created.name,
                display_name,
                created.is_default,
                color,
                writable,
                deletable,
            )
        }
    }
    .ok_or_else(|| CreateFailure::Client(missing_id(requested.kind)))?;

    Ok(Child::for_resource(requested.kind, account, &resource))
}

/// The [`Resource`] for a new collection, or `None` if the server gave it no id.
///
/// A `/set` create that reports no `id` for the new object breaks RFC 8620 §5.3.
/// This is important: `[Resource] Identity` contains the id, and EDS deletes the
/// cache file of a child without one on the next start. Thus this function
/// refuses it, and does not give an empty identity two layers lower.
///
/// `server_name` is the name in the create response, and can be blank. A
/// `created` response must only contain the properties that the server set
/// (RFC 8620 §5.3). When the server accepted the requested name without change,
/// `name` is not one of them. Thus a blank `server_name` means "accepted as
/// requested", not "the server gave no name". The result is as follows:
/// - a `server_name` that is not blank (a normalisation) is used;
/// - if not, `requested_name` is used;
/// - if both are blank, the id is used.
fn created_resource(
    id: Option<Id>,
    server_name: &str,
    requested_name: &str,
    is_default: Option<bool>,
    color: Option<String>,
    writable: Option<bool>,
    deletable: Option<bool>,
) -> Option<Resource> {
    let id = id?;
    let name = if server_name.trim().is_empty() {
        requested_name
    } else {
        server_name
    };
    Some(Resource {
        name: shown_name(name, &id),
        is_default: is_default == Some(true),
        color,
        writable,
        deletable,
        id,
    })
}

/// The error for a `/set` create that the server answered with no id.
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
        // A normalised name in the response is the real name of the
        // collection, so it overrides the requested name.
        let resource = created_resource(
            Some(Id::new("AB9")),
            "Work Contacts",
            "work contacts",
            None,
            None,
            None,
            None,
        )
        .expect("the server named an id");
        assert_eq!(resource.name, "Work Contacts");
        assert_eq!(resource.id, Id::new("AB9"));
        assert!(!resource.is_default);
    }

    #[test]
    fn a_created_collection_whose_response_omits_the_name_shows_the_requested_name() {
        // RFC 8620 §5.3: a `created` response must only contain the properties
        // that the server set. The caller gives an empty string when the
        // response has no `name`. This means "accepted as requested", so the
        // result is the requested name, not the id.
        let resource =
            created_resource(Some(Id::new("CiV")), "", "testcal", None, None, None, None)
                .expect("the server named an id");
        assert_eq!(resource.name, "testcal");
    }

    #[test]
    fn a_created_collection_neither_name_names_anything_shows_its_id() {
        // A listing uses the same rule: a name is never blank, because the user
        // cannot tell two blank rows in the sidebar apart. Evolution refuses an
        // empty name in its create dialogs, but `Requested` sends a blank name
        // and does not refuse it.
        let resource = created_resource(Some(Id::new("Cal9")), "   ", "  ", None, None, None, None)
            .expect("the server named an id");
        assert_eq!(resource.name, "Cal9");
    }

    #[test]
    fn a_create_the_server_reported_no_id_for_is_no_resource() {
        // The id becomes `[Resource] Identity`. EDS deletes the cache file of a
        // child without one.
        assert!(created_resource(None, "Work", "Work", None, None, None, None).is_none());
    }

    #[test]
    fn the_servers_default_flag_and_calendar_colour_reach_the_resource() {
        let resource = created_resource(
            Some(Id::new("Cal9")),
            "Work",
            "Work",
            Some(true),
            Some("#ff8800".to_owned()),
            None,
            None,
        )
        .expect("the server named an id");
        assert!(resource.is_default);
        assert_eq!(resource.color, Some("#ff8800".to_owned()));
    }

    #[test]
    fn the_servers_my_rights_reach_the_resource_as_writable() {
        // A create response can contain `myRights` (RFC 8620 §5.3 lets the
        // server add properties to it). The reading of `resources()` for a
        // discovered collection applies here too. If not, a new read-only share
        // shows as writable until the next populate.
        let resource = created_resource(
            Some(Id::new("AB9")),
            "Read-only share",
            "Read-only share",
            None,
            None,
            Some(false),
            None,
        )
        .expect("the server named an id");
        assert_eq!(resource.writable, Some(false));
    }

    #[test]
    fn the_servers_my_rights_reach_the_resource_as_deletable() {
        // As `writable` above, for `mayDelete`. If the rights in the response
        // do not get to the resource, a new share that cannot be deleted shows
        // as deletable until the next populate.
        let resource = created_resource(
            Some(Id::new("AB9")),
            "Locked share",
            "Locked share",
            None,
            None,
            None,
            Some(false),
        )
        .expect("the server named an id");
        assert_eq!(resource.deletable, Some(false));
    }
}
