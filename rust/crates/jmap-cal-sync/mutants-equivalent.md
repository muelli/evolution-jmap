<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-cal-sync`

## `src/patch.rs`

- `crates/jmap-cal-sync/src/patch.rs:483:43` and `:484:43`: replace the match
  guards `s.is_empty()` and `m.is_empty()` with `false` in
  `diff_virtual_locations`, for the baseline side (`before_norm`). The baseline
  is the server's event after `event_to_ical` and `ical_to_event`. The writer
  does not write a `LABEL` for an empty `name` and does not write a `FEATURE`
  for an empty `features`. The reader then does not set these members. Thus
  the baseline never holds an empty string or an empty object here, and the
  guards never match. `diff` computes the baseline itself, so a caller cannot
  give it a different one.
