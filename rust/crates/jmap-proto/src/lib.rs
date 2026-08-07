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
//!
//! [RFC 8620]: https://www.rfc-editor.org/rfc/rfc8620
//! [RFC 8621]: https://www.rfc-editor.org/rfc/rfc8621
//! [RFC 9610]: https://www.rfc-editor.org/rfc/rfc9610
//! [RFC 9553]: https://www.rfc-editor.org/rfc/rfc9553
//! [RFC 8984]: https://www.rfc-editor.org/rfc/rfc8984
//! [draft-ietf-jmap-calendars]: https://datatracker.ietf.org/doc/draft-ietf-jmap-calendars/

#[cfg(feature = "contacts")]
pub mod contacts;
pub mod error;
pub mod id;
#[cfg(feature = "mail")]
pub mod mail;
pub mod methods;
pub mod request;
pub mod response;
pub mod session;
pub mod state;

pub use id::Id;
pub use state::{State, UtcDate};
