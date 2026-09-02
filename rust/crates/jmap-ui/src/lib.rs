// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP-only features in Evolution's own UI.
//!
//! The Camel provider and the EDS backends give Evolution what its existing
//! surfaces can show; this crate is for what they cannot: a vacation
//! autoresponder page in the account editor, scheduled send in the composer,
//! snooze in the message list — server features Evolution has no concept of
//! and no backend hook for (the survey is EVOLUTION-GAP.md in the harness
//! repository; scheduled send and snooze are GNOME/evolution#411 and #374
//! upstream).
//!
//! Everything here is an `EExtension` registered by [`module::load`] out of
//! the same `module-jmap-configuration.so` the account-setup module lives in,
//! and everything is gated twice: a synchronous "is this account ours" check
//! on the `ESource`/`CamelProvider` decides whether UI exists at all, and the
//! server-side facts in [`session_cache`] decide whether it is sensitive.

pub mod dispatch;
pub mod link;
pub mod module;
pub mod send_later;
pub mod session_cache;
pub mod snooze;
pub mod vacation;
