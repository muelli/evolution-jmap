// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The vacation autoresponder page in Evolution's account editor.
//!
//! JMAP has had this since RFC 8621 §8 (`VacationResponse`, one singleton
//! object per account); Evolution has no concept of it and no backend hook —
//! the survey that established that is EVOLUTION-GAP.md in the harness
//! repository, and the precedent for shipping it anyway is evolution-ews's
//! Out of Office page: an out-of-tree module that talks to the server itself.
//!
//! Four pieces, split by what they need to run: [`form`] is the pure mapping
//! between widget values and the `VacationResponse` object (unit-tested,
//! FFI-free); [`io`] turns the editor's `ESource` into a connected
//! [`jmap_client::Client`] and does the two round trips (worker threads
//! only); [`page`] is the `GtkBox` subclass implementing `EMailConfigPage`;
//! [`extension`] is the `EExtension` on `EMailConfigNotebook` that gates on
//! the account's backend name and adds the page.
//!
//! Only the account *editor* builds an `EMailConfigNotebook` — the
//! new-account assistant composes its pages directly — so the page appears
//! exactly where a server-side setting can be read: on an account that
//! already exists.

pub mod extension;
pub mod form;
pub mod io;
pub mod page;
