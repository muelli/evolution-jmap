// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Blocking JMAP client.
//!
//! Talks to a JMAP server over HTTP: session discovery
//! (`/.well-known/jmap`), method batching against the API endpoint, and blob
//! upload/download. HTTP is abstracted behind the [`Transport`] trait so the
//! default `ureq` implementation can later be replaced by a libsoup-backed
//! one inside Evolution Data Server processes; [`CancelFlag`] is the seam
//! that will map to `GCancellable`.
//!
//! [`Transport`]: transport::Transport
//! [`CancelFlag`]: transport::CancelFlag

mod calendars;
mod changes;
mod client;
mod contacts;
mod error;
pub mod limits;
mod mail;
pub mod transport;
mod url;

pub use changes::ChangeSet;
pub use client::{Client, ClientBuilder, Credentials};
pub use error::Error;
pub use transport::CancelFlag;
