// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! Run a stateful in-memory mock JMAP server.
//! This crate covers the JMAP surface used by integration tests.
//! Tests inspect state through `Arc<Mutex<..>>`.
//! The server uses plain HTTP on `127.0.0.1` with `tiny_http`.
//! `/eventsource` runs on a separate thread.

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
    DEFAULT_SIZE_REQUEST, DEFAULT_SIZE_UPLOAD, MockServer, MockServerBuilder,
};
pub use state::{
    AccountState, Blob, Change, ChangeKind, EventSourceHub, RecordedSchedulingMessage,
    RecordedSubmission, ServerState, Store,
};
