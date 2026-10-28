<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent mutants in `jmap-mock`

- `crates/jmap-mock/src/dispatch.rs:266:9: delete match arm "ContactCard/parse" in handle_method`: equivalent in the default-feature mutant run because `contact_card_parse` itself returns `unknownMethod` when `parse` is disabled, so removing this arm and falling back to the default `unknownMethod` path yields the same observable response.
- `crates/jmap-mock/src/dispatch.rs:353:9: delete match arm "CalendarEvent/parse" in handle_method`: equivalent in the default-feature mutant run because `calendar_event_parse` itself returns `unknownMethod` when `parse` is disabled, so removing this arm and falling back to the default `unknownMethod` path yields the same observable response.

# Missed mutants in `jmap-mock`

Batch 13 expects each missed mutant to have either a killing test or a one-line reason.
This lane is comment-only in `rust/crates/jmap-mock`, so this file records reasons.

- `crates/jmap-mock/src/time.rs:29:57: replace || with && in parse_utc`: not equivalent, because this weakens timestamp field validation and can accept malformed UTC times that should return `None`; the current parse-enabled suite does not fail on this path.
- `crates/jmap-mock/src/time.rs:55:38: replace - with + in days_from_civil`: not equivalent, because this changes month-to-day conversion for January and February and can map valid dates to wrong Unix days outside the currently asserted cases.
- `crates/jmap-mock/src/time.rs:55:38: replace - with / in days_from_civil`: not equivalent, because this changes month-to-day conversion arithmetic and can map valid dates to wrong Unix days outside the currently asserted cases.
- `crates/jmap-mock/src/time.rs:65:62: replace - with + in civil_from_days`: not equivalent, because this changes the year/day decomposition and can format valid Unix timestamps into wrong calendar dates outside the currently asserted cases.
- `crates/jmap-mock/src/time.rs:65:62: replace - with / in civil_from_days`: not equivalent, because this changes the year/day decomposition and can format valid Unix timestamps into wrong calendar dates outside the currently asserted cases.
- `crates/jmap-mock/src/time.rs:65:41: replace + with - in civil_from_days`: not equivalent, because this changes the year/day decomposition and can format valid Unix timestamps into wrong calendar dates outside the currently asserted cases.
- `crates/jmap-mock/src/quota.rs:21:5: replace quota_get -> Result<Value, MethodError> with Ok(Default::default())`: not equivalent, because this drops the RFC 9425 `Quota/get` response body and `notFound` reporting, which changes observable method results for existing and unknown ids.
- `crates/jmap-mock/src/quota.rs:46:5: replace quota_query -> Result<Value, MethodError> with Ok(Default::default())`: not equivalent, because this drops filtered ids, position, and optional total from `Quota/query`, which changes observable pagination and filtering output.
- `crates/jmap-mock/src/quota.rs:78:5: replace quota_matches -> bool with true`: not equivalent, because this bypasses all filter checks and can return quota ids that do not match requested `name`, `scope`, `resourceType`, or `type`.
- `crates/jmap-mock/src/quota.rs:78:5: replace quota_matches -> bool with false`: not equivalent, because this rejects every quota and can force empty query results where matching ids should be returned.
- `crates/jmap-mock/src/quota.rs:79:12: delete ! in quota_matches`: not equivalent, because this inverts the `name` substring rule and can reject quotas that should match while accepting non-matches.
- `crates/jmap-mock/src/quota.rs:84:32: replace != with == in quota_matches`: not equivalent, because this inverts the `resourceType` match predicate and can reject matching quotas while admitting mismatched ones.
- `crates/jmap-mock/src/quota.rs:89:24: replace != with == in quota_matches`: not equivalent, because this inverts the `scope` match predicate and can reject matching quotas while admitting mismatched ones.
- `crates/jmap-mock/src/quota.rs:94:12: delete ! in quota_matches`: not equivalent, because this inverts the `type` membership check and can reject quotas with the requested type while accepting quotas that lack it.
- `crates/jmap-mock/src/quota.rs:94:42: replace == with != in quota_matches`: not equivalent, because this inverts per-entry equality in the `types` membership scan and can misclassify whether a quota supports the requested type.
