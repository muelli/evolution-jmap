<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# RFC example fixtures

The JSON examples printed in the JMAP and JSContact/JSCalendar specifications,
one file per example, used to check that `jmap-proto`'s wire types accept
what the specifications themselves send.

## Specifications

| Directory | Specification | Version |
|---|---|---|
| `rfc8620/` | RFC 8620, JMAP core | July 2019 |
| `rfc8621/` | RFC 8621, JMAP for Mail | August 2019 |
| `rfc9610/` | RFC 9610, JMAP for Contacts | December 2024 |
| `rfc9553/` | RFC 9553, JSContact | May 2024 |
| `rfc8984/` | RFC 8984, JSCalendar | July 2021 |
| `jmap_calendars/` | draft-ietf-jmap-calendars-32, JMAP for Calendars | 5 October 2026 |
| `rfc9425/` | RFC 9425, JMAP for Quotas | June 2023 |
| `rfc9670/` | RFC 9670, JMAP Sharing (principals) | November 2024 |
| `rfc9661/` | RFC 9661, JMAP for Sieve Scripts | September 2024 |
| `rfc9404/` | RFC 9404, JMAP Blob Management | August 2023 |

The calendars fixtures are taken from draft -32. `jmap_proto::calendars`
cites draft -28 for its property names; -28 and -32 define the same
properties and section structure (only the example values changed), so the
-32 examples check that model.

## How they differ from the specification text

Each file is the example as published, made into valid JSON:

- line 1 is a `// Source:` comment naming the specification, section and
  figure (and the erratum, if one was applied);
- strings folded at 72 columns, or wrapped with RFC 8792 `\` markers, are
  unfolded;
- `...` placeholders for omitted properties or list entries are replaced
  with valid JSON, without touching any field `jmap-proto` models;
- verified or obvious errata are applied where they break parsing (listed
  per specification below); otherwise the example stays as published.

## How `tests/rfc_examples.rs` uses them

`load_rfc_fixture` strips the `//` lines, requires the source note, and
deserialises the rest into the wire type under test. Each test then checks
the source note names the expected section, asserts the fields the example
is about, and re-serialises the value to confirm the round trip is lossless.
Adding an example means adding the file here and a test there.

## 1. RFC 8620 Core JMAP Examples (July 2019)

RFC 8620 defines the fundamental JMAP protocol data structures, envelopes, problem details, and push notifications.

### 1.1 Fixtures

`rfc8620/`, 31 files:

1. `2.1_session.json`: Complete JMAP Session resource advertising core, mail, contacts, and custom vendor capabilities with primary and secondary accounts.
2. `3.3.1_request.json`: Standard Request envelope carrying `using` and multiple method calls.
3. `3.4.1_response.json`: Standard Response envelope carrying multiple method responses and `sessionState`.
4. `3.6.1.1_unknown_capability.json`: Request-level problem details (`urn:ietf:params:jmap:error:unknownCapability`).
5. `3.6.1.1_limit.json`: Request-level problem details (`urn:ietf:params:jmap:error:limit`) with `limit: "maxSizeRequest"`.
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

### 1.2 Errata

RFC 8620 errata:

- **Errata ID 5791 (Technical, Verified)**: In §2.1, the example Session resource response erroneously named `maxConcurrentRequest` instead of `maxConcurrentRequests`, and was missing a trailing comma after `"urn:ietf:params:jmap:mail": {}`. Corrected in fixture `2.1_session.json`.
- **Errata ID 6603 (Technical, Reported)**: Section 5.5 `upToId` specification clarification.
- **Errata ID 6604 (Technical, Reported)**: Section 5.6 `/queryUpdates` client cache positioning clarification.
- **Errata ID 6605 (Technical, Reported)**: Section 5.5 `/query` position value clarification when no ids are returned.
- **Errata ID 6606 (Technical, Reported)**: Section 3.3 `using` list clarification for JMAP core capability.
- **Errata ID 6607 (Technical, Reported)**: Section 5.3 `/set` create and destroy referencing in the same request.

### 1.3 Model notes

1. **RFC 8620 §7.2 `PushSubscription/get` privacy requirement**:
   Section 7.2 states:
   > "As the 'url' and 'keys' properties may contain data that is private to a particular device, the values for these properties MUST NOT be returned. If the 'properties' argument is null or omitted, the server MUST default to all properties excluding these two."

   So the §7.2.3 example returns `PushSubscription` objects without `url`, which is why `PushSubscription.url` is an `Option<String>`; `verification_code` is modelled too.

2. **RFC 72-column string line folding**:
   In plain-text RFC documents, URLs and description strings that exceed 72 columns are formatted across multiple lines (e.g. `downloadUrl` and `eventSourceUrl` in §2.1, and `detail` in §3.6.1.1). Unfolded in the fixtures.

3. **RFC text ellipsis notation**:
   Informational RFC examples use ellipsis `...` to represent omitted properties or additional array elements (e.g. §2.1 `accountCapabilities`, §3.7 list responses, §5.7 `list` and `Todo/set` responses). Replaced with valid JSON in the fixtures.

## 2. RFC 8621 JMAP for Mail Examples (August 2019)

RFC 8621 defines the data model for synchronizing email data with a JMAP server, covering Mailboxes, Threads, Emails, SearchSnippets, Identities, and EmailSubmissions.

### 2.1 Fixtures

`rfc8621/`, 31 files:

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

### 2.2 Errata

RFC 8621 has no errata.

### 2.3 Model notes

1. **RFC 8621 §4.2 `Email/get` arguments**:
   RFC 8621 §4.2 specifies `Email/get` as extending standard `/get` with `bodyProperties`, `fetchTextBodyValues`, `fetchHTMLBodyValues`, `fetchAllBodyValues`, and `maxBodyValueBytes`. In the specification's own example in §4.2.1, `accountId` is omitted in the request. `EmailGetRequest` models these arguments, with `account_id` optional.

2. **RFC line folding in string literals**:
   Plain-text RFC formatting folds lines exceeding 72 characters, splitting string literals across lines in §4.2.1 (`bodyValues`), §4.10 (`preview`, `bodyValues` text and HTML contents), §5.2 (`preview`), §6.4 (`htmlSignature`), and §7.5.1 (`description`). Unfolded in the fixtures.

3. **Ellipsis notation in RFC examples**:
   RFC 8621 uses ellipsis `...` to indicate omitted fields in responses (§2.6 `list`, §4.10 `ids`, `list`, and `SetResponse` omitted maps, §5.2 `emailIds` and `list`, §7.5.1 `rcptTo`). Replaced with valid JSON in the fixtures.

## 3. RFC 9610 JMAP for Contacts Examples (December 2024)

RFC 9610 defines the data model for synchronizing address books and contact cards with a JMAP server, mapping JSContact cards into the JMAP data model with AddressBook and ContactCard types.

### 3.1 Fixtures

`rfc9610/`, 4 files:

1. `4.1_get_request.json`: Method calls pipeline requesting `AddressBook/get` and `ContactCard/get` (§4.1, Figure 1).
2. `4.1_get_response.json`: Method responses returning AddressBook list with `myRights` and `isDefault`, alongside ContactCard records with structured name components and emails (§4.1, Figure 2).
3. `4.2_set_request.json`: `AddressBook/set` invocation setting `onSuccessSetIsDefault` to reassign the default address book (§4.2, Figure 3).
4. `4.2_set_response.json`: `AddressBook/set` response confirming update of default address books (§4.2, Figure 4).

### 3.2 Errata

RFC 9610 has no errata.

### 3.3 Model notes

1. **RFC 9610 §2.3 `AddressBook/set` arguments**:
   Section 2.3 defines two additional request arguments on `AddressBook/set`:
   - `onDestroyRemoveContents: Option<bool>`
   - `onSuccessSetIsDefault: Option<Id>`

   `AddressBookSetRequest` wraps `SetRequest<AddressBook>` to model them.

2. **RFC 9610 §4.2 Figure 4 erratum in example response**:
   In Figure 4 of RFC 9610 §4.2, the example `AddressBook/set` response places `"oldState": "~4144"` and `"newState": "~4148"` inside the `"updated"` map rather than at the top level of the `SetResponse` object. In standard JMAP (RFC 8620 §5.3), `oldState` and `newState` are top-level properties of `SetResponse`. The fixture keeps the example as published, so `rfc_examples.rs` checks it as a raw invocation rather than a typed `SetResponse`.

## 4. RFC 9553 JSContact Card Examples (May 2024)

RFC 9553 defines JSContact, a JSON-based data format for contact card data.

### 4.1 Fixtures

`rfc9553/`, 42 files (excluding Figures 2 and 5 which define ABNF grammar rules):

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

### 4.2 Errata

RFC 9553 errata:

- **Errata ID 8263 (Technical, Reported)**: Section 2.2.1 Figure 20 omitted required Card properties (`uid`, `version`). The fixture preserves the example as published.
- **Errata ID 8264 (Editorial, Reported)**: Section 2.7.1 Figure 39 bracket formatting consistency.
- **Errata ID 8265 (Editorial, Reported)**: Section 2.2.1 Figure 18 property indentation consistency.
- **Errata ID 8266 (Technical, Reported)**: Section 2.6.2 Figure 36 omitted the closing brace `}` on the `"directories"` map. Fixed in fixture `2.6.2_directories.json`.
- **Errata ID 8267 (Technical, Reported)**: Section 2.6.1 CryptoKey kind enumeration clarification.

### 4.3 Model notes

1. **RFC 9553 §2.4.2 `schedulingAddresses`**:
   The specification defines `schedulingAddresses: Id[SchedulingAddress] (optional)`. `ContactCard.scheduling_addresses` models it as `Option<BTreeMap<String, SchedulingAddress>>`.

2. **RFC 9553 §2.2.4 `speakToAs.pronouns`**:
   The specification models `pronouns: Id[Pronouns] (optional)`, where each `Pronouns` object carries `pronouns: String`, optional `contexts`, and optional `pref`. `PronounsValue` accepts both the standard map of `Pronouns` and a plain string shorthand.

3. **RFC 9553 §2.1 top-level Card properties**:
   `ContactCard` has typed `language`, `members` and `prod_id` fields, so every core JSContact property deserialises.

4. **Multiline data URI folding in Figure 35**:
   Figure 35 embeds an RSA public key in a base64 data URI split across multiple lines. `2.6.1_crypto_keys_embedded.json` has it unfolded into one string.

## 5. RFC 8984 JSCalendar: A JSON Representation of Calendar Data (July 2021)

RFC 8984 defines the data model and JSON representation for calendar events, tasks, and groups of calendar objects.

### 5.1 Fixtures

`rfc8984/`, 10 files:

1. `6.1_simple_event.json`: Simple Event (§6.1).
2. `6.2_simple_task.json`: Simple Task (§6.2).
3. `6.3_simple_group.json`: Simple Group, with Errata ID 8028 applied (§6.3).
4. `6.4_all_day_event.json`: All-Day Event (§6.4).
5. `6.5_task_due_date.json`: Task with a Due Date (§6.5).
6. `6.6_event_end_time_zone.json`: Event with End Time Zone (§6.6).
7. `6.7_floating_time_event.json`: Floating-Time Event with Recurrence (§6.7).
8. `6.8_multiple_locations.json`: Event with Multiple Locations and Localization (§6.8).
9. `6.9_recurring_event_overrides.json`: Recurring Event with Overrides (§6.9).
10. `6.10_recurring_event_participants.json`: Recurring Event with Participants (§6.10).

### 5.2 Errata

RFC 8984 errata:

- **Errata ID 6872 (Technical, Verified)**: Section 5.1.2 private event shared properties clarification.
- **Errata ID 6873 (Technical, Verified)**: Section 4.3.4 floating time `recurrenceIdTimeZone` clarification.
- **Errata ID 8028 (Technical, Reported)**: Section 6.3 Group example incorrectly used `"name"` instead of `"title"` for Group title property.
- **Errata ID 8461 (Technical, Reported)**: Section 4.5.3 `offsetFrom` and `offsetTo` type definition string UTC offset format clarification.

### 5.3 Model notes

1. **RFC 8984 §6.5 Task optional `uid`**:
   The §6.5 example (`6.5_task_due_date.json`) has no `uid`, so `Task.uid` in `tasks.rs` defaults to empty and is skipped when empty.

2. **RFC 8984 §5.3.1 Group `entries` array representation**:
   RFC 8984 §5.3.1 defines `entries: (Event|Task)[]` as an array of JSCalendar objects. `CalendarGroup.entries` in `calendars.rs` is the untagged enum `GroupEntries`, accepting that array as well as a map.

## 6. draft-ietf-jmap-calendars-32: JMAP for Calendars (5 October 2026)

draft-ietf-jmap-calendars defines the data model and methods for synchronizing calendar objects and managing calendars via JMAP.

### 6.1 Fixtures

`jmap_calendars/`, 20 files (Figure 14, a sketch of an alert notification, is not JSON):

1. `5.9.1_calendar_event.json`: Recurring `CalendarEvent` with two participants (§5.9.1, Figure 1).
2. `5.9.1_patch_zoe_decline.json`: `CalendarEvent/set` patch declining one occurrence for one participant (§5.9.1, Figure 2).
3. `5.9.1_recurrence_overrides.json`: The resulting `recurrenceOverrides` (§5.9.1, Figure 3).
4. `5.9.1_patch_tom_restore.json`: Patch removing one participant's declined status from the override again (§5.9.1, Figure 4).
5. `5.9.1_patch_invalid_remove.json`: Patch the draft shows does not work for removing a participant from one occurrence (§5.9.1, Figure 5).
6. `5.9.1_patch_tom_remove_instance.json`: Patch that does it instead, replacing the whole override so the participant maps to null (§5.9.1, Figure 6).
7. `8.1_initial_data_request.json`: `Calendar/get`, `ParticipantIdentity/get` and `CalendarEvent/query` for initial sync (§8.1, Figure 7).
8. `8.1_initial_data_response.json`: Its responses (§8.1, Figure 8).
9. `8.2_create_event_request.json`: `CalendarEvent/set` creating an event with participants (§8.2, Figure 9).
10. `8.2_create_event_response.json`: Its response with server-set properties (§8.2, Figure 10).
11. `8.3_push_alert_notification.json`: `CalendarAlert` push object (§8.3, Figure 11).
12. `8.3_get_event_request.json`: `CalendarEvent/get` for the alerting event (§8.3, Figure 12).
13. `8.3_get_event_response.json`: Its response (§8.3, Figure 13).
14. `8.3_snooze_alert_request.json`: `CalendarEvent/set` acknowledging and snoozing an alert (§8.3, Figure 15).
15. `8.4_default_calendar_request.json`: `Calendar/set` with `onSuccessSetIsDefault` (§8.4, Figure 16).
16. `8.4_default_calendar_response.json`: Its response (§8.4, Figure 17).
17. `8.5_parse_ical_request.json`: `CalendarEvent/parse` request (§8.5, Figure 18).
18. `8.5_parse_ical_parsed_response.json`: Response with a parsed event (§8.5, Figure 19).
19. `8.5_parse_ical_not_found_response.json`: Response with `notFound` (§8.5, Figure 20).
20. `8.5_parse_ical_not_parsable_response.json`: Response with `notParsable` (§8.5, Figure 21).

### 6.2 Errata and normalisation

- An Internet-Draft has no errata.
- `...` placeholders in Figures 8 and 10 are replaced with valid JSON, and the RFC 8792 `\` line wrapping in Figures 2, 4, 5 and 15 is unfolded.

### 6.3 Model notes

1. **Section 4.3 `Calendar/set` arguments**:
   Section 4.3 defines two additional request arguments on `Calendar/set`: `onDestroyRemoveContents: Option<bool>` and `onSuccessSetIsDefault: Option<Id>`. `CalendarSetRequest` wraps `SetRequest<Calendar>` to model them.

2. **Section 6.4 `CalendarAlert` push notifications**:
   Section 6.4 defines the `CalendarAlert` object pushed when an alert triggers. `CalendarAlert` in `calendars.rs` models `accountId`, `calendarEventId`, `uid`, `recurrenceId` and `alertId`.

## 7. RFC 9425 JMAP for Quotas Examples (June 2023)

RFC 9425 defines a data model for accessing server quotas via JMAP, enabling clients to inspect data usage and storage limits across mail, calendar, contact, and system resources.

### 7.1 Fixtures

`rfc9425/`, 4 files:

1. `5.1_get_request.json`: `Quota/get` request querying quota records (§5.1).
2. `5.1_get_response.json`: `Quota/get` response returning octets and count quotas with `used`, `hardLimit`, and `scope` (§5.1).
3. `5.2_changes_request.json`: Chained `Quota/changes` and back-referenced `Quota/get` request (§5.2).
4. `5.2_changes_response.json`: Chained `Quota/changes` and back-referenced `Quota/get` response (§5.2).

### 7.2 Errata

RFC 9425 has no errata.

### 7.3 Model notes

1. **RFC 9425 §5.1 Property Filtering**:
   In `Quota/get`, the `properties` argument allows clients to request a subset of quota fields. When specific properties are omitted by the server in response to filtering, fields such as `name`, `resource_type`, `used`, `hard_limit`, and `scope` must default, so these fields on `Quota` in `quota.rs` are `#[serde(default)]`.

## 8. RFC 9670 JMAP Sharing Examples (November 2024)

RFC 9670 specifies a sharing data model for JMAP, defining Principals and ShareNotifications to represent access rights and invitations across accounts.

### 8.1 Fixtures

`rfc9670/`, 4 files:

1. `4.1_session_object.json`: Session object advertising `urn:ietf:params:jmap:principals` with principal account capabilities (§4.1).
2. `4.1_principal_get_request.json`: `Principal/get` request fetching user principals (§4.1).
3. `4.1_principal_get_response.json`: `Principal/get` response returning principal records with account id, name, and email (§4.1).
4. `4.1_todolist_set_request.json`: `TodoList/set` request creating a shared list with `shareWith` access rights (§4.1).

### 8.2 Errata

RFC 9670 has no errata.

### 8.3 Model notes

Every example round-trips losslessly with no special handling.

## 9. RFC 9661 JMAP for Sieve Scripts Examples (September 2024)

RFC 9661 specifies a data model for managing Sieve mail filtering scripts via JMAP, supporting script upload, validation, activation, and deactivation.

### 9.1 Fixtures

`rfc9661/`, 16 files:

1. `1.2.2_session.json`: Session object advertising `urn:ietf:params:jmap:sieve` capability (§1.2.2).
2. `2.3.1_get_all_request.json`: `SieveScript/get` request fetching all scripts with `ids: null` (§2.3.1).
3. `2.3.1_get_all_response.json`: `SieveScript/get` response returning active script (§2.3.1).
4. `2.3.1_get_and_blob_get_request.json`: Chained `SieveScript/get` and `Blob/get` with `#ids` backreference (§2.3.1).
5. `2.3.1_get_and_blob_get_response.json`: Chained `SieveScript/get` and `Blob/get` response (§2.3.1).
6. `2.4.1_upload_response.json`: `Blob/upload` response for script upload (§2.4.1).
7. `2.4.1_create_activate_request.json`: `SieveScript/set` create and activate request with `name: null` and `onSuccessActivateScript: "#A"` (§2.4.1).
8. `2.4.1_create_activate_response.json`: `SieveScript/set` response returning created script and updated deactivate on previous script (§2.4.1).
9. `2.4.1_update_blob_request.json`: `Blob/upload` and `SieveScript/set` update request (§2.4.1).
10. `2.4.1_update_blob_response.json`: `Blob/upload` and `SieveScript/set` update response (§2.4.1).
11. `2.4.1_rename_deactivate_request.json`: `SieveScript/set` rename and deactivate request with `onSuccessDeactivateScript: true` (§2.4.1).
12. `2.4.1_rename_deactivate_response.json`: `SieveScript/set` rename and deactivate response (§2.4.1).
13. `2.4.1_reactivate_request.json`: `SieveScript/set` request reactivating an existing script (§2.4.1).
14. `2.4.1_reactivate_response.json`: `SieveScript/set` response confirming activation (§2.4.1).
15. `2.4.1_deactivate_destroy_request.json`: `SieveScript/set` deactivate and destroy request (§2.4.1).
16. `2.4.1_deactivate_destroy_response.json`: `SieveScript/set` deactivate and destroy response (§2.4.1).

### 9.2 Errata

RFC 9661 has no errata.

### 9.3 Model notes

1. **RFC 9661 §2.1 and §2.4 SieveScript Nullable Name**:
   RFC 9661 §2.1 types `name: String|null (default: null)`. On script creation, clients may pass `name: null` or omit `name` to allow the server to choose a name. `SieveScript.name` in `sieve.rs` is an `Option<String>`.

2. **Partial Update Deserialization in SetResponse**:
   In `SetResponse<SieveScript>`, partial update responses (such as deactivating the old script `updated: { id: { isActive: false } }`) omit `blobId`, so `SieveScript.blob_id` defaults and is skipped when empty.

3. **RFC 9661 §2.4 Deactivate Script Extension Argument**:
   RFC 9661 §2.4 introduces `onSuccessDeactivateScript: Option<bool>` alongside `onSuccessActivateScript` on `SieveScriptSetRequest`, which models it.

4. **RFC 9661 §2.3.1 Backreferences in Blob/get**:
   RFC 9661 §2.3.1 pipelines `Blob/get` with `#ids` backreference to fetch the script body blob, which is why `BlobGetRequest` in `blob.rs` has `ids_ref` and a defaulted `ids`.

## 10. RFC 9404 JMAP Blob Management Extension Examples (August 2023)

RFC 9404 specifies methods for uploading, retrieving, and inspecting raw data blobs in JMAP.

### 10.1 Fixtures

`rfc9404/`, 11 files:

1. `3.1.1_capability.json`: Session object advertising `urn:ietf:params:jmap:blob` capability (§3.1.1).
2. `4.1.1_upload_request.json`: `Blob/upload` request uploading data as text (§4.1.1).
3. `4.1.1_upload_response.json`: `Blob/upload` response returning created blob record (§4.1.1).
4. `4.1.2_upload_request.json`: `Blob/upload` request with base64 data and SHA-256 digest (§4.1.2).
5. `4.1.2_upload_response.json`: `Blob/upload` response confirming upload (§4.1.2).
6. `4.2.1_get_request.json`: `Blob/get` request fetching text content (§4.2.1).
7. `4.2.1_get_response.json`: `Blob/get` response returning data as text and encoding problem indicator (§4.2.1).
8. `4.2.2_get_request.json`: `Blob/get` request fetching byte range with offset and length (§4.2.2).
9. `4.2.2_get_response.json`: `Blob/get` response returning sliced text (§4.2.2).
10. `4.3.1_lookup_request.json`: `Blob/lookup` request (§4.3.1).
11. `4.3.1_lookup_response.json`: `Blob/lookup` response (§4.3.1).

### 10.2 Errata

RFC 9404 has no errata.

### 10.3 Model notes

1. **RFC 9404 §2.1 BlobInfo Encoding Problem and Truncated Indicators**:
   RFC 9404 §2.1 types `isEncodingProblem: Option<bool>` and `isTruncated: Option<bool>` on `BlobInfo`, both typed in `blob.rs`.

2. **RFC 9404 §2.2 BlobUploadResponse States**:
   RFC 9404 §2.2 types `oldState: Option<String>` and `newState: Option<String>` on `BlobUploadResponse`, both typed in `blob.rs`.

3. **Single Account Context AccountId Omission**:
   In RFC 9404 §4 request and response examples, `accountId` is omitted when operating within a single default account context. So `account_id` defaults and is skipped when empty in `BlobUploadRequest`, `BlobUploadResponse`, `BlobGetRequest`, `BlobGetResponse`, `BlobLookupRequest`, and `BlobLookupResponse`.

