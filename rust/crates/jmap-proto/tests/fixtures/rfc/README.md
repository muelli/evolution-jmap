<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# RFC example fixtures

This directory contains the JSON examples that the JMAP and JSContact/JSCalendar
specifications print. Each example has its own file. The tests use them to
check that the wire types of `jmap-proto` accept what the specifications
themselves send.

## Specifications

| Directory | Specification | Version |
|---|---|---|
| `rfc8620/` | RFC 8620, JMAP core | July 2019 |
| `rfc8621/` | RFC 8621, JMAP for Mail | August 2019 |
| `rfc9610/` | RFC 9610, JMAP for Contacts | December 2024 |
| `rfc9553/` | RFC 9553, JSContact | May 2024 |
| `rfc8984/` | RFC 8984, JSCalendar | July 2021 |
| `jmap_calendars/` | draft-ietf-jmap-calendars-33, JMAP for Calendars | 14 October 2026 |
| `rfc9425/` | RFC 9425, JMAP for Quotas | June 2023 |
| `rfc9670/` | RFC 9670, JMAP Sharing (principals) | November 2024 |
| `rfc9661/` | RFC 9661, JMAP for Sieve Scripts | September 2024 |
| `rfc9404/` | RFC 9404, JMAP Blob Management | August 2023 |

The calendars fixtures come from draft -33. `jmap_proto::calendars` cites draft
-33 for its property names and section numbers. Draft -28 and draft -33 define
the same section structure, so the -33 examples check that model.

## How they differ from the specification text

Each file is the example as published, changed into valid JSON:

- Line 1 is a `// Source:` comment. It names the specification, the section and
  the figure (and the erratum, if one was applied).
- The files do not contain the folding of strings. A string that the
  specification folds at 72 columns, or wraps with RFC 8792 `\` markers, is
  unfolded.
- The files do not contain `...` placeholders for omitted properties or list
  entries. Valid JSON replaces them. This does not touch any field that
  `jmap-proto` models.
- Verified or obvious errata are applied if they break parsing (the list for
  each specification is below). Otherwise the example stays as published.

## How `tests/rfc_examples.rs` uses them

`load_rfc_fixture` strips the `//` lines. It requires the source note. It
deserialises the rest into the wire type under test. Each test then does these
steps:

1. It checks that the source note names the expected section.
2. It asserts the fields that the example is about.
3. It serialises the value again to confirm that the round trip is lossless.

To add an example, add the file here and a test there.

## 1. RFC 8620 Core JMAP Examples (July 2019)

RFC 8620 defines the basic JMAP protocol data structures, envelopes, problem
details and push notifications.

### 1.1 Fixtures

`rfc8620/`, 31 files:

1. `2.1_session.json`: complete JMAP Session resource. It advertises core, mail, contacts and custom vendor capabilities with primary and secondary accounts.
2. `3.3.1_request.json`: standard Request envelope with `using` and several method calls.
3. `3.4.1_response.json`: standard Response envelope with several method responses and `sessionState`.
4. `3.6.1.1_unknown_capability.json`: request-level problem details (`urn:ietf:params:jmap:error:unknownCapability`).
5. `3.6.1.1_limit.json`: request-level problem details (`urn:ietf:params:jmap:error:limit`) with `limit: "maxSizeRequest"`.
6. `3.6.2_method_error.json`: method-level `error` invocation with `type: "unknownMethod"`.
7. `3.7_simple_backref_request.json`: method calls that chain `Foo/changes` and `Foo/get` with a `#ids` `ResultReference`.
8. `3.7_thread_backref_request.json`: pipeline of four method calls that chains `Email/query`, `Email/get`, `Thread/get` and `Email/get` with JSON Pointer backreferences.
9. `3.7_thread_backref_response.json`: response with the partial results of the query and get methods.
10. `3.7_resolved_get_request.json`: resolved `Email/get` arguments that the server evaluated from backreferences.
11. `4.1_echo_request.json`: `Core/echo` method call invocation.
12. `4.1_echo_response.json`: `Core/echo` method response invocation.
13. `5.7_query_get_request.json`: standard chained `Todo/query` and `Todo/get` request.
14. `5.7_query_get_response.json`: standard `Todo/query` and `Todo/get` responses.
15. `5.7_set_update_full_request.json`: standard `Todo/set` call with a whole-object update.
16. `5.7_set_update_patch_request.json`: standard `Todo/set` call with JSON Pointer patch syntax.
17. `5.7_set_update_response.json`: standard `Todo/set` update response with properties that the server calculated.
18. `5.7_set_create_subtask_request.json`: standard `Todo/set` call that references the creation id `"#k15"` in a foreign key property.
19. `5.7_changes_request.json`: standard `Todo/changes` and `Todo/queryChanges` request.
20. `5.7_changes_response.json`: standard `Todo/changes` and `Todo/queryChanges` response.
21. `5.7_copy_request.json`: standard `Todo/copy` request across accounts with `onSuccessDestroyOriginal: true`.
22. `5.7_copy_response.json`: standard `Todo/copy` response. It has several responses (`Todo/copy` and implicit `Todo/set`) for the same call id.
23. `7.1.1_state_change.json`: EventSource `StateChange` push notification across several accounts.
24. `7.2.3_get_request.json`: `PushSubscription/get` request with `ids: null`.
25. `7.2.3_get_response.json`: `PushSubscription/get` response with subscription records.
26. `7.2.3_set_create_request.json`: `PushSubscription/set` create call with `deviceClientId` and `url`.
27. `7.2.3_set_created_response.json`: `PushSubscription/set` created response with an `expires` value that the server capped.
28. `7.2.3_verification.json`: POST payload of the `PushVerification` challenge that the server starts.
29. `7.2.3_set_update_code_request.json`: `PushSubscription/set` update with the verification code.
30. `7.2.3_set_update_expires_request.json`: `PushSubscription/set` update that extends the expiry timestamp.
31. `7.2.3_set_update_expires_response.json`: `PushSubscription/set` updated response that confirms the extended expiry.

### 1.2 Errata

RFC 8620 errata:

- **Errata ID 5791 (Technical, Verified)**: In §2.1, the example Session resource response names `maxConcurrentRequest` where it must name `maxConcurrentRequests`. It also lacks a trailing comma after `"urn:ietf:params:jmap:mail": {}`. The fixture `2.1_session.json` corrects both.
- **Errata ID 6603 (Technical, Reported)**: clarification of the `upToId` specification in section 5.5.
- **Errata ID 6604 (Technical, Reported)**: clarification of the `/queryUpdates` client cache positioning in section 5.6.
- **Errata ID 6605 (Technical, Reported)**: clarification of the `/query` position value in section 5.5 when no ids are returned.
- **Errata ID 6606 (Technical, Reported)**: clarification of the `using` list for the JMAP core capability in section 3.3.
- **Errata ID 6607 (Technical, Reported)**: clarification of the reference to create and destroy in the same request in section 5.3 (`/set`).

### 1.3 Model notes

1. **RFC 8620 §7.2 `PushSubscription/get` privacy requirement**:
   Section 7.2 states:
   > "As the 'url' and 'keys' properties may contain data that is private to a particular device, the values for these properties MUST NOT be returned. If the 'properties' argument is null or omitted, the server MUST default to all properties excluding these two."

   The §7.2.3 example therefore returns `PushSubscription` objects without `url`. This is why `PushSubscription.url` is an `Option<String>`. `verification_code` is also modelled.

2. **RFC 72-column string line folding**:
   Plain-text RFC documents format URLs and description strings that exceed 72 columns across several lines (for example `downloadUrl` and `eventSourceUrl` in §2.1, and `detail` in §3.6.1.1). The fixtures contain them unfolded.

3. **RFC text ellipsis notation**:
   Informational RFC examples use the ellipsis `...` for omitted properties or more array elements (for example §2.1 `accountCapabilities`, §3.7 list responses, §5.7 `list` and `Todo/set` responses). The fixtures contain valid JSON in their place.

## 2. RFC 8621 JMAP for Mail Examples (August 2019)

RFC 8621 defines the data model to synchronize email data with a JMAP server. It covers Mailboxes, Threads, Emails, SearchSnippets, Identities and EmailSubmissions.

### 2.1 Fixtures

`rfc8621/`, 31 files:

1. `2.6_get_request.json`: `Mailbox/get` request that fetches all mailboxes with `ids: null` (§2.6).
2. `2.6_get_response.json`: `Mailbox/get` response that returns the Inbox with `myRights` and `isSubscribed` (§2.6).
3. `2.6_changes_request.json`: pipelined request that chains `Mailbox/changes` and back-referenced `Mailbox/get` calls (§2.6).
4. `2.6_changes_response.json`: pipelined response with `Mailbox/changes` and the updated Mailbox data (§2.6).
5. `2.6_set_request.json`: `Mailbox/set` request that renames one mailbox and destroys another (§2.6).
6. `2.6_set_response.json`: `Mailbox/set` response with the updated mailbox and a forbidden error for the destroy (§2.6).
7. `3.1.1_get_request.json`: `Thread/get` request with explicit ids (§3.1.1).
8. `3.1.1_get_response.json`: `Thread/get` response with thread records that contain `emailIds` (§3.1.1).
9. `4.1.2.3_addresses.json`: parsed form `EmailAddress[]` of a header field, with display names and email addresses (§4.1.2.3).
10. `4.1.2.4_grouped_addresses.json`: parsed form `EmailAddressGroup[]` of a header field that keeps the group information (§4.1.2.4).
11. `4.2.1_get_request.json`: `Email/get` request that specifies body properties, `fetchHTMLBodyValues` and `maxBodyValueBytes` (§4.2.1).
12. `4.2.1_get_response.json`: `Email/get` response with `htmlBody`, truncated `bodyValues` and `notFound` (§4.2.1).
13. `4.10_initial_sync_request.json`: initial mailbox sync pipeline that chains `Email/query`, `Email/get`, `Thread/get` and `Email/get` (§4.10).
14. `4.10_initial_sync_response.json`: initial mailbox sync response with query results, thread ids, thread records and email summaries (§4.10).
15. `4.10_set_update_request.json`: `Email/set` request that updates the keywords patch (`keywords/$seen: null` marks the email as unread) (§4.10).
16. `4.10_set_update_response.json`: `Email/set` response that confirms the keywords update (§4.10).
17. `4.10_resync_request.json`: resynchronization request that chains `Email/changes` and `Email/queryChanges` (§4.10).
18. `4.10_resync_response.json`: resynchronization response with the created, updated and destroyed email ids and the query delta items (§4.10).
19. `4.10_set_create_text_request.json`: `Email/set` request that creates a plaintext draft with `bodyStructure` and `bodyValues` (§4.10).
20. `4.10_set_create_text_response.json`: `Email/set` response with the metadata of the created draft (§4.10).
21. `4.10_set_create_html_request.json`: `Email/set` request that creates a multipart/alternative HTML draft and destroys the previous draft (§4.10).
22. `4.10_set_create_html_response.json`: `Email/set` response that confirms the creation of the HTML draft and the destruction of the old draft (§4.10).
23. `4.10_copy_request.json`: `Email/copy` request across accounts with `onSuccessDestroyOriginal: true` (§4.10).
24. `4.10_copy_response.json`: `Email/copy` response across accounts. The implicit `Email/set` destroy response has the same call id (§4.10).
25. `5.2_get_request.json`: `SearchSnippet/get` request that filters by a text query across email ids (§5.2).
26. `5.2_get_response.json`: `SearchSnippet/get` response with match snippets that have `<mark>` highlight tags (§5.2).
27. `6.4_get_request.json`: `Identity/get` request invocation (§6.4).
28. `6.4_get_response.json`: `Identity/get` response invocation with identities that have text and HTML signatures (§6.4).
29. `7.5.1_set_create_request.json`: `EmailSubmission/set` create request with `envelope` and an `onSuccessUpdateEmail` patch (§7.5.1).
30. `7.5.1_set_created_response.json`: `EmailSubmission/set` response with an implicit `Email/set` update response (§7.5.1).
31. `7.5.1_set_error_response.json`: `EmailSubmission/set` error response with a localized `forbiddenToSend` failure (§7.5.1).

### 2.2 Errata

RFC 8621 has no errata.

### 2.3 Model notes

1. **RFC 8621 §4.2 `Email/get` arguments**:
   RFC 8621 §4.2 specifies `Email/get` as an extension of the standard `/get`. The extension adds `bodyProperties`, `fetchTextBodyValues`, `fetchHTMLBodyValues`, `fetchAllBodyValues` and `maxBodyValueBytes`. In the own example of the specification in §4.2.1, the request omits `accountId`. `EmailGetRequest` models these arguments, with an optional `account_id`.

2. **RFC line folding in string literals**:
   The plain-text RFC format folds lines of more than 72 characters. This splits string literals across lines in §4.2.1 (`bodyValues`), §4.10 (`preview`, and the text and HTML contents of `bodyValues`), §5.2 (`preview`), §6.4 (`htmlSignature`) and §7.5.1 (`description`). The fixtures contain them unfolded.

3. **Ellipsis notation in RFC examples**:
   RFC 8621 uses the ellipsis `...` for omitted fields in responses (§2.6 `list`, §4.10 `ids`, `list` and the omitted maps of `SetResponse`, §5.2 `emailIds` and `list`, §7.5.1 `rcptTo`). The fixtures contain valid JSON in their place.

## 3. RFC 9610 JMAP for Contacts Examples (December 2024)

RFC 9610 defines the data model to synchronize address books and contact cards with a JMAP server. It maps JSContact cards into the JMAP data model with the types AddressBook and ContactCard.

### 3.1 Fixtures

`rfc9610/`, 4 files:

1. `4.1_get_request.json`: pipeline of method calls that requests `AddressBook/get` and `ContactCard/get` (§4.1, Figure 1).
2. `4.1_get_response.json`: method responses with the AddressBook list (`myRights` and `isDefault`) and ContactCard records with structured name components and emails (§4.1, Figure 2).
3. `4.2_set_request.json`: `AddressBook/set` invocation that sets `onSuccessSetIsDefault` to assign the default address book again (§4.2, Figure 3).
4. `4.2_set_response.json`: `AddressBook/set` response that confirms the update of the default address books (§4.2, Figure 4).

### 3.2 Errata

RFC 9610 has no errata.

### 3.3 Model notes

1. **RFC 9610 §2.3 `AddressBook/set` arguments**:
   Section 2.3 defines two more request arguments on `AddressBook/set`:
   - `onDestroyRemoveContents: Option<bool>`
   - `onSuccessSetIsDefault: Option<Id>`

   `AddressBookSetRequest` wraps `SetRequest<AddressBook>` to model them.

2. **RFC 9610 §4.2 Figure 4 erratum in the example response**:
   In Figure 4 of RFC 9610 §4.2, the example `AddressBook/set` response puts `"oldState": "~4144"` and `"newState": "~4148"` inside the `"updated"` map. They must be at the top level of the `SetResponse` object. In standard JMAP (RFC 8620 §5.3), `oldState` and `newState` are top-level properties of `SetResponse`. The fixture keeps the example as published. `rfc_examples.rs` therefore checks it as a raw invocation. It does not check it as a typed `SetResponse`.

## 4. RFC 9553 JSContact Card Examples (May 2024)

RFC 9553 defines JSContact, a JSON-based data format for contact card data.

### 4.1 Fixtures

`rfc9553/`, 42 files (Figures 2 and 5 are not included, because they define ABNF grammar rules):

1. `1.4.5_phonetic.json`: phonetic name pronunciation with the International Phonetic Alphabet (IPA) (§1.4.5, Figure 1).
2. `1.6.2_vendor_properties.json`: custom vendor-specific property extensions with a domain prefix (§1.6.2, Figure 3).
3. `1.6.2_vendor_value.json`: vendor-specific value for the `kind` property (§1.6.2, Figure 4).
4. `2.1_basic_card.json`: minimal valid Card object for an individual with structured name components (§2.1, Figure 6).
5. `2.1.2_version.json`: Card `version` property (§2.1.2, Figure 7).
6. `2.1.3_created.json`: Card `created` timestamp (§2.1.3, Figure 8).
7. `2.1.4_kind.json`: Card `kind` property set to `individual` (§2.1.4, Figure 9).
8. `2.1.5_language.json`: Card `language` tag (§2.1.5, Figure 10).
9. `2.1.6_members.json`: group Card with `members` set (§2.1.6, Figure 11).
10. `2.1.7_prod_id.json`: Card `prodId` software identifier (§2.1.7, Figure 12).
11. `2.1.8_related_to.json`: Card `relatedTo` relation mappings (§2.1.8, Figure 13).
12. `2.1.9_uid.json`: mandatory Card `uid` URI (§2.1.9, Figure 14).
13. `2.1.10_updated.json`: Card `updated` timestamp (§2.1.10, Figure 15).
14. `2.2.1_name_two_words.json`: name component with a surname of several words (§2.2.1, Figure 16).
15. `2.2.1_name_second_surname.json`: name with a second surname (§2.2.1, Figure 17).
16. `2.2.1_name_full.json`: formatted `full` name property (§2.2.1, Figure 18).
17. `2.2.1_name_sort_as.json`: name sort order override mappings (§2.2.1, Figure 19).
18. `2.2.1_name_phonetic_localizations.json`: Card with Chinese characters and Cantonese Jyutping phonetic localization patches (§2.2.1, Figure 20).
19. `2.2.2_nicknames.json`: nicknames map with familiar names (§2.2.2, Figure 21).
20. `2.2.3_organizations.json`: organizational affiliation with division and department units and a sort key (§2.2.3, Figure 22).
21. `2.2.4_speak_to_as.json`: addressing preferences with grammatical gender and ranked pronouns (§2.2.4, Figure 23).
22. `2.2.5_titles.json`: professional titles and roles that link to organizations (§2.2.5, Figure 24).
23. `2.3.1_emails.json`: several email addresses with contexts and preferences (§2.3.1, Figure 25).
24. `2.3.2_online_services.json`: online service user names and protocol URIs (§2.3.2, Figure 26).
25. `2.3.3_phones.json`: telephone numbers with features and types (§2.3.3, Figure 27).
26. `2.3.4_preferred_languages.json`: language communication preferences, ranked by context (§2.3.4, Figure 28).
27. `2.4.1_calendars.json`: calendaring URIs for the personal calendar and free-busy lookups (§2.4.1, Figure 29).
28. `2.4.2_scheduling_addresses.json`: calendar scheduling addresses with ranking (§2.4.2, Figure 30).
29. `2.5.1_address_usa.json`: structured postal address in the United States (§2.5.1, Figure 31).
30. `2.5.1_address_thailand.json`: structured postal address in Thailand with subdistrict and district components (§2.5.1, Figure 32).
31. `2.5.1_address_tokyo.json`: postal address in Tokyo with its localized Japanese representation (§2.5.1, Figure 33).
32. `2.6.1_crypto_keys_external.json`: external cryptographic key reference (§2.6.1, Figure 34).
33. `2.6.1_crypto_keys_embedded.json`: inline cryptographic public key with a base64 data URI (§2.6.1, Figure 35).
34. `2.6.2_directories.json`: directory service URLs for user lookup (§2.6.2, Figure 36).
35. `2.6.3_links.json`: contact resource link with a custom label and a preference (§2.6.3, Figure 37).
36. `2.6.4_media.json`: media attachments that include a sound pronunciation, a logo and a photo data URI (§2.6.4, Figure 38).
37. `2.7.1_localization_top_level.json`: localization of a top-level property with JSON patch paths (§2.7.1, Figure 39).
38. `2.7.1_localization_nested.json`: localization of nested properties for name and titles (§2.7.1, Figure 40).
39. `2.8.1_anniversaries.json`: anniversaries with partial calendar dates and a timestamp (§2.8.1, Figure 41).
40. `2.8.2_keywords.json`: keyword tags map (§2.8.2, Figure 42).
41. `2.8.3_notes.json`: free-text notes with authorship and a creation timestamp (§2.8.3, Figure 43).
42. `2.8.4_personal_info.json`: personal hobbies, interests and expertise with proficiency levels (§2.8.4, Figure 44).

### 4.2 Errata

RFC 9553 errata:

- **Errata ID 8263 (Technical, Reported)**: Figure 20 in section 2.2.1 omits the required Card properties (`uid`, `version`). The fixture keeps the example as published.
- **Errata ID 8264 (Editorial, Reported)**: consistency of the bracket formatting in Figure 39 in section 2.7.1.
- **Errata ID 8265 (Editorial, Reported)**: consistency of the property indentation in Figure 18 in section 2.2.1.
- **Errata ID 8266 (Technical, Reported)**: Figure 36 in section 2.6.2 omits the closing brace `}` of the `"directories"` map. The fixture `2.6.2_directories.json` fixes this.
- **Errata ID 8267 (Technical, Reported)**: clarification of the CryptoKey kind enumeration in section 2.6.1.

### 4.3 Model notes

1. **RFC 9553 §2.4.2 `schedulingAddresses`**:
   The specification defines `schedulingAddresses: Id[SchedulingAddress] (optional)`. `ContactCard.scheduling_addresses` models it as `Option<BTreeMap<String, SchedulingAddress>>`.

2. **RFC 9553 §2.2.4 `speakToAs.pronouns`**:
   The specification models `pronouns: Id[Pronouns] (optional)`. Each `Pronouns` object carries `pronouns: String`, optional `contexts` and optional `pref`. `PronounsValue` accepts the standard map of `Pronouns` and also a plain string shorthand.

3. **RFC 9553 §2.1 top-level Card properties**:
   `ContactCard` has typed `language`, `members` and `prod_id` fields. Every core JSContact property therefore deserialises.

4. **Folding of the multiline data URI in Figure 35**:
   Figure 35 embeds an RSA public key in a base64 data URI. The figure splits the URI across several lines. `2.6.1_crypto_keys_embedded.json` has it unfolded into one string.

## 5. RFC 8984 JSCalendar: A JSON Representation of Calendar Data (July 2021)

RFC 8984 defines the data model and the JSON representation for calendar events, tasks and groups of calendar objects.

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

- **Errata ID 6872 (Technical, Verified)**: clarification of the shared properties of a private event in section 5.1.2.
- **Errata ID 6873 (Technical, Verified)**: clarification of `recurrenceIdTimeZone` for floating time in section 4.3.4.
- **Errata ID 8028 (Technical, Reported)**: the Group example in section 6.3 uses `"name"` where it must use `"title"` for the Group title property.
- **Errata ID 8461 (Technical, Reported)**: clarification of the string UTC offset format in the type definition of `offsetFrom` and `offsetTo` in section 4.5.3.

### 5.3 Model notes

1. **RFC 8984 §6.5 Task optional `uid`**:
   The §6.5 example (`6.5_task_due_date.json`) has no `uid`. `Task.uid` in `tasks.rs` therefore defaults to empty. The serialiser skips it if it is empty.

2. **RFC 8984 §5.3.1 Group `entries` array representation**:
   RFC 8984 §5.3.1 defines `entries: (Event|Task)[]` as an array of JSCalendar objects. `CalendarGroup.entries` in `calendars.rs` is the untagged enum `GroupEntries`. It accepts that array and also a map.

## 6. draft-ietf-jmap-calendars-33: JMAP for Calendars (14 October 2026)

draft-ietf-jmap-calendars defines the data model and the methods to synchronize calendar objects and to manage calendars through JMAP.

### 6.1 Fixtures

`jmap_calendars/`, 20 files (Figure 14, a sketch of an alert notification, is not JSON):

1. `5.9.1_calendar_event.json`: recurring `CalendarEvent` with two participants (§5.9.1, Figure 1).
2. `5.9.1_patch_zoe_decline.json`: `CalendarEvent/set` patch that declines one occurrence for one participant (§5.9.1, Figure 2).
3. `5.9.1_recurrence_overrides.json`: the resulting `recurrenceOverrides` (§5.9.1, Figure 3).
4. `5.9.1_patch_tom_restore.json`: patch that removes the declined status of one participant from the override again (§5.9.1, Figure 4).
5. `5.9.1_patch_invalid_remove.json`: patch that the draft shows does not work to remove a participant from one occurrence (§5.9.1, Figure 5).
6. `5.9.1_patch_tom_remove_instance.json`: patch that does this instead. It replaces the whole override, so that the participant maps to null (§5.9.1, Figure 6).
7. `8.1_initial_data_request.json`: `Calendar/get`, `ParticipantIdentity/get` and `CalendarEvent/query` for the initial sync (§8.1, Figure 7).
8. `8.1_initial_data_response.json`: the responses to that request (§8.1, Figure 8).
9. `8.2_create_event_request.json`: `CalendarEvent/set` that creates an event with participants (§8.2, Figure 9).
10. `8.2_create_event_response.json`: the response to that request, with properties that the server set (§8.2, Figure 10).
11. `8.3_push_alert_notification.json`: `CalendarAlert` push object (§8.3, Figure 11).
12. `8.3_get_event_request.json`: `CalendarEvent/get` for the alerting event (§8.3, Figure 12).
13. `8.3_get_event_response.json`: the response to that request (§8.3, Figure 13).
14. `8.3_snooze_alert_request.json`: `CalendarEvent/set` that acknowledges and snoozes an alert (§8.3, Figure 15).
15. `8.4_default_calendar_request.json`: `Calendar/set` with `onSuccessSetIsDefault` (§8.4, Figure 16).
16. `8.4_default_calendar_response.json`: the response to that request (§8.4, Figure 17).
17. `8.5_parse_ical_request.json`: `CalendarEvent/parse` request (§8.5, Figure 18).
18. `8.5_parse_ical_parsed_response.json`: response with a parsed event (§8.5, Figure 19).
19. `8.5_parse_ical_not_found_response.json`: response with `notFound` (§8.5, Figure 20).
20. `8.5_parse_ical_not_parsable_response.json`: response with `notParsable` (§8.5, Figure 21).

### 6.2 Errata and normalisation

- An Internet-Draft has no errata.
- Valid JSON replaces the `...` placeholders in Figures 8 and 10. The fixtures contain the RFC 8792 `\` line wrapping in Figures 2, 4, 5 and 15 in unfolded form.

### 6.3 Model notes

1. **Section 4.3 `Calendar/set` arguments**:
   Section 4.3 defines two more request arguments on `Calendar/set`: `onDestroyRemoveContents: Option<bool>` and `onSuccessSetIsDefault: Option<Id>`. `CalendarSetRequest` wraps `SetRequest<Calendar>` to model them.

2. **Section 6.4 `CalendarAlert` push notifications**:
   Section 6.4 defines the `CalendarAlert` object that the server pushes when an alert triggers. `CalendarAlert` in `calendars.rs` models `accountId`, `calendarEventId`, `uid`, `recurrenceId` and `alertId`.

## 7. RFC 9425 JMAP for Quotas Examples (June 2023)

RFC 9425 defines a data model to access server quotas through JMAP. A client can inspect data usage and storage limits for mail, calendar, contact and system resources.

### 7.1 Fixtures

`rfc9425/`, 4 files:

1. `5.1_get_request.json`: `Quota/get` request that queries quota records (§5.1).
2. `5.1_get_response.json`: `Quota/get` response with octets and count quotas that have `used`, `hardLimit` and `scope` (§5.1).
3. `5.2_changes_request.json`: chained request with `Quota/changes` and a back-referenced `Quota/get` (§5.2).
4. `5.2_changes_response.json`: chained response with `Quota/changes` and a back-referenced `Quota/get` (§5.2).

### 7.2 Errata

RFC 9425 has no errata.

### 7.3 Model notes

1. **RFC 9425 §5.1 Property Filtering**:
   In `Quota/get`, the `properties` argument lets a client request a subset of the quota fields. The server can omit properties in response to this filtering. The fields `name`, `resource_type`, `used`, `hard_limit` and `scope` must then have default values. These fields on `Quota` in `quota.rs` are therefore `#[serde(default)]`.

## 8. RFC 9670 JMAP Sharing Examples (November 2024)

RFC 9670 specifies a sharing data model for JMAP. It defines Principals and ShareNotifications to represent access rights and invitations across accounts.

### 8.1 Fixtures

`rfc9670/`, 4 files:

1. `4.1_session_object.json`: Session object that advertises `urn:ietf:params:jmap:principals` with principal account capabilities (§4.1).
2. `4.1_principal_get_request.json`: `Principal/get` request that fetches user principals (§4.1).
3. `4.1_principal_get_response.json`: `Principal/get` response with principal records that have account id, name and email (§4.1).
4. `4.1_todolist_set_request.json`: `TodoList/set` request that creates a shared list with `shareWith` access rights (§4.1).

### 8.2 Errata

RFC 9670 has no errata.

### 8.3 Model notes

Every example round-trips losslessly. No special handling is necessary.

## 9. RFC 9661 JMAP for Sieve Scripts Examples (September 2024)

RFC 9661 specifies a data model to manage Sieve mail filtering scripts through JMAP. It supports the upload, validation, activation and deactivation of scripts.

### 9.1 Fixtures

`rfc9661/`, 16 files:

1. `1.2.2_session.json`: Session object that advertises the `urn:ietf:params:jmap:sieve` capability (§1.2.2).
2. `2.3.1_get_all_request.json`: `SieveScript/get` request that fetches all scripts with `ids: null` (§2.3.1).
3. `2.3.1_get_all_response.json`: `SieveScript/get` response with the active script (§2.3.1).
4. `2.3.1_get_and_blob_get_request.json`: chained `SieveScript/get` and `Blob/get` with a `#ids` backreference (§2.3.1).
5. `2.3.1_get_and_blob_get_response.json`: chained `SieveScript/get` and `Blob/get` response (§2.3.1).
6. `2.4.1_upload_response.json`: `Blob/upload` response for the script upload (§2.4.1).
7. `2.4.1_create_activate_request.json`: `SieveScript/set` request that creates and activates a script with `name: null` and `onSuccessActivateScript: "#A"` (§2.4.1).
8. `2.4.1_create_activate_response.json`: `SieveScript/set` response with the created script and the deactivation update of the previous script (§2.4.1).
9. `2.4.1_update_blob_request.json`: `Blob/upload` and `SieveScript/set` update request (§2.4.1).
10. `2.4.1_update_blob_response.json`: `Blob/upload` and `SieveScript/set` update response (§2.4.1).
11. `2.4.1_rename_deactivate_request.json`: `SieveScript/set` request that renames and deactivates a script with `onSuccessDeactivateScript: true` (§2.4.1).
12. `2.4.1_rename_deactivate_response.json`: `SieveScript/set` rename and deactivate response (§2.4.1).
13. `2.4.1_reactivate_request.json`: `SieveScript/set` request that activates an existing script again (§2.4.1).
14. `2.4.1_reactivate_response.json`: `SieveScript/set` response that confirms the activation (§2.4.1).
15. `2.4.1_deactivate_destroy_request.json`: `SieveScript/set` request that deactivates and destroys a script (§2.4.1).
16. `2.4.1_deactivate_destroy_response.json`: `SieveScript/set` deactivate and destroy response (§2.4.1).

### 9.2 Errata

RFC 9661 has no errata.

### 9.3 Model notes

1. **RFC 9661 §2.1 and §2.4 SieveScript Nullable Name**:
   RFC 9661 §2.1 types `name: String|null (default: null)`. When a client creates a script, it can pass `name: null` or omit `name`. The server then chooses a name. `SieveScript.name` in `sieve.rs` is an `Option<String>`.

2. **Partial Update Deserialization in SetResponse**:
   In `SetResponse<SieveScript>`, partial update responses omit `blobId`. An example is the deactivation of the old script (`updated: { id: { isActive: false } }`). `SieveScript.blob_id` therefore has a default. The serialiser skips it if it is empty.

3. **RFC 9661 §2.4 Deactivate Script Extension Argument**:
   RFC 9661 §2.4 introduces `onSuccessDeactivateScript: Option<bool>` beside `onSuccessActivateScript` on `SieveScriptSetRequest`. `SieveScriptSetRequest` models it.

4. **RFC 9661 §2.3.1 Backreferences in Blob/get**:
   RFC 9661 §2.3.1 pipelines `Blob/get` with a `#ids` backreference to fetch the blob of the script body. This is why `BlobGetRequest` in `blob.rs` has `ids_ref` and an `ids` with a default.

## 10. RFC 9404 JMAP Blob Management Extension Examples (August 2023)

RFC 9404 specifies methods to upload, retrieve and inspect raw data blobs in JMAP.

### 10.1 Fixtures

`rfc9404/`, 11 files:

1. `3.1.1_capability.json`: Session object that advertises the `urn:ietf:params:jmap:blob` capability (§3.1.1).
2. `4.1.1_upload_request.json`: `Blob/upload` request that uploads data as text (§4.1.1).
3. `4.1.1_upload_response.json`: `Blob/upload` response with the created blob record (§4.1.1).
4. `4.1.2_upload_request.json`: `Blob/upload` request with base64 data and a SHA-256 digest (§4.1.2).
5. `4.1.2_upload_response.json`: `Blob/upload` response that confirms the upload (§4.1.2).
6. `4.2.1_get_request.json`: `Blob/get` request that fetches text content (§4.2.1).
7. `4.2.1_get_response.json`: `Blob/get` response with data as text and an encoding problem indicator (§4.2.1).
8. `4.2.2_get_request.json`: `Blob/get` request that fetches a byte range with offset and length (§4.2.2).
9. `4.2.2_get_response.json`: `Blob/get` response with the sliced text (§4.2.2).
10. `4.3.1_lookup_request.json`: `Blob/lookup` request (§4.3.1).
11. `4.3.1_lookup_response.json`: `Blob/lookup` response (§4.3.1).

### 10.2 Errata

RFC 9404 has no errata.

### 10.3 Model notes

1. **RFC 9404 §2.1 BlobInfo Encoding Problem and Truncated Indicators**:
   RFC 9404 §2.1 types `isEncodingProblem: Option<bool>` and `isTruncated: Option<bool>` on `BlobInfo`. `blob.rs` types both.

2. **RFC 9404 §2.2 BlobUploadResponse States**:
   RFC 9404 §2.2 types `oldState: Option<String>` and `newState: Option<String>` on `BlobUploadResponse`. `blob.rs` types both.

3. **Single Account Context AccountId Omission**:
   In the request and response examples of RFC 9404 §4, `accountId` is omitted when the call operates in a single default account context. For this reason, `account_id` has a default and the serialiser skips it if it is empty. This applies to `BlobUploadRequest`, `BlobUploadResponse`, `BlobGetRequest`, `BlobGetResponse`, `BlobLookupRequest` and `BlobLookupResponse`.
