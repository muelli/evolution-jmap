<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# evolution-jmap-proto

Pure data types and serde serialization and deserialization for the JMAP
(JSON Meta Application Protocol) wire format.

This crate contains data models and JSON wire codecs with zero I/O and zero
external network dependencies.

## Supported Specifications

The crate implements wire types following standard RFCs and active IETF drafts:

- **RFC 8620**: JMAP Core (Session resource, Request and Response envelopes,
  Invocation arrays, ResultReference back-references, Id and State primitives,
  MethodError and SetError taxonomy).
- **RFC 8621**: JMAP for Mail (Mailbox, Email, Identity, EmailSubmission,
  SearchSnippet).
- **RFC 9610**: JMAP for Contacts (AddressBook, ContactCard carrying JSContact
  RFC 9553 cards).
- **draft-ietf-jmap-calendars**: JMAP for Calendars (Calendar, CalendarEvent
  carrying JSCalendar RFC 8984 events, ParticipantIdentity,
  CalendarEventNotification).
- **RFC 9670**: JMAP Sharing (Principal, ShareNotification, availability
  calculation).
- **RFC 9404**: JMAP Blob Management (Blob/get, Blob/upload).
- **RFC 9425**: JMAP for Quotas (Quota object, Quota/query filter).
- **RFC 9661**: JMAP for Sieve Scripts (SieveScript, SieveScript/validate).
- **RFC 8887**: JMAP Subprotocol for WebSocket (WebSocket request/response
  wrappers, push enablement).
- **RFC 9749**: JMAP Web Push VAPID (WebPushVapidCapability).
- **draft-ietf-jmap-filenode**: JMAP File Storage (FileNode, FileNodeCapability).
- **draft-ietf-jmap-refplus**: JMAP Enhanced Result References (RefPlusCapability).
- **draft-ietf-jmap-metadata**: JMAP Metadata (MetadataCapability,
  MetadataFilterCondition).
- **draft-ietf-jmap-mail-sharing**: JMAP Mail Sharing (MailShareCapability,
  MailboxRights).

## Minimal Working Example

```rust
use jmap_proto::id::Id;
use jmap_proto::request::Request;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let request = Request::new([
        "urn:ietf:params:jmap:core",
        "urn:ietf:params:jmap:mail",
    ])
    .call(
        "Core/echo",
        &json!({
            "message": "hello jmap"
        }),
        "c0",
    )?;

    let serialized = serde_json::to_string_pretty(&request)?;
    let deserialized: Request = serde_json::from_str(&serialized)?;

    assert_eq!(deserialized.using.len(), 2);
    assert_eq!(deserialized.method_calls.len(), 1);
    assert_eq!(deserialized.method_calls[0].name, "Core/echo");
    assert_eq!(deserialized.method_calls[0].call_id, "c0");
    assert_eq!(deserialized.method_calls[0].arguments["message"], "hello jmap");

    let account_id = Id::from("acc123");
    assert_eq!(account_id.as_str(), "acc123");
    Ok(())
}
```

## Feature flags

The crate organizes domain models behind Cargo feature flags:

| Feature | Default | Description |
|---|---|---|
| `mail` | Yes | JMAP Mail types (RFC 8621) |
| `contacts` | Yes | JMAP Contacts types (RFC 9610 and RFC 9553) |
| `calendars` | Yes | JMAP Calendars types (draft-ietf-jmap-calendars and RFC 8984) |
| `principals` | Yes | JMAP Sharing and Principals types (RFC 9670; enables `calendars`) |

Core protocol facilities (RFC 8620 session, requests, responses, errors, IDs,
states, blob management, quotas, sieve, and websocket structures) are always
enabled.

## Minimum Supported Rust Version (MSRV)

This crate requires **Rust 1.97** or newer and uses the Rust 2024 edition.

## License

This project is licensed under the **GNU General Public License v3.0 or later**
(`GPL-3.0-or-later`). See the repository root for full license details.

## Scope statement

The scope of this crate follows the project's standing rule: conform strictly
to published standards while being tolerant of real-world server implementations
("do whatever Stalwart does").

- **Pure wire representation**: Types model the JSON representation exchanged
  over HTTP and WebSocket transports.
- **Tolerant deserialization**: Unknown fields are preserved or ignored without
  breaking deserialization; server quirks observed in live implementations
  (such as nullable state fields or omitted optional values) are accepted on input.
- **Standard output**: Serialization emits standard RFC conformant property names
  and structures.

### What is not implemented

- **Network I/O**: This crate contains no HTTP client, connection pooling, or
  socket handling. For a blocking HTTP client using these types, see
  `evolution-jmap-client`.
- **Server dispatch and storage**: No server-side business logic or storage
  layer is provided. For an in-memory mock server for testing, see
  `evolution-jmap-mock`.
- **Legacy format conversion**: Direct translation between iCalendar (RFC 5545)
  and JSCalendar (RFC 8984), or vCard (RFC 6350) and JSContact (RFC 9553), is
  handled in `evolution-jmap-ical` and `evolution-jmap-vcard`.

## Documentation gaps (TODO)

Module-level documentation covers protocol relationships and RFC citations.
Granular field-level doc comments across all generated data models are being
documented iteratively. Gaps are catalogued in the workspace documentation
audit and refined module by module.
