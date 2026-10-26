<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent mutants in `jmap-mock`

- `crates/jmap-mock/src/dispatch.rs:266:9: delete match arm "ContactCard/parse" in handle_method`: equivalent in the default-feature mutant run because `contact_card_parse` itself returns `unknownMethod` when `parse` is disabled, so removing this arm and falling back to the default `unknownMethod` path yields the same observable response.
- `crates/jmap-mock/src/dispatch.rs:353:9: delete match arm "CalendarEvent/parse" in handle_method`: equivalent in the default-feature mutant run because `calendar_event_parse` itself returns `unknownMethod` when `parse` is disabled, so removing this arm and falling back to the default `unknownMethod` path yields the same observable response.
