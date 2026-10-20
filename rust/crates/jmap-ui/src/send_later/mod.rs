// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scheduled send from the composer.
//!
//! Evolution has no send-later function. `CamelTransport` has no time for
//! each message, and the Outbox has no such function. The `presend` signal of
//! the composer can only stop a send; it cannot replace it
//! (GNOME/evolution#411 records this upstream gap). Thus this feature does
//! not change the usual Send path. A *File ▸ Send Later* submenu tells the
//! server to hold the message: `Email/import` into Drafts, and an
//! `EmailSubmission` with an RFC 4865 `HOLDFOR`. The `maxDelayedSend` of the
//! account and the FUTURERELEASE capability advertise this mechanism
//! (RFC 8621 §7).
//!
//! The module has three parts, one for each set of needs:
//! - [`schedule`] changes a preset into seconds on the local clock (GLib
//!   calendar, no widgets).
//! - [`submit`] is the blocking JMAP conversation (worker threads only,
//!   tested against the mock).
//! - [`extension`] is the `EExtension` on `EMsgComposer`. It adds the menu,
//!   runs the gate again at each From change, and sends a click through
//!   `e_msg_composer_get_message` to [`submit::schedule_send`].
//!
//! The gate has two levels. Level 1: the transport backend name of the From
//! identity tells if the menu applies to this account. Level 2: the
//! `maxDelayedSend` of the server tells if the menu is sensitive. The
//! extension gets this value off the main loop and keeps it on the composer.
//! An insensitive menu keeps a tooltip that tells why; the menu layout does
//! not change.

pub mod extension;
pub mod schedule;
pub mod submit;
