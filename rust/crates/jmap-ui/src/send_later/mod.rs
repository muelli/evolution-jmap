// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scheduled send from the composer.
//!
//! Evolution has no send-later concept — no per-message time in
//! `CamelTransport`, nothing in the Outbox, and the composer's `presend`
//! signal can only veto a send, not substitute one (GNOME/evolution#411
//! tracks the upstream gap). So this feature does not touch the ordinary Send
//! path at all: a *File ▸ Send Later* submenu asks the server to hold the
//! message instead — `Email/import` into Drafts and an `EmailSubmission` with
//! an RFC 4865 `HOLDFOR`, the mechanism the account's `maxDelayedSend` and
//! FUTURERELEASE capability advertise (RFC 8621 §7).
//!
//! Split by what each piece needs: [`schedule`] turns a preset into seconds
//! on the local clock (GLib's calendar, no widgets); [`submit`] is the
//! blocking JMAP conversation (worker threads only, mock-tested);
//! [`extension`] is the `EExtension` on `EMsgComposer` that merges the menu,
//! re-gates it on every From switch, and walks a click through
//! `e_msg_composer_get_message` to [`submit::schedule_send`].
//!
//! The gate is the plan's two levels: the From identity's transport backend
//! name says whether the menu is for this account at all, and the server's
//! own `maxDelayedSend`/FUTURERELEASE (fetched off the main loop, cached on
//! the composer) say whether it is sensitive. Insensitive controls keep a
//! tooltip saying why — geometry never changes.

pub mod extension;
pub mod schedule;
pub mod submit;
