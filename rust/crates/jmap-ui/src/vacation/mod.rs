// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The vacation autoresponder page in the Evolution account editor.
//!
//! JMAP has this function in RFC 8621 §8 (`VacationResponse`, one singleton
//! object for each account). Evolution has no equivalent and no backend hook.
//! The precedent is the evolution-ews Out of Office page: an out-of-tree
//! module that talks to the server directly.
//!
//! The module has four parts, divided by what each part needs to run:
//!
//! - [`form`] maps between widget values and the `VacationResponse` object.
//!   It has no FFI and has unit tests.
//! - [`io`] does the two round trips over a connected
//!   [`jmap_client::Client`]. It runs on worker threads only.
//!   [`crate::link`] makes the connection from the `ESource` of the editor.
//! - [`page`] is the `GtkScrolledWindow` subclass that implements
//!   `EMailConfigPage`.
//! - [`extension`] is the `EExtension` on `EMailConfigNotebook`. It checks
//!   the backend name of the account and adds the page.
//!
//! Only the account editor builds an `EMailConfigNotebook`. The new-account
//! assistant puts its pages together directly. Thus the page shows only for
//! an account that exists, where the server-side setting is available.

pub mod extension;
pub mod form;
pub mod io;
pub mod page;
