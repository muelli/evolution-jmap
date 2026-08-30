// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP protocol types.
//!
//! Pure data types with serde implementations — no I/O. Wire format follows:
//!
//! - [RFC 8620] JMAP core: session object, request/response envelopes,
//!   `Id`/`State` primitives, method-level and set-level errors.
//! - [RFC 8621] JMAP for Mail: `Mailbox`, `Email`, `Identity`,
//!   `EmailSubmission` (feature `mail`).
//! - [RFC 9610] JMAP for Contacts: `AddressBook`, `ContactCard` carrying
//!   JSContact ([RFC 9553]) cards (feature `contacts`).
//! - [draft-ietf-jmap-calendars] JMAP for Calendars: `Calendar`,
//!   `CalendarEvent` carrying JSCalendar ([RFC 8984]) events
//!   (feature `calendars`).
//! - [RFC 9670] JMAP Sharing: the `Principal` object and its `/get`/`/query`
//!   methods (feature `principals`).
//! - [RFC 9425] JMAP for Quotas: the `Quota` object and `/query` filter.
//! - [RFC 9404] JMAP Blob Management: `Blob/get` and `Blob/upload`.
//! - [RFC 9265] JMAP for Sieve Scripts: `SieveScript`, `SieveScript/validate`.
//! - [RFC 8887] JMAP Subprotocol for WebSocket: `Request`, `Response`,
//!   `RequestError`, `WebSocketPushEnable`, `WebSocketPushDisable`.
//!
//! [RFC 8620]: https://www.rfc-editor.org/rfc/rfc8620
//! [RFC 8621]: https://www.rfc-editor.org/rfc/rfc8621
//! [RFC 9610]: https://www.rfc-editor.org/rfc/rfc9610
//! [RFC 9553]: https://www.rfc-editor.org/rfc/rfc9553
//! [RFC 8984]: https://www.rfc-editor.org/rfc/rfc8984
//! [draft-ietf-jmap-calendars]: https://datatracker.ietf.org/doc/draft-ietf-jmap-calendars/
//! [RFC 9670]: https://www.rfc-editor.org/rfc/rfc9670
//! [RFC 9425]: https://www.rfc-editor.org/rfc/rfc9425
//! [RFC 9404]: https://www.rfc-editor.org/rfc/rfc9404
//! [RFC 9265]: https://www.rfc-editor.org/rfc/rfc9265
//! [RFC 8887]: https://www.rfc-editor.org/rfc/rfc8887

pub mod blob;
#[cfg(feature = "calendars")]
pub mod calendars;
#[cfg(feature = "contacts")]
pub mod contacts;
pub mod error;
pub mod id;
#[cfg(feature = "mail")]
pub mod mail;
pub mod methods;
#[cfg(feature = "principals")]
pub mod principals;
pub mod push;
pub mod quota;
pub mod request;
pub mod response;
pub mod session;
pub mod sieve;
pub mod state;
#[cfg(feature = "calendars")]
pub mod tasks;
pub mod websocket;

pub use id::Id;
pub use state::{State, UtcDate};
