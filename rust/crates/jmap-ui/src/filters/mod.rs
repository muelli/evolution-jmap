// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Server Filters page in the Evolution account editor.
//!
//! JMAP has this function in RFC 9661 (`SieveScript`, one set of scripts
//! for each account, at most one active). Evolution has no equivalent and
//! no backend hook; the pattern is [`crate::vacation`].
//!
//! The module is divided by what each part needs to run:
//!
//! - [`rules`] and [`script`] are the pure model: a rule, independent of
//!   Sieve, and the generator and parser that turn a list of rules into
//!   managed Sieve script text and back.
//! - [`form`] is the staged state of the page: the scripts, the edits the
//!   user made, which buttons are usable, and what one submit writes.
//! - [`rule_form`] is the staged state of rules mode: the managed script's
//!   rule list and one rule's own editing dialog. [`rule_dialog`] reads and
//!   writes the second of those; `page.rs` takes up the first in a later
//!   step.
//! - [`rule_dialog`] is the modal dialog that edits one rule. Its own pure
//!   half decides what each combo offers and how an index maps back to the
//!   model.
//! - [`io`] does the round trips over a connected
//!   [`jmap_client::Client`]. It runs on worker threads only.
//! - [`page`] is the `GtkScrolledWindow` subclass that implements
//!   `EMailConfigPage`.
//! - [`extension`] is the `EExtension` on `EMailConfigNotebook`. It checks
//!   the backend name of the account and adds the page.
//!
//! Only [`page`], [`extension`] and the second half of [`rule_dialog`] have
//! FFI. Every decision those widgets show belongs to [`form`] or to the
//! pure half of [`rule_dialog`], both of which have unit tests.

pub mod extension;
pub mod form;
pub mod io;
pub mod page;
pub mod rule_dialog;
pub mod rule_form;
pub mod rules;
pub mod script;
