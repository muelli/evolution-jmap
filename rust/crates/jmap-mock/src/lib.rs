// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! Provide an in-memory mock JMAP server with mutable state.
//! Use this crate in integration tests for the JMAP surface under test.
//! Inspect server state through `Arc<Mutex<..>>`.
//! The server listens on `127.0.0.1` over plain HTTP through `tiny_http`.
//! The `/eventsource` endpoint runs on a separate thread.

mod auth;
mod blob;
mod calendars;
mod contacts;
mod dispatch;
mod eventsource;
mod mail;
mod message;
mod patch;
mod principals;
mod quota;
mod scheduling;
mod server;
mod setops;
mod sieve;
mod state;
mod time;

pub use mail::EmailSeed;
pub use server::{
    DEFAULT_ACCOUNT_ID, DEFAULT_ACCOUNT_NAME, DEFAULT_CALLS_IN_REQUEST, DEFAULT_OBJECTS_IN_GET,
    DEFAULT_SIZE_REQUEST, DEFAULT_SIZE_UPLOAD, Faults, MockServer, MockServerBuilder,
};
pub use state::{
    AccountState, Blob, Change, ChangeKind, EventSourceHub, RecordedSchedulingMessage,
    RecordedSubmission, ServerState, Store,
};
