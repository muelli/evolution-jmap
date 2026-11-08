// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Delegates page in the Evolution account editor (ROADMAP item 164).
//!
//! [`crate::delegation`] holds the pure logic: delegation levels, the
//! `shareWith` patches and client calls they drive, and the `Principal/query`
//! lookup. This module is the page itself, the same split as
//! [`crate::vacation`] and [`crate::filters`]:
//!
//! - [`io`] does the round trip over a connected [`jmap_client::Client`]. It
//!   runs on worker threads only.
//! - [`page`] is the `GtkScrolledWindow` subclass that implements
//!   `EMailConfigPage`.
//! - [`extension`] is the `EExtension` on `EMailConfigNotebook`. It checks
//!   the backend name of the account and adds the page.
//!
//! This first slice is read-only: it lists the account's existing
//! delegates. Granting, revoking and the `Principal/query` picker stay a
//! later claim, now with a page to add them to.

pub mod extension;
pub mod io;
pub mod page;
