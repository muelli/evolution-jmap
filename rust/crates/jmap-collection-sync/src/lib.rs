// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! What one JMAP account fans out into.
//!
//! The collection backend gets one `ESource`: one account, one server, one set
//! of credentials. From it, the backend makes the child sources that Evolution
//! shows. The user does not select which children exist. The server tells this
//! in the session document at `/.well-known/jmap` (RFC 8620 §2).
//!
//! The modules:
//!
//! - [`layout`]: which JMAP account serves mail, contacts and calendars for
//!   this login, and which shared accounts offer contacts or calendars.
//! - [`resources`]: which address books and calendars are in those accounts.
//!   Evolution shows one source for each collection, and an account can hold
//!   many. [`Fanout::discover`] does [`layout`] and [`resources`] together.
//! - [`children`]: the child sources that a `populate` makes from a
//!   [`Fanout`], one for each collection. Each child has a resource id:
//!   `e_collection_backend_new_child` gets it, and `dup_resource_id` must
//!   return it.
//! - [`child_source`]: the settings to write on each child source, and how to
//!   read the resource id back from a child source after a restart.
//! - [`parts`]: which of mail, contacts and calendars the user switched on.
//!   A switched-off part is not listed and gets no children. Its existing
//!   children stay: they are not obsolete.
//! - [`create`]: create an address book or a calendar on the server, for
//!   Evolution's "New Address Book" and "New Calendar".
//! - [`delete`]: destroy an address book or a calendar on the server, for
//!   Evolution's "Delete".
//! - [`pending`]: the undo window before that destroy, and when it is due.
//!
//! Like `jmap-book-sync`, `jmap-cal-sync` and `jmap-mail-sync`, this crate does
//! not use GObject or the EDS headers. Thus tests can examine each decision on
//! any machine: against hand-written session documents, and against
//! `jmap-mockd`.
//!
//! This crate does not create the `ESource`s. The `ECollectionBackend`
//! subclass in `jmap-backend-collection` needs the EDS headers for that. It
//! creates each source from a [`Child`] and writes [`Child::settings`] on it.
//!
//! This crate does not describe the mail sources either. They are not children
//! of the collection backend: the account setup writes them, and the
//! `prepare_mail` vfunc of `jmap-backend-collection` fills them in.
//! [`CollectionLayout`] reports the mail service, but no child comes from it.

pub mod child_source;
pub mod children;
pub mod create;
pub mod delete;
pub mod layout;
pub mod parts;
pub mod pending;
pub mod resources;

pub use child_source::{BACKEND_NAME, Connection, Setting, resource_id_for};
pub use children::{Child, ChildKind, parse_resource_id, split_shared_identity};
pub use create::{CreateFailure, Requested, create_collection};
pub use delete::{DeleteFailure, Doomed, delete_collection};
pub use layout::{CollectionLayout, MailService, ServiceAccount, SharedService};
pub use parts::Parts;
pub use resources::{Fanout, Resource, SharedListing};
