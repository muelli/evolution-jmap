<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-proto`

## `src/id.rs`

- Zero missed mutants (9 of 9 generated mutants caught by test suites).

## `src/state.rs`

- Zero missed mutants (14 of 14 generated mutants caught by test suites).

## `src/blob.rs`

- Zero missed mutants (28 of 28 viable mutants caught by test suites, 21 unviable due to types without Default).

## `src/error.rs`

- Zero missed mutants (5 unviable mutants generated due to types without Default: RequestError, MethodError, and SetError).

## `src/metadata.rs`

- Zero missed mutants (15 of 15 viable mutants caught by test suites, 1 unviable due to MetadataTextFilter without Default).

## `src/quota.rs`

- Zero missed mutants (11 of 11 generated mutants caught by test suites).

## `src/push.rs`

- Zero missed mutants (12 of 12 viable mutants caught by test suites, 6 unviable due to types without Default: PushVerification and StateChange).

## `src/request.rs`

- Zero missed mutants (3 of 3 viable mutants caught by test suites, 6 unviable due to types without Default: Request and Invocation, unbounded generic in Invocation::parse, and lack of Default on Serializer::Ok).

## `src/response.rs`

- Zero missed mutants (2 of 2 viable mutants caught by test suites, 3 unviable due to types without Default: Response and Invocation).

## `src/session.rs`

- Zero missed mutants (99 of 99 viable mutants caught by test suites, 11 unviable due to types without Default: Session and Account).

## `src/methods.rs`

- Zero missed mutants (8 of 8 viable mutants caught by test suites, 64 unviable due to types without Default: GetRequest, GetResponse, SetRequest, SetResponse, QueryRequest, QueryResponse, ChangesRequest, UploadResponse, ChangesResponse, QueryChangesRequest, QueryChangesResponse, CopyRequest, CopyResponse, and BlobCopyResponse).

## `src/filenode.rs`

- Zero missed mutants (21 of 21 viable mutants caught by test suites, 10 unviable due to FileNode without Default).

## `src/websocket.rs`

- Zero missed mutants (7 of 7 viable mutants caught by test suites, 8 unviable due to types without Default: WebSocketRequest, WebSocketRequestError, and Invocation).

## `src/principals.rs`

- Zero missed mutants (28 of 28 viable mutants caught by test suites, 5 unviable due to types without Default: GetAvailabilityRequest, BusyPeriod, and PrincipalsOwnerCapability).

## `src/sieve.rs`

- Zero missed mutants (18 of 18 viable mutants caught by test suites, 7 unviable due to types without Default: SieveScriptSetRequest, SieveScriptValidateRequest, and SieveScriptValidateResponse).

## `src/tasks.rs`

- Zero missed mutants (38 of 38 viable mutants caught by test suites, 8 unviable due to TaskList without Default).

## `src/calendars.rs`

- Zero missed mutants (143 of 143 viable mutants caught by test suites, 24 unviable due to types without Default: CalendarSetRequest, CalendarEventSetRequest, CalendarEventParseRequest, CalendarEventParseResponse, GetFreeBusyRequest, FreeBusyBlock, GroupEntries, CalendarAlert, CalendarEventSendRequest, CalendarEventSendResponse, ParticipantReply, and ParticipantIdentitySetRequest).

## `src/contacts.rs`

- 1 genuinely equivalent mutant:
  - `ContactCard::simple`: deleting field `card_type` from the struct expression falls back to `..Self::default()`, which sets `card_type` to `Some("Card".to_owned())`, the identical value.
- Zero missed mutants (219 of 219 viable mutants caught by test suites, 11 unviable due to types without Default: AddressBookSetRequest, NameComponent, PronounsValue, ContactCardParseRequest, and ContactCardParseResponse).


