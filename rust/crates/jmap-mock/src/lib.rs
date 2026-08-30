// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stateful in-memory mock JMAP server.
//!
//! Serves a subset of JMAP sufficient for exercising the Evolution JMAP
//! client end-to-end in tests: session discovery, Basic/Bearer auth, mail
//! read/send/import, contacts CRUD, and calendar CRUD. State lives behind an
//! `Arc<Mutex<..>>` handle that tests can inspect directly (for example the
//! recorded `EmailSubmission` outbox).
//!
//! Not a real server: single-threaded per connection via `tiny_http`,
//! plaintext HTTP on `127.0.0.1`, everything held in memory. The one
//! exception is `/eventsource` (RFC 8620 §7.3): a long-lived connection is
//! handed to its own thread rather than answered inline — see the
//! `eventsource` module doc.

mod auth;
mod calendars;
mod contacts;
mod dispatch;
mod eventsource;
mod mail;
mod message;
mod patch;
mod principals;
mod server;
mod setops;
mod state;

pub use mail::EmailSeed;
pub use server::{
    DEFAULT_ACCOUNT_ID, DEFAULT_ACCOUNT_NAME, DEFAULT_CALLS_IN_REQUEST, DEFAULT_OBJECTS_IN_GET,
    DEFAULT_SIZE_REQUEST, DEFAULT_SIZE_UPLOAD, MockServer, MockServerBuilder,
};
pub use state::{
    AccountState, Blob, Change, ChangeKind, EventSourceHub, RecordedSubmission, ServerState, Store,
};
