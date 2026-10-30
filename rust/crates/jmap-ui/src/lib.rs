// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP-only features in the UI of Evolution.
//!
//! The Camel provider and the EDS backends supply the data that the existing
//! Evolution surfaces can show. This crate supplies the features that they
//! cannot show:
//!
//! - a vacation autoresponder page in the account editor,
//! - scheduled send in the composer,
//! - snooze in the message list,
//! - an info bar with "Undo" after the delete of a calendar or an address
//!   book,
//! - a placeholder, instead of a silent `<img src>`, for a contact photo
//!   that a vCard gives as an http(s) URL.
//!
//! Evolution has no concept of these server features and no backend hook for
//! them. Upstream, scheduled send is GNOME/evolution#411 and snooze is
//! GNOME/evolution#374.
//!
//! Each feature is an `EExtension` that [`module::load`] registers. The
//! registration comes from `module-jmap-configuration.so`, the same file that
//! holds the account-setup module. Each feature has two gates:
//!
//! 1. A synchronous check on the `ESource`/`CamelProvider` ("is this account
//!    a JMAP account?") decides if the UI exists.
//! 2. The server-side facts in [`session_cache`] decide if the UI is
//!    sensitive.

pub mod dispatch;
pub mod external_photo;
pub mod filters;
pub mod link;
pub mod module;
pub mod send_later;
pub mod session_cache;
pub mod snooze;
pub mod undo_delete;
pub mod vacation;
