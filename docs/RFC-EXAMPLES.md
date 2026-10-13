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
