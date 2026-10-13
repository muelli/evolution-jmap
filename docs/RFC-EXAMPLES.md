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
