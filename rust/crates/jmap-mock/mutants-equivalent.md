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
- `crates/jmap-mock/src/principals.rs:31:5: replace principal_get -> Result<Value, MethodError> with Ok(Default::default())`: not equivalent, because this drops `Principal/get` `list` and `notFound` output and changes observable results for present and missing ids.
- `crates/jmap-mock/src/principals.rs:63:5: replace principal_query -> Result<Value, MethodError> with Ok(Default::default())`: not equivalent, because this drops filtered ids, position, and optional total from `Principal/query`, which changes query results.
- `crates/jmap-mock/src/principals.rs:96:5: replace principal_get_availability -> Result<Value, MethodError> with Ok(Default::default())`: not equivalent, because this suppresses `notFound` and busy-period response data for `Principal/getAvailability`.
- `crates/jmap-mock/src/principals.rs:103:8: delete ! in principal_get_availability`: not equivalent, because this inverts the authorization check and can deny allowed principals while allowing disallowed ones.
- `crates/jmap-mock/src/principals.rs:119:5: replace may_get_availability -> bool with true`: not equivalent, because this ignores capability policy and allows availability reads for principals that should be denied.
- `crates/jmap-mock/src/principals.rs:119:5: replace may_get_availability -> bool with false`: not equivalent, because this denies availability reads for principals that should be allowed by default.
- `crates/jmap-mock/src/principals.rs:130:5: replace busy_period_for -> Option<BusyPeriod> with None`: not equivalent, because this drops every candidate busy period and can return an empty list where events should be reported.
- `crates/jmap-mock/src/principals.rs:141:29: replace || with && in busy_period_for`: not equivalent, because this weakens the window exclusion rule and can include out-of-window events as busy periods.
- `crates/jmap-mock/src/principals.rs:141:14: replace < with == in busy_period_for`: not equivalent, because this changes pre-window exclusion to only exact equality and can include events before `utcStart`.
- `crates/jmap-mock/src/principals.rs:141:14: replace < with > in busy_period_for`: not equivalent, because this flips the start bound comparison and can exclude valid in-window events while including invalid ones.
- `crates/jmap-mock/src/principals.rs:141:14: replace < with <= in busy_period_for`: not equivalent, because this changes bound inclusivity and can incorrectly drop events that start exactly at `utcStart`.
- `crates/jmap-mock/src/principals.rs:141:38: replace >= with < in busy_period_for`: not equivalent, because this flips the end bound and can include events at or after `utcEnd` while excluding in-window ones.
- `crates/jmap-mock/src/principals.rs:146:50: replace == with != in busy_period_for`: not equivalent, because this inverts tentative-status classification and can swap `tentative` and `confirmed` busy status values.
- `crates/jmap-mock/src/principals.rs:168:54: replace + with - in busy_end`: not equivalent, because this subtracts duration seconds instead of adding them and produces incorrect end timestamps.
- `crates/jmap-mock/src/principals.rs:168:54: replace + with * in busy_end`: not equivalent, because this multiplies by duration seconds and can overflow or produce invalid day-second totals.
- `crates/jmap-mock/src/principals.rs:168:39: replace + with - in busy_end`: not equivalent, because this subtracts the minute component and can return incorrect end timestamps for valid inputs.
- `crates/jmap-mock/src/principals.rs:168:49: replace * with / in busy_end`: not equivalent, because this divides minutes by 60 and undercounts minute contribution in end-time arithmetic.
- `crates/jmap-mock/src/principals.rs:215:5: replace share_notification_get -> Result<Value, MethodError> with Ok(Default::default())`: not equivalent, because this drops `ShareNotification/get` results and `notFound` behavior.
- `crates/jmap-mock/src/principals.rs:224:36: replace == with != in share_notification_get`: not equivalent, because this inverts viewer filtering and can return notifications for other recipients while hiding the caller's own notifications.
- `crates/jmap-mock/src/principals.rs:232:56: replace match guard Some(recipient) == viewer with true in share_notification_get`: not equivalent, because this bypasses recipient visibility checks and can leak notifications that should stay hidden.
- `crates/jmap-mock/src/principals.rs:232:56: replace match guard Some(recipient) == viewer with false in share_notification_get`: not equivalent, because this rejects all visible notifications and can report valid ids as `notFound`.
- `crates/jmap-mock/src/principals.rs:232:72: replace == with != in share_notification_get`: not equivalent, because this flips viewer matching and can leak other recipients' notifications while dropping the caller's own entries.
- `crates/jmap-mock/src/principals.rs:258:5: replace share_notification_query -> Result<Value, MethodError> with Ok(Default::default())`: not equivalent, because this drops filtered ids, position, and optional total from `ShareNotification/query`.
- `crates/jmap-mock/src/principals.rs:264:35: replace && with || in share_notification_query`: not equivalent, because this changes visibility-plus-filter logic and can include notifications that fail either viewer or filter checks.
- `crates/jmap-mock/src/principals.rs:264:25: replace == with != in share_notification_query`: not equivalent, because this inverts recipient visibility checks and can return notifications for the wrong principal.
- `crates/jmap-mock/src/principals.rs:295:5: replace share_notification_matches -> bool with true`: not equivalent, because this bypasses `after`, `before`, and `objectType` filters and can over-return notifications.
- `crates/jmap-mock/src/principals.rs:295:5: replace share_notification_matches -> bool with false`: not equivalent, because this rejects all notifications and can force empty results despite matching data.
- `crates/jmap-mock/src/principals.rs:296:42: replace <= with > in share_notification_matches`: not equivalent, because this inverts the `after` bound and can accept stale notifications while rejecting newer ones.
- `crates/jmap-mock/src/principals.rs:301:42: replace >= with < in share_notification_matches`: not equivalent, because this inverts the `before` bound and can accept late notifications while rejecting earlier ones.
- `crates/jmap-mock/src/principals.rs:306:38: replace != with == in share_notification_matches`: not equivalent, because this inverts `objectType` matching and can reject matching entries while accepting mismatches.
- `crates/jmap-mock/src/principals.rs:324:5: replace record_share_changes with ()`: not equivalent, because this suppresses notification creation for rights changes and hides share-change events from clients.
- `crates/jmap-mock/src/principals.rs:336:23: replace == with != in record_share_changes`: not equivalent, because this flips unchanged-versus-changed detection and can create notifications for no-op updates while skipping real changes.
- `crates/jmap-mock/src/principals.rs:356:8: delete ! in record_share_changes`: not equivalent, because this inverts the empty-check and can skip creating notifications when changes exist.
- `crates/jmap-mock/src/principals.rs:367:5: replace principal_matches -> bool with true`: not equivalent, because this bypasses `name`, `email`, and `text` filters and can return non-matching principals.
- `crates/jmap-mock/src/principals.rs:367:5: replace principal_matches -> bool with false`: not equivalent, because this rejects every principal and can force empty query results for matching filters.
- `crates/jmap-mock/src/principals.rs:368:12: delete ! in principal_matches`: not equivalent, because this inverts the `name` substring predicate and can reject matching names while accepting non-matches.
- `crates/jmap-mock/src/principals.rs:373:39: replace != with == in principal_matches`: not equivalent, because this inverts exact-email matching and can reject matching principals while accepting wrong emails.
- `crates/jmap-mock/src/principals.rs:383:12: delete ! in principal_matches`: not equivalent, because this inverts the text fallback predicate and can reject principals that match by name or email text.
- `crates/jmap-mock/src/principals.rs:383:27: replace || with && in principal_matches`: not equivalent, because this requires both name and email text matches and can wrongly reject principals that match only one field.
- `crates/jmap-mock/src/principals.rs:393:9: replace AccountState::seed_principal -> Id with Default::default()`: not equivalent, because this returns an unrelated id and breaks callers that expect the created principal id.
- `crates/jmap-mock/src/principals.rs:401:9: replace AccountState::seed_current_user_principal -> Id with Default::default()`: not equivalent, because this returns an unrelated id and can desynchronize `currentUserPrincipalId` from the id reported to callers.
