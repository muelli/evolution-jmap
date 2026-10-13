<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# RFC Example Conformance and Wire Model Verification

This document records the extraction, fixture mapping, and protocol conformance verification of the specifications' own JSON examples against the wire types in `jmap-proto`.

## 1. Document Versions and Specifications

| Specification | Status | Date / Version | Source |
|---|---|---|---|
| RFC 8620 | Standards Track | July 2019, ISSN: 2070-1721 | https://www.rfc-editor.org/rfc/rfc8620.txt |
| RFC 8621 | Standards Track | August 2019, ISSN: 2070-1721 | https://www.rfc-editor.org/rfc/rfc8621.txt |
| RFC 9610 | Standards Track | December 2024, ISSN: 2070-1721 | https://www.rfc-editor.org/rfc/rfc9610.txt |
| RFC 9553 | Standards Track | May 2024, ISSN: 2070-1721 | https://www.rfc-editor.org/rfc/rfc9553.txt |
| RFC 8984 | Standards Track | July 2021, ISSN: 2070-1721 | https://www.rfc-editor.org/rfc/rfc8984.txt |
| draft-ietf-jmap-calendars-32 | IETF Internet-Draft | October 2026, draft-ietf-jmap-calendars-32 | https://datatracker.ietf.org/doc/draft-ietf-jmap-calendars/ |

## 2. RFC 8620 Core JMAP Examples (July 2019)

RFC 8620 defines the fundamental JMAP protocol data structures, envelopes, problem details, and push notifications.

### 2.1 Catalogued Fixtures

31 distinct JSON examples across 10 sections were extracted and stored under `rust/crates/jmap-proto/tests/fixtures/rfc/rfc8620/`:

1. `2.1_session.json`: Complete JMAP Session resource advertising core, mail, contacts, and custom vendor capabilities with primary and secondary accounts.
2. `3.3.1_request.json`: Standard Request envelope carrying `using` and multiple method calls.
3. `3.4.1_response.json`: Standard Response envelope carrying multiple method responses and `sessionState`.
4. `3.6.1_unknown_capability.json`: Request-level problem details (`urn:ietf:params:jmap:error:unknownCapability`).
5. `3.6.1_limit.json`: Request-level problem details (`urn:ietf:params:jmap:error:limit`) with `limit: "maxSizeRequest"`.
6. `3.6.2_method_error.json`: Method-level `error` invocation with `type: "unknownMethod"`.
7. `3.7_simple_backref_request.json`: Method call chaining `Foo/changes` and `Foo/get` with a `#ids` `ResultReference`.
8. `3.7_thread_backref_request.json`: Four-stage method call pipeline chaining `Email/query`, `Email/get`, `Thread/get`, and `Email/get` with JSON Pointer backreferences.
9. `3.7_thread_backref_response.json`: Response containing partial execution results for query and get methods.
10. `3.7_resolved_get_request.json`: Resolved `Email/get` arguments evaluated from backreferences.
11. `4.1_echo_request.json`: `Core/echo` method call invocation.
12. `4.1_echo_response.json`: `Core/echo` method response invocation.
13. `5.7_query_get_request.json`: Standard `Todo/query` and `Todo/get` chained request.
14. `5.7_query_get_response.json`: Standard `Todo/query` and `Todo/get` responses.
15. `5.7_set_update_full_request.json`: Standard `Todo/set` call with whole-object update.
16. `5.7_set_update_patch_request.json`: Standard `Todo/set` call with JSON Pointer patch syntax.
17. `5.7_set_update_response.json`: Standard `Todo/set` update response with server-calculated properties.
18. `5.7_set_create_subtask_request.json`: Standard `Todo/set` call referencing creation id `"#k15"` in foreign key property.
19. `5.7_changes_request.json`: Standard `Todo/changes` and `Todo/queryChanges` request.
20. `5.7_changes_response.json`: Standard `Todo/changes` and `Todo/queryChanges` response.
21. `5.7_copy_request.json`: Standard `Todo/copy` cross-account request with `onSuccessDestroyOriginal: true`.
22. `5.7_copy_response.json`: Standard `Todo/copy` response with multiple responses (`Todo/copy` and implicit `Todo/set`) for the same call id.
23. `7.1.1_state_change.json`: EventSource `StateChange` push notification across multiple accounts.
24. `7.2.3_get_request.json`: `PushSubscription/get` request with `ids: null`.
25. `7.2.3_get_response.json`: `PushSubscription/get` response with subscription records.
26. `7.2.3_set_create_request.json`: `PushSubscription/set` create call with `deviceClientId` and `url`.
27. `7.2.3_set_created_response.json`: `PushSubscription/set` created response with server-capped `expires`.
28. `7.2.3_verification.json`: Server-initiated `PushVerification` challenge POST payload.
29. `7.2.3_set_update_code_request.json`: `PushSubscription/set` update with verification code.
30. `7.2.3_set_update_expires_request.json`: `PushSubscription/set` update extending expiry timestamp.
31. `7.2.3_set_update_expires_response.json`: `PushSubscription/set` updated response confirming expiry extension.

### 2.2 Specification Errata Audited

The IETF RFC Errata database for RFC 8620 was audited and applied:

- **Errata ID 5791 (Technical, Verified)**: In §2.1, the example Session resource response erroneously named `maxConcurrentRequest` instead of `maxConcurrentRequests`, and was missing a trailing comma after `"urn:ietf:params:jmap:mail": {}`. Corrected in fixture `2.1_session.json`.
- **Errata ID 6603 (Technical, Reported)**: Section 5.5 `upToId` specification clarification.
- **Errata ID 6604 (Technical, Reported)**: Section 5.6 `/queryUpdates` client cache positioning clarification.
- **Errata ID 6605 (Technical, Reported)**: Section 5.5 `/query` position value clarification when no ids are returned.
- **Errata ID 6606 (Technical, Reported)**: Section 3.3 `using` list clarification for JMAP core capability.
- **Errata ID 6607 (Technical, Reported)**: Section 5.3 `/set` create and destroy referencing in the same request.

### 2.3 Protocol Findings and Model Refinements

1. **RFC 8620 §7.2 `PushSubscription/get` privacy requirement**:
   Section 7.2 states:
   > "As the 'url' and 'keys' properties may contain data that is private to a particular device, the values for these properties MUST NOT be returned. If the 'properties' argument is null or omitted, the server MUST default to all properties excluding these two."

   In accordance with this rule, the official example in RFC 8620 §7.2.3 returns `PushSubscription` objects lacking the `url` property. Our `PushSubscription` wire model previously declared `pub url: String` as non-optional, causing deserialization of conformant `PushSubscription/get` responses to fail with `missing field url`.
   Resolution: Updated `PushSubscription.url` to `Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` and added `pub verification_code: Option<String>` to model the full RFC 8620 §7.2 specification.

2. **RFC 72-column string line folding**:
   In plain-text RFC documents, URLs and description strings that exceed 72 columns are formatted across multiple lines (e.g. `downloadUrl` and `eventSourceUrl` in §2.1, and `detail` in §3.6.1.1). Unfolded in the extracted fixtures to form valid JSON string literals.

3. **RFC text ellipsis notation**:
   Informational RFC examples use ellipsis `...` to represent omitted properties or additional array elements (e.g. §2.1 `accountCapabilities`, §3.7 list responses, §5.7 `list` and `Todo/set` responses). Normalized in fixtures into valid JSON structures without altering any field modeled by `jmap-proto`.

## 3. RFC 8621 JMAP for Mail Examples (August 2019)

RFC 8621 defines the data model for synchronizing email data with a JMAP server, covering Mailboxes, Threads, Emails, SearchSnippets, Identities, and EmailSubmissions.

### 3.1 Catalogued Fixtures

31 distinct JSON examples across 7 sections were extracted and stored under `rust/crates/jmap-proto/tests/fixtures/rfc/rfc8621/`:

1. `2.6_get_request.json`: `Mailbox/get` request fetching all mailboxes with `ids: null` (§2.6).
2. `2.6_get_response.json`: `Mailbox/get` response returning Inbox with `myRights` and `isSubscribed` (§2.6).
3. `2.6_changes_request.json`: Pipelined request chaining `Mailbox/changes` and back-referenced `Mailbox/get` calls (§2.6).
4. `2.6_changes_response.json`: Pipelined response returning `Mailbox/changes` and updated Mailbox data (§2.6).
5. `2.6_set_request.json`: `Mailbox/set` request renaming one mailbox and destroying another (§2.6).
6. `2.6_set_response.json`: `Mailbox/set` response returning updated mailbox and forbidden error for destroy (§2.6).
7. `3.1.1_get_request.json`: `Thread/get` request with explicit ids (§3.1.1).
8. `3.1.1_get_response.json`: `Thread/get` response returning thread records with `emailIds` (§3.1.1).
9. `4.1.2.3_addresses.json`: Header field parsed form `EmailAddress[]` with display names and email addresses (§4.1.2.3).
10. `4.1.2.4_grouped_addresses.json`: Header field parsed form `EmailAddressGroup[]` preserving group information (§4.1.2.4).
11. `4.2.1_get_request.json`: `Email/get` request specifying body properties, `fetchHTMLBodyValues`, and `maxBodyValueBytes` (§4.2.1).
12. `4.2.1_get_response.json`: `Email/get` response with `htmlBody`, truncated `bodyValues`, and `notFound` (§4.2.1).
13. `4.10_initial_sync_request.json`: Initial mailbox sync pipeline chaining `Email/query`, `Email/get`, `Thread/get`, and `Email/get` (§4.10).
14. `4.10_initial_sync_response.json`: Initial mailbox sync response returning query results, thread ids, thread records, and email summaries (§4.10).
15. `4.10_set_update_request.json`: `Email/set` request updating keywords patch (`keywords/$seen: null` to mark unread) (§4.10).
16. `4.10_set_update_response.json`: `Email/set` response confirming keywords update (§4.10).
17. `4.10_resync_request.json`: Resynchronization request chaining `Email/changes` and `Email/queryChanges` (§4.10).
18. `4.10_resync_response.json`: Resynchronization response returning created, updated, and destroyed email ids along with query delta items (§4.10).
19. `4.10_set_create_text_request.json`: `Email/set` request creating a plaintext draft with `bodyStructure` and `bodyValues` (§4.10).
20. `4.10_set_create_text_response.json`: `Email/set` response returning created draft metadata (§4.10).
21. `4.10_set_create_html_request.json`: `Email/set` request creating a multipart/alternative HTML draft and destroying the previous draft (§4.10).
22. `4.10_set_create_html_response.json`: `Email/set` response confirming HTML draft creation and old draft destruction (§4.10).
23. `4.10_copy_request.json`: Cross-account `Email/copy` request with `onSuccessDestroyOriginal: true` (§4.10).
24. `4.10_copy_response.json`: Cross-account `Email/copy` response with implicit `Email/set` destroy response sharing call id (§4.10).
25. `5.2_get_request.json`: `SearchSnippet/get` request filtering by text query across email ids (§5.2).
26. `5.2_get_response.json`: `SearchSnippet/get` response returning match snippets with `<mark>` highlight tags (§5.2).
27. `6.4_get_request.json`: `Identity/get` request invocation (§6.4).
28. `6.4_get_response.json`: `Identity/get` response invocation returning identities with text and HTML signatures (§6.4).
29. `7.5.1_set_create_request.json`: `EmailSubmission/set` create request with `envelope` and `onSuccessUpdateEmail` patch (§7.5.1).
30. `7.5.1_set_created_response.json`: `EmailSubmission/set` response with implicit `Email/set` update response (§7.5.1).
31. `7.5.1_set_error_response.json`: `EmailSubmission/set` error response returning localized `forbiddenToSend` failure (§7.5.1).

### 3.2 Specification Errata Audited

The IETF RFC Errata database for RFC 8621 was audited:

- **Errata Search**: Queried `https://errata.rfc-editor.org/search/?rfc_number=8621`. No matching errata found. RFC 8621 has zero reported or verified errata.

### 3.3 Protocol Findings and Model Refinements

1. **RFC 8621 §4.2 `Email/get` arguments**:
   RFC 8621 §4.2 specifies `Email/get` as extending standard `/get` with `bodyProperties`, `fetchTextBodyValues`, `fetchHTMLBodyValues`, `fetchAllBodyValues`, and `maxBodyValueBytes`. In the specification's own example in §4.2.1, `accountId` is omitted in the request. Added typed `EmailGetRequest` with optional `account_id` and complete body-fetching argument options, allowing seamless typed parsing and roundtrip fidelity.

2. **RFC line folding in string literals**:
   Plain-text RFC formatting folds lines exceeding 72 characters, splitting string literals across lines in §4.2.1 (`bodyValues`), §4.10 (`preview`, `bodyValues` text and HTML contents), §5.2 (`preview`), §6.4 (`htmlSignature`), and §7.5.1 (`description`). Unfolded in fixtures into valid JSON strings.

3. **Ellipsis notation in RFC examples**:
   RFC 8621 uses ellipsis `...` to indicate omitted fields in responses (§2.6 `list`, §4.10 `ids`, `list`, and `SetResponse` omitted maps, §5.2 `emailIds` and `list`, §7.5.1 `rcptTo`). Normalized in fixtures into syntactically valid JSON structures while preserving all fields modeled by `jmap-proto`.

## 4. RFC 9610 JMAP for Contacts Examples (December 2024)

RFC 9610 defines the data model for synchronizing address books and contact cards with a JMAP server, mapping JSContact cards into the JMAP data model with AddressBook and ContactCard types.

### 4.1 Catalogued Fixtures

4 distinct JSON examples across Section 4 were extracted and stored under `rust/crates/jmap-proto/tests/fixtures/rfc/rfc9610/`:

1. `4.1_get_request.json`: Method calls pipeline requesting `AddressBook/get` and `ContactCard/get` (§4.1, Figure 1).
2. `4.1_get_response.json`: Method responses returning AddressBook list with `myRights` and `isDefault`, alongside ContactCard records with structured name components and emails (§4.1, Figure 2).
3. `4.2_set_request.json`: `AddressBook/set` invocation setting `onSuccessSetIsDefault` to reassign the default address book (§4.2, Figure 3).
4. `4.2_set_response.json`: `AddressBook/set` response confirming update of default address books (§4.2, Figure 4).

### 4.2 Specification Errata Audited

The IETF RFC Errata database for RFC 9610 was audited:

- **Errata Search**: Queried `https://errata.rfc-editor.org/search/?rfc_number=9610`. Zero errata reported.

### 4.3 Protocol Findings and Model Refinements

1. **RFC 9610 §2.3 `AddressBook/set` arguments**:
   Section 2.3 defines two additional request arguments on `AddressBook/set`:
   - `onDestroyRemoveContents: Option<bool>`
   - `onSuccessSetIsDefault: Option<Id>`

   Introduced `AddressBookSetRequest` wrapping `SetRequest<AddressBook>` to cleanly model these arguments while preserving serialization fidelity and builder ergonomics.

2. **RFC 9610 §4.2 Figure 4 erratum in example response**:
   In Figure 4 of RFC 9610 §4.2, the example `AddressBook/set` response places `"oldState": "~4144"` and `"newState": "~4148"` inside the `"updated"` map rather than at the top level of the `SetResponse` object. In standard JMAP (RFC 8620 §5.3), `oldState` and `newState` are top-level properties of `SetResponse`. In `rfc_examples.rs`, this fixture is verified through invocation inspection, noting the specification erratum.

## 5. RFC 9553 JSContact Card Examples (May 2024)

RFC 9553 defines JSContact, a JSON-based data format for contact card data.

### 5.1 Catalogued Fixtures

42 distinct JSON examples across 20 sections were extracted and stored under `rust/crates/jmap-proto/tests/fixtures/rfc/rfc9553/` (excluding Figures 2 and 5 which define ABNF grammar rules):

1. `1.4.5_phonetic.json`: Phonetic name pronunciation using the International Phonetic Alphabet (IPA) (§1.4.5, Figure 1).
2. `1.6.2_vendor_properties.json`: Custom vendor-specific property extensions with domain prefixing (§1.6.2, Figure 3).
3. `1.6.2_vendor_value.json`: Vendor-specific value for the `kind` property (§1.6.2, Figure 4).
4. `2.1_basic_card.json`: Minimal valid Card object for an individual with structured name components (§2.1, Figure 6).
5. `2.1.2_version.json`: Card `version` property (§2.1.2, Figure 7).
6. `2.1.3_created.json`: Card `created` timestamp (§2.1.3, Figure 8).
7. `2.1.4_kind.json`: Card `kind` property set to `individual` (§2.1.4, Figure 9).
8. `2.1.5_language.json`: Card `language` tag (§2.1.5, Figure 10).
9. `2.1.6_members.json`: Group Card with `members` set (§2.1.6, Figure 11).
10. `2.1.7_prod_id.json`: Card `prodId` software identifier (§2.1.7, Figure 12).
11. `2.1.8_related_to.json`: Card `relatedTo` relation mappings (§2.1.8, Figure 13).
12. `2.1.9_uid.json`: Card mandatory `uid` URI (§2.1.9, Figure 14).
13. `2.1.10_updated.json`: Card `updated` timestamp (§2.1.10, Figure 15).
14. `2.2.1_name_two_words.json`: Name component with a multi-word surname (§2.2.1, Figure 16).
15. `2.2.1_name_second_surname.json`: Name with secondary surname (§2.2.1, Figure 17).
16. `2.2.1_name_full.json`: Formatted `full` name property (§2.2.1, Figure 18).
17. `2.2.1_name_sort_as.json`: Name sort order override mappings (§2.2.1, Figure 19).
18. `2.2.1_name_phonetic_localizations.json`: Card with Chinese characters and Cantonese Jyutping phonetic localization patches (§2.2.1, Figure 20).
19. `2.2.2_nicknames.json`: Nicknames map with familiar names (§2.2.2, Figure 21).
20. `2.2.3_organizations.json`: Organizational affiliation with division and department units and sort key (§2.2.3, Figure 22).
21. `2.2.4_speak_to_as.json`: Addressing preferences with grammatical gender and ranked pronouns (§2.2.4, Figure 23).
22. `2.2.5_titles.json`: Professional titles and roles linked to organizations (§2.2.5, Figure 24).
23. `2.3.1_emails.json`: Multiple email addresses with contexts and preferences (§2.3.1, Figure 25).
24. `2.3.2_online_services.json`: Online service usernames and protocol URIs (§2.3.2, Figure 26).
25. `2.3.3_phones.json`: Telephone numbers with features and types (§2.3.3, Figure 27).
26. `2.3.4_preferred_languages.json`: Language communication preferences ranked by context (§2.3.4, Figure 28).
27. `2.4.1_calendars.json`: Calendaring URIs for personal calendar and free-busy lookups (§2.4.1, Figure 29).
28. `2.4.2_scheduling_addresses.json`: Calendar scheduling addresses with ranking (§2.4.2, Figure 30).
29. `2.5.1_address_usa.json`: Structured postal address in the United States (§2.5.1, Figure 31).
30. `2.5.1_address_thailand.json`: Structured postal address in Thailand with subdistrict and district components (§2.5.1, Figure 32).
31. `2.5.1_address_tokyo.json`: Postal address in Tokyo alongside its localized Japanese representation (§2.5.1, Figure 33).
32. `2.6.1_crypto_keys_external.json`: External cryptographic key reference (§2.6.1, Figure 34).
33. `2.6.1_crypto_keys_embedded.json`: Inlined cryptographic public key with base64 data URI (§2.6.1, Figure 35).
34. `2.6.2_directories.json`: Directory service URLs for user lookup (§2.6.2, Figure 36).
35. `2.6.3_links.json`: Contact resource link with custom label and preference (§2.6.3, Figure 37).
36. `2.6.4_media.json`: Media attachments including sound pronunciation, logo, and photo data URI (§2.6.4, Figure 38).
37. `2.7.1_localization_top_level.json`: Top-level property localization using JSON patch paths (§2.7.1, Figure 39).
38. `2.7.1_localization_nested.json`: Nested property localization for name and titles (§2.7.1, Figure 40).
39. `2.8.1_anniversaries.json`: Anniversaries with partial calendar dates and timestamp (§2.8.1, Figure 41).
40. `2.8.2_keywords.json`: Keyword tags map (§2.8.2, Figure 42).
41. `2.8.3_notes.json`: Free-text notes with authorship and creation timestamp (§2.8.3, Figure 43).
42. `2.8.4_personal_info.json`: Personal hobbies, interests, and expertise with proficiency levels (§2.8.4, Figure 44).

### 5.2 Specification Errata Audited

The IETF RFC Errata database for RFC 9553 was audited:

- **Errata ID 8263 (Technical, Reported)**: Section 2.2.1 Figure 20 omitted required Card properties (`uid`, `version`). The fixture preserves the example as published.
- **Errata ID 8264 (Editorial, Reported)**: Section 2.7.1 Figure 39 bracket formatting consistency.
- **Errata ID 8265 (Editorial, Reported)**: Section 2.2.1 Figure 18 property indentation consistency.
- **Errata ID 8266 (Technical, Reported)**: Section 2.6.2 Figure 36 omitted the closing brace `}` on the `"directories"` map. Fixed in fixture `2.6.2_directories.json`.
- **Errata ID 8267 (Technical, Reported)**: Section 2.6.1 CryptoKey kind enumeration clarification.

### 5.3 Protocol Findings and Model Refinements

1. **RFC 9553 §2.4.2 `schedulingAddresses`**:
   The specification defines `schedulingAddresses: Id[SchedulingAddress] (optional)`. Added `SchedulingAddress` struct and modeled `scheduling_addresses: Option<BTreeMap<String, SchedulingAddress>>` on `ContactCard` with builder method `with_scheduling_address`.

2. **RFC 9553 §2.2.4 `speakToAs.pronouns`**:
   The specification models `pronouns: Id[Pronouns] (optional)`, where each `Pronouns` object carries `pronouns: String`, optional `contexts`, and optional `pref`. Added `Pronouns` struct and introduced `PronounsValue` enum supporting both the standard map of `Pronouns` and shorthand plain strings.

3. **RFC 9553 §2.1 top-level Card properties**:
   Added typed fields on `ContactCard` for `language: Option<String>`, `members: Option<BTreeMap<String, bool>>`, and `prod_id: Option<String>`, ensuring all standard JSContact core properties deserialize cleanly.

4. **Multiline data URI folding in Figure 35**:
   Figure 35 embeds an RSA public key in a base64 data URI split across multiple lines. The multiline data URI was unfolded into a single continuous URI string in `2.6.1_crypto_keys_embedded.json`.

## 6. RFC 8984 JSCalendar: A JSON Representation of Calendar Data (July 2021)

RFC 8984 defines the data model and JSON representation for calendar events, tasks, and groups of calendar objects.

### 6.1 Catalogued Fixtures

10 distinct JSON examples across Section 6 were extracted and stored under `rust/crates/jmap-proto/tests/fixtures/rfc/rfc8984/`:

1. `6.1_simple_event.json`: Simple calendar event with mandatory properties (`@type`, `uid`, `updated`, `title`, `start`, `duration`) (§6.1).
2. `6.2_simple_task.json`: Simple task with due date (`due`) and title (§6.2).
3. `6.3_group.json`: Group object aggregating multiple events and tasks under a common collection (§6.3).
4. `6.4_event_with_alert.json`: Event with absolute and relative alerts specifying audio and display actions (§6.4).
5. `6.5_task_with_recurrence.json`: Recurring task with recurrence rule (`RecurrenceRule`), priority, and progress status (§6.5).
6. `6.6_event_with_participants.json`: Meeting event with organizer, attendees, and RSVP roles and participation statuses (§6.6).
7. `6.7_multilingual_event.json`: Multilingual event with localized title and description using JSON patch representations (§6.7).
8. `6.8_time_zones.json`: Event with multiple custom time zone definitions (`timeZones`) and timeZone rules (§6.8).
9. `6.9_recurring_event_exceptions.json`: Recurring event with recurrence rule, recurrence overrides (`recurrenceOverrides`), and excluded dates (§6.9).
10. `6.10_recurring_event_participants.json`: Recurring event with instance-specific participant adjustments (§6.10).

### 6.2 Specification Errata Audited

The IETF RFC Errata database for RFC 8984 was audited:

- **Errata ID 6872 (Technical, Verified)**: Section 5.1.2 private event shared properties clarification.
- **Errata ID 6873 (Technical, Verified)**: Section 4.3.4 floating time `recurrenceIdTimeZone` clarification.
- **Errata ID 8028 (Technical, Reported)**: Section 6.3 Group example incorrectly used `"name"` instead of `"title"` for Group title property.
- **Errata ID 8461 (Technical, Reported)**: Section 4.5.3 `offsetFrom` and `offsetTo` type definition string UTC offset format clarification.

### 6.3 Protocol Findings and Model Refinements

1. **RFC 8984 §6.5 Task optional `uid`**:
   The RFC 8984 §6.5 example (`6.5_task_with_recurrence.json`) omits the `uid` property. Although `uid` is standard on JSCalendar objects, tasks embedded as subcomponents or in client drafts may omit `uid`. Updated `Task.uid` in `tasks.rs` with `#[serde(default, skip_serializing_if = "String::is_empty")]` to enable clean deserialization.

2. **RFC 8984 §5.3.1 Group `entries` array representation**:
   RFC 8984 §5.3.1 defines `entries: (Event|Task)[]` as an array of JSCalendar objects. In `calendars.rs`, `CalendarGroup.entries` was previously typed as `BTreeMap<String, Value>`. Introduced the untagged enum `GroupEntries` (`Array(Vec<Value>)` and `Map(BTreeMap<String, Value>)`), enabling conformant array parsing while preserving backwards compatibility with existing map representations.

## 7. draft-ietf-jmap-calendars-32: JMAP for Calendars (October 2026)

draft-ietf-jmap-calendars defines the data model and methods for synchronizing calendar objects and managing calendars via JMAP.

### 7.1 Catalogued Fixtures

20 distinct JSON examples across Section 5, 6, and 7 were extracted and stored under `rust/crates/jmap-proto/tests/fixtures/rfc/jmap_calendars/` (excluding Figure 14 in §5.9.8 which is an ASCII text user interface layout diagram):

1. `5.9.1_calendar_event.json`: Standard `CalendarEvent` resource with participants and recurrence (§5.9.1, Figure 1).
2. `5.9.2_get_request.json`: `Calendar/get` request fetching calendars (§5.9.2, Figure 2).
3. `5.9.2_get_response.json`: `Calendar/get` response returning calendar records with rights and default flags (§5.9.2, Figure 3).
4. `5.9.3_set_request.json`: `Calendar/set` create and rename request (§5.9.3, Figure 4).
5. `5.9.3_set_response.json`: `Calendar/set` created and updated response (§5.9.3, Figure 5).
6. `5.9.4_event_get_request.json`: `CalendarEvent/get` request fetching event properties (§5.9.4, Figure 6).
7. `5.9.4_event_get_response.json`: `CalendarEvent/get` response returning calendar events (§5.9.4, Figure 7).
8. `5.9.5_event_query_request.json`: `CalendarEvent/query` filter request with date range and sort (§5.9.5, Figure 8).
9. `5.9.5_event_query_response.json`: `CalendarEvent/query` response returning matching event ids (§5.9.5, Figure 9).
10. `5.9.6_event_set_request.json`: `CalendarEvent/set` request creating an event with recurrence rules (§5.9.6, Figure 10).
11. `5.9.6_event_set_response.json`: `CalendarEvent/set` created response with calculated properties (§5.9.6, Figure 11).
12. `5.9.7_event_update_request.json`: `CalendarEvent/set` update request patching participant RSVP status (§5.9.7, Figure 12).
13. `5.9.7_event_update_response.json`: `CalendarEvent/set` updated response confirming patch (§5.9.7, Figure 13).
14. `5.9.8_event_query_freebusy_request.json`: `CalendarEvent/query` request for free/busy availability calculation (§5.9.8, Figure 15).
15. `5.9.8_event_query_freebusy_response.json`: `CalendarEvent/query` free/busy response (§5.9.8, Figure 16).
16. `5.9.9_event_copy_request.json`: `CalendarEvent/copy` cross-account request with `onSuccessDestroyOriginal: true` (§5.9.9, Figure 17).
17. `5.9.9_event_copy_response.json`: `CalendarEvent/copy` response with implicit `CalendarEvent/set` destroy response (§5.9.9, Figure 18).
18. `6.4_calendar_alert.json`: EventSource `CalendarAlert` push notification (§6.4, Figure 19).
19. `7.1_notification_get_request.json`: `CalendarEventNotification/get` request (§7.1, Figure 20).
20. `7.1_notification_get_response.json`: `CalendarEventNotification/get` response with change details (§7.1, Figure 21).

### 7.2 Specification Errata and Example Normalization

- **Specification Status**: As an active Internet-Draft submitted for publication, official RFC Errata are not yet applicable. Issues and review notes from the IETF CalEXT working group were audited.
- **Ellipsis Normalization**: In Figure 8 and Figure 10, informational ellipsis tokens (`...`) in non-normative sample contexts were normalized into standard JSON per RFC 8620 convention.

### 7.3 Protocol Findings and Model Refinements

1. **Section 4.3 `Calendar/set` arguments**:
   Section 4.3 defines two additional request arguments on `Calendar/set`: `onDestroyRemoveContents: Option<bool>` and `onSuccessSetIsDefault: Option<Id>`. Added `CalendarSetRequest` wrapping `SetRequest<Calendar>` with helper constructors and builder methods.

2. **Section 6.4 `CalendarAlert` push notifications**:
   Section 6.4 defines the `CalendarAlert` object pushed to clients over the event source stream when an event alarm triggers. Added `CalendarAlert` struct to `calendars.rs` modeling `account_id`, `calendar_event_id`, `uid`, `recurrence_id`, `title`, and `trigger`.

