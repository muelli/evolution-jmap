// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What one JMAP account fans out into.
//!
//! M6's collection backend is handed a single `ESource` — one account, one
//! server, one set of credentials — and has to produce the children Evolution
//! shows: a mail account, an address book, a calendar. Which of those are
//! warranted is not a setting for the user to tick; the server already says so,
//! in the session document fetched at `/.well-known/jmap` (RFC 8620 §2). This
//! crate is that reading, and nothing else: it takes a [`Session`] and answers
//! which JMAP account serves mail, contacts and calendars for this login.
//!
//! Like `jmap-book-sync`, `jmap-cal-sync` and `jmap-mail-sync`, it knows
//! nothing about GObject or the EDS headers, so the decision is testable on any
//! machine — here against a hand-written session document for the shapes a
//! server may present, and against `jmap-mockd` for the one a server does.
//!
//! Turning a [`CollectionLayout`] into `ESource`s is deliberately *not* here.
//! That is the EDS-side half (`e_collection_backend_new_child`, the
//! `[Mail Account]`/`[Mail Transport]`/`[Address Book]`/`[Calendar]`
//! extensions), it needs the headers, and it is the part no test on this
//! machine can verify.

pub mod layout;

pub use layout::{CollectionLayout, MailService, ServiceAccount};
