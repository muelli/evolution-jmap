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
