// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Blocking JMAP client.
//!
//! Talks to a JMAP server over HTTP: session discovery
//! (`/.well-known/jmap`), method batching against the API endpoint, and blob
//! upload/download. HTTP is abstracted behind the [`Transport`] trait so the
//! default `ureq` implementation can later be replaced by a libsoup-backed
//! one inside Evolution Data Server processes; the cancellation hook maps to
//! `GCancellable` there.
//!
//! [`Transport`]: transport::Transport

pub mod transport {}
