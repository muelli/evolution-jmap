// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The info bar with "Undo" after the delete of a calendar or an address book.
//!
//! The registry does not destroy a deleted collection at once. It disables the
//! child source and stores a due time on it (`jmap_config::pending_delete`).
//! While the due time is in the future, [`shell_ext`] shows an info bar. Undo
//! enables the source again, and then the registry sends nothing to the server.
//! [`notice`] decides the text and the time.

pub mod notice;
pub mod shell_ext;
