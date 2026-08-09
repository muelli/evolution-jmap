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
//! The answer comes in three parts, and this crate is all three of them and
//! nothing else. [`layout`] is the first: which JMAP account serves mail,
//! contacts and calendars for this login, read off the session document.
//! [`resources`] is the second: which address books and which calendars are
//! *in* those accounts, because Evolution shows one source per collection and
//! an account holds any number of them. [`Fanout::discover`] is those two
//! together. [`children`] is the third: what a `populate` makes of that — one
//! child source per collection, each under the resource id
//! `e_collection_backend_new_child` names it by and `dup_resource_id` has to
//! give back.
//!
//! Like `jmap-book-sync`, `jmap-cal-sync` and `jmap-mail-sync`, it knows
//! nothing about GObject or the EDS headers, so the decision is testable on any
//! machine — here against a hand-written session document for the shapes a
//! server may present, and against `jmap-mockd` for the one a server does.
//!
//! *Creating* the `ESource`s is deliberately not here: the calls, the
//! `[Address Book]`/`[Calendar]`/`[Resource]` extensions and the keyfile they
//! write need the headers, and [`Child`] is what they will be handed. The mail
//! children are not described even here — see [`children`] for why that is a
//! stopping point rather than an omission.

pub mod children;
pub mod layout;
pub mod resources;

pub use children::{Child, ChildKind, parse_resource_id};
pub use layout::{CollectionLayout, MailService, ServiceAccount};
pub use resources::{Fanout, Resource};
