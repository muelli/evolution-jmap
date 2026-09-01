<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Stale-comments audit — comments that no longer match the code

Read-only sweep of `rust/crates/**` (doc comments and inline comments; not
the project's log and planning documents, which are logs, not code
comments), 2026-08-19, per Track A7. Goal: find
comments describing an outdated state — renamed/removed items, changed
behaviour, done TODOs, resolved-milestone references ("once M7 lands" when M7
is done), calcard/percent-encoding migration leftovers — and fix the
high-confidence ones in the same pass.

## Headline

Seven HIGH-confidence stale comments found and fixed, all in the same family:
prose written while M7 (`insert_widgets`, the OAuth2 `EConfigLookupWorker`,
`check_complete`'s insecure-transport refusal) was still in progress, never
updated once the described gap closed. One MEDIUM-confidence finding — a
claim that M9's Xvfb tier exercises the account-setup page, which it does
not (it seeds a pre-built `.source` file instead) — fixed alongside the HIGH
ones since the correction is small and precise. No calcard or
percent-encoding leftovers were found: both migrations' comments accurately
describe the current, migrated code. No milestone-reference false claims
were found outside the M7/config-lookup family above.

## Fixed (HIGH confidence)

1. **`jmap-config/tests/textdomain.rs:17`** — said "None of them exists yet —
   `insert_widgets` is unwritten"; `insert_widgets` (`jmap-config/src/backend.rs`)
   has built every field for some time (`src/lib.rs:125`: "now written").
   Corrected to describe the binding's ordering requirement without the false
   "unwritten" claim.
2. **`jmap-config/src/module.rs:94`** — same claim, second file: "…which
   `insert_widgets` has yet to put on screen." Same fix.
3. **`jmap-config/src/backend.rs`'s `insert_widgets` doc, "## Untestable
   here" section** — said this needs "a real Evolution session (or M9's
   Xvfb tier)" and is "not tagged complete until a human confirms it." M7 is
   `COMPLETE` (`c3cac2d`) via two operator rounds in real Evolution; the
   "M9's Xvfb tier" parenthetical was also wrong (see MEDIUM finding below).
   Rewritten to state the human confirmation that already happened and drop
   the inaccurate Xvfb claim.
4. **`jmap-backend-collection/tests/oauth2_service.rs:94`** — "…what a real
   JMAP account's `[Authentication] method` would be set to once M7's setup
   UI writes OAuth2 accounts" (conditional future tense). M7's setup UI does
   this today; reworded to present tense.
5. **`jmap-client/src/oauth.rs:28-31`** — "…the vfuncs that wire any of this
   to EDS need the `EOAuth2Service` interface, which is a later slice."
   `jmap-config/src/oauth2_service.rs` implements `EOAuth2Service` and is
   registered (`jmap-config/src/module.rs`); reworded to name it as built,
   keeping the still-true point that a real browser/provider consent round
   trip is what remains unexercised here.
6. **`jmap-config/tests/account.rs:241-242`** — "The UI half of it… is
   `check_complete`'s, and is not written yet." `check_complete`
   (`jmap-config/src/complete.rs::check`) already refuses an insecure
   transport, tested by
   `tests/complete.rs::plaintext_to_a_server_that_is_not_this_machine_is_refused`.
   Reworded to point at that test instead of claiming the gap is open.
7. **`jmap-config/src/config_lookup.rs`'s "## What is not yet proven"
   section** and **`evo-sys/tests/config_lookup.rs`**'s matching note — both
   said `run()`'s live dispatch through a real `EConfigLookup` was "left for
   that harness" / would be exercised by "the next increment's own test."
   That increment landed:
   `jmap-functional/tests/config-lookup.rs` ("M9 layer 1, config lookup")
   already drives a real `EConfigLookup` running `JmapConfigLookup` against
   the mock's OAuth 2.0 endpoints. Both comments reworded to point at the
   test that now provides this coverage instead of describing it as future
   work.

## Fixed (MEDIUM confidence)

8. **`evo-sys/src/lib.rs:60`** and **`evo-sys/tests/gtk.rs:11-12`** — both
   said the account-setup page "is only exercisable under M9's Xvfb tier."
   Checked `ci/gui-smoke.sh`: M9's Tier-2 GUI smoke test copies three
   pre-built `.source` keyfiles into place rather than driving the account
   assistant, so it never actually calls `insert_widgets`. The literal claim
   implied M9 covers this page; it doesn't — only a human running the
   assistant in real Evolution does (which is exactly what happened for M7's
   sign-off). Medium confidence because a narrower
   reading of the original sentence ("needs a display, which only that tier
   or a human supplies") isn't strictly false — but as written it reads as
   "M9's tier exercises this," which it doesn't. Both reworded to say so
   plainly.

## Looked suspicious but fine

- `jmap-config/src/backend.rs`'s "## What is not here yet" section (real-
  Evolution visual verification, OAuth2 consent round trip) — still
  genuinely open gaps, correctly described.
- `jmap-backend-collection/src/lib.rs` / `jmap-config/src/backend.rs`'s
  `commit_changes` doc ("the collection backend… is the next increment") —
  cross-checked against NIGHT-LOG; still an open, correctly-described gap
  (mail-source host not filled by the collection backend on the assistant
  path).
- `jmap-ical`/`jmap-proto` `BYxxx`/RFC-citation comments, `jmap-cal-sync`/
  `jmap-backend-cal` test docs — describe current, deliberate parsing/
  rendering behaviour, not stale.
- `jmap-mail-sync/tests/hostile.rs` "calendar arithmetic… is not written for
  them" — describes intentional non-support of absurd years; still accurate.
- `jmap-mail-sync/src/keywords.rs`, its tests, `jmap-mail/tests/message_info.rs`
  — `todo`/`home/todo` are literal JSON-pointer/tag test fixtures, not TODO
  markers.
- `jmap-mail/src/{synchronize,provider,summary,folders}.rs` — "not yet
  written back" is Camel's own terminology for its summary-dirty state, not a
  gap in this project's code.
- Various "unwritten" references in `jmap-config/src/{backend,mail}.rs`,
  `jmap-collection-sync/src/child_source.rs`, `jmap-backend-core/src/{source,i18n}.rs`
  — all describe the deliberate absent-vs-empty keyfile semantics, still
  correct.
- `po-compile/src/lib.rs` — a genuinely unsupported PO construct, accurate.
- `jmap-backend-collection/src/{prepare_mail,factory}.rs`,
  `jmap-mail/src/cache.rs`, `jmap-mail/tests/recipe.rs`,
  `jmap-mock/tests/mockd_oauth2.rs` — correctly reference M6/M7 as
  already-delivered milestones.
- `eds-sys/{tests/layout.rs,src/lib.rs,build.rs}` — M10 references describe
  its actual, current matrix behaviour; accurate (M10 is `COMPLETE`).
- `jmap-backend-cal/tests/recipe.rs` — describes the deliberate, current
  VTODO/VJOURNAL non-support (matches ROADMAP's "Not doing (protocol-gated)").
- calcard/percent-encoding sweep: no leftovers. `jmap-client/src/url.rs` and
  `jmap-mock/src/server.rs` both already use the `percent_encoding` crate,
  matching their doc comments; all `calcard` references describe the current
  parser, not a pre-migration hand-rolled one.
- Routine M-number provenance references in `jmap-config/src/{lib,config_lookup}.rs`,
  `jmap-backend-collection/src/module.rs`, and the various `tests/connect.rs`
  files — accurate.

## Method

Grepped `rust/crates` and `docs` for TODO/FIXME/XXX, "not yet
implemented/written/done", "unwritten", "once M\d+ lands"/"when M\d+ lands",
"has yet to", and similar phrasings; cross-checked every hit's file:line
against the current code it describes and against the milestones ledger
(all milestones M1–M10 plus `CALCARD` are `COMPLETE`). Ran a second,
independent sweep for bare milestone references (`M7`, `M9`, `M10`) and for
`calcard`/`percent` across `rust/crates/**/*.rs` to catch stale claims the
first grep's wording wouldn't. Verified the higher-risk findings (the M9
Xvfb-tier claim, the config-lookup "not yet proven" section) by reading
`ci/gui-smoke.sh` and `jmap-functional/tests/config-lookup.rs` directly
rather than trusting the comment's own framing.

## 2026-09-01 — re-audit after the item 33(d) sweep and the repository split

Re-checked this file's own 2026-08-19 findings against today's tree (all
still hold; the code they describe has not moved) and swept `rust/`,
`cmake/` and `ci/` fresh for the two things that changed since: the item
33(d) sweep (dozens of module headers rewritten "to state the fact instead
of citing the log") and the 2026-09-01 repository split (every `infra/`
path moved to the harness repository's `harness/`, and the harness
bookkeeping files moved out of this repository entirely). Grepped for
TODO/FIXME/XXX, "not yet"/"unwritten"/"has yet to" phrasings (same list as
the first pass, all findings already-known-fine — see below), `.rs:<N>`
file:line citations in comments, and stale `infra/`/`harness/` path
references.

### Fixed

1. **`jmap-client/tests/live_server.rs`, three citations of
   `infra/gcp/create-stalwart.sh` and `infra/stalwart/stw seed`** — both
   scripts moved to the harness repository's `harness/gcp/` and
   `harness/stalwart/` in the split; `infra/` no longer exists anywhere.
   Reworded to name the harness repository instead of a path this
   repository cannot resolve.
2. **`jmap-client/tests/live_server.rs:483` and `:716`, two drifted
   `lib.rs:<N>` citations** into `jmap-book-sync`'s and `jmap-cal-sync`'s
   `list_existing`/`get_changes`: the `ContactCard/changes` citation named
   `lib.rs:153`, a line inside an unrelated function, when the
   `all_changes(&self.account_id, "ContactCard", since)` call it describes
   is at `lib.rs:206`; the `CalendarEventQueryFilter::in_calendar` citation
   named `lib.rs:98` (the line above the call) instead of `lib.rs:101` (the
   call itself). Both corrected to the current line numbers.
3. **`jmap-client/tests/live_server.rs:219`'s doc on `connect_for_write`**
   said "Credentials for the one test in this file that writes" — the
   function is called by all eight mutating tests (grepped every call
   site), not one. This reads like leftover text from when the live-server
   suite had a single write test, before it grew. Reworded to "the tests…
   that write".
4. **`ci/checks.sh`'s repository-split boundary-lint comment** described
   itself as "not yet an enforced boundary… a progress meter for the
   sweep… flip to hard-fail only once the count is zero" — but the code
   right below it already does hard-fail (`exit 1` on any match), and the
   failure message a few lines down says outright "the sweep reached zero
   on 2026-08-31 and this lint now enforces the boundary." The header
   comment was describing the pre-enforcement state of its own check,
   contradicting the code and the message beneath it. Rewritten to state
   what the check does now.
5. **`docs/manual-test-live-server.md`** — same `infra/` drift as (1), five
   occurrences, plus two stale test counts: "A fifth test," describing
   `send_email_delivers_to_a_second_account_on_the_real_server`, when it
   is preceded by a list of seven (not four) write tests; and "The six
   mutating tests are skipped," when that list names seven. Reworded the
   count-dependent sentence to not name a number at all, so it cannot drift
   again the same way, and corrected "six" to "seven". Out of this item's
   named scope (`rust/`, `cmake/`, `ci/`) but fixed alongside since it is
   the doc these very comments point readers to, describing the same
   drift, at no separate verification cost.

### Looked at, not touched

- `po-compile/tests/gettext.rs`'s `#: rust/crates/jmap-mail/src/provider.rs:65`
  line inside a `.po` fixture string: the real line is now 67. Not a
  comment about this project's code, a piece of realistic-looking `.po`
  syntax the test feeds to glibc's `gettext`, which never reads or checks
  gettext reference comments; the fixture's correctness does not depend on
  the number. Left as is.
- 33(d)'s rewrites: sampled `jmap-mail` (`push.rs`, `store.rs`) and
  `jmap-backend-cal` (`backend.rs`) directly against `git show` for the
  commits that touched them — each is a mechanical citation drop with no
  behaviour-claim change, matching the sweep's stated goal. Did not diff
  every one of the ~20 commits; the sampled ones give no reason to expect
  the others differ in kind.
- Every "not yet"/"unwritten"/"has yet to" hit from a fresh grep matches
  the first pass's "looked suspicious but fine" list — deliberate
  Camel/keyfile absent-vs-empty semantics, literal JSON-pointer/tag test
  fixtures, current parsing behaviour. No new hits outside that set.
- `// SAFETY:` comments (2242 of them across ~80 files): grepped for
  `file:line`-style citations that could have drifted the same way the
  `live_server.rs` ones had; none exist; a SAFETY comment cites the
  invariant it relies on, not a location. Verifying that each invariant
  still holds against today's code is the recurring FFI/unsafe audit's own
  job (Track A5/A6, the periodic re-audit that is item 42 this round,
  explicitly reserved for escalation) and out of this pass's reach at this
  scale; not attempted here.
- No `milestones.rs`, `docs/NIGHT-LOG`, `docs/BACKLOG`, `AGY-TASKS` or
  other harness-bookkeeping path reference found anywhere in `rust/`,
  `cmake/` or `ci/` — the boundary lint's own zero count still holds.

### Method

Same grep list as the first pass, plus three new sweeps this round's own
risk areas called for: `grep -rEn '[A-Za-z0-9_/-]+\.rs:[0-9]+'` across
`rust/crates/**/*.rs` for file:line citations, checked each one against the
file and line it names; `grep -rn 'infra/'` across the whole repository;
and a read of every `item 33(d)` commit's `--stat` plus a sample of the
larger diffs. `ci/checks.sh` and `cmake/` had no hits from any grep in this
pass.
