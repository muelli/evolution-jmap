// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stateful in-memory mock JMAP server.
//!
//! Serves a subset of JMAP sufficient for exercising the Evolution JMAP
//! client end-to-end in tests: session discovery, Basic/Bearer auth, mail
//! read/send, contacts CRUD, and calendar CRUD. State lives behind an
//! `Arc<Mutex<..>>` handle that tests can inspect directly (for example the
//! recorded `EmailSubmission` outbox).
//!
//! Not a real server: single-threaded per connection via `tiny_http`,
//! plaintext HTTP on `127.0.0.1`, everything held in memory.

mod auth;
mod dispatch;
mod mail;
mod patch;
mod server;
mod state;

pub use mail::EmailSeed;
pub use server::{DEFAULT_ACCOUNT_ID, DEFAULT_ACCOUNT_NAME, MockServer, MockServerBuilder};
pub use state::{AccountState, Blob, Change, ChangeKind, RecordedSubmission, ServerState, Store};
