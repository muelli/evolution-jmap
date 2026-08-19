// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What one JMAP account fans out into.
//!
//! M6's collection backend is handed a single `ESource` — one account, one
//! server, one set of credentials — and has to produce the children Evolution
//! shows: a mail account, an address book, a calendar. Which of those are
//! warranted is not a setting for the user to tick; the server already says so,
//! in the session document fetched at `/.well-known/jmap` (RFC 8620 §2).
//!
//! The answer comes in five parts, and this crate is all five of them and
//! nothing else. [`layout`] is the first: which JMAP account serves mail,
//! contacts and calendars for this login, read off the session document.
//! [`resources`] is the second: which address books and which calendars are
//! *in* those accounts, because Evolution shows one source per collection and
//! an account holds any number of them. [`Fanout::discover`] is those two
//! together. [`children`] is the third: what a `populate` makes of that — one
//! child source per collection, each under the resource id
//! `e_collection_backend_new_child` names it by and `dup_resource_id` has to
//! give back. [`child_source`] is the fourth: what has to be *set* on each of
//! those sources for it to be an address book of this account, and how the
//! resource id is read back off one that outlived a restart. [`parts`] is the
//! fifth, and it cuts across the other four: which of mail, contacts and
//! calendars the user left switched on, what is therefore not asked for and not
//! created — and, the half that can lose data, what a switched-off part means
//! for the children a previous populate already created.
//!
//! Like `jmap-book-sync`, `jmap-cal-sync` and `jmap-mail-sync`, it knows
//! nothing about GObject or the EDS headers, so the decision is testable on any
//! machine — here against a hand-written session document for the shapes a
//! server may present, and against `jmap-mockd` for the one a server does.
//!
//! [`create`] and [`delete`] are the two directions that do not merely mirror
//! the server: what Evolution's "New Address Book"/"New Calendar" and "Delete"
//! ask it to hold, or stop holding. They live beside the discovery for the same
//! reason as everything else here — the decision needs no headers, so it is
//! tested against a running `jmap-mockd`.
//!
//! *Creating* the `ESource`s is deliberately not here: the `ECollectionBackend`
//! subclass, the `e_source_get_extension` calls and the keyfile they write need
//! the headers, and [`Child`] and its [`Child::settings`] are what they will be
//! handed. The mail children are not described even here — see [`children`] for
//! why that is a stopping point rather than an omission.

pub mod child_source;
pub mod children;
pub mod create;
pub mod delete;
pub mod layout;
pub mod parts;
pub mod resources;

pub use child_source::{BACKEND_NAME, Connection, Setting, resource_id_for};
pub use children::{Child, ChildKind, parse_resource_id};
pub use create::{CreateFailure, Requested, create_collection};
pub use delete::{DeleteFailure, Doomed, delete_collection};
pub use layout::{CollectionLayout, MailService, ServiceAccount};
pub use parts::Parts;
pub use resources::{Fanout, Resource};
