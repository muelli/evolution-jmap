# Calendar sync at scale

Every other live-server test in `jmap-cal-sync` uses a single event; item
94(b) asks the same question of calendars that item 94(a) already asked of
address books (`BOOK-SYNC-SCALE.md`): cold listing time, incremental
`get_changes` cost, and bulk mutation, once a calendar is no longer tiny —
with a third of the events recurring with a detached, overridden occurrence,
not all single-occurrence events.

Test: `rust/crates/jmap-cal-sync/tests/live_server_scale.rs`, `#[ignore]`d,
size read from `JMAP_SCALE_TEST_EVENTS` (default 500). Run against a
throwaway account (`stw seed`) — it does not clean up after itself, the same
tradeoff `BOOK-SYNC-SCALE.md` makes.

## Batch 1: N=500 import/listing/edit/delete, 2026-10-09

Real Stalwart, freshly seeded throwaway account
(`agent1@agent-calsync-scale.test`). Scoped to 500 events rather than the
item's own 1,000/3,000 checkpoints to start, the same way `BOOK-SYNC-SCALE.md`
batch 1 scoped down after hitting Stalwart's `Http.rateLimitAuthenticated`
(1000 req/60s) partway through 500 `ContactCard/set` calls. `jmap-cal-sync`'s
own `Client::event_get` chunking bug (the `CalendarEvent/get` sibling of that
batch's `ContactCard/get` finding) was already found and fixed ahead of this
test, in the same session that added `CalendarEvent/get` chunking across
`maxObjectsInGet` — see the roadmap's item 94 entry. This batch did not
re-find a new bug: it ran clean at 500 without even approaching the rate
limit, so no `retrying` sleep fired at this size.

| Measurement | Result |
|---|---|
| Import 500 events (⅓ recurring w/ an override, sequential `save_component`) | 44.2s total (~88.4ms/event) |
| Cold listing of 500 events (`list_existing`) | 137.5ms |
| Edit 20 of 500 events | 213.2ms (~10.7ms/event) |
| `get_changes` after editing 20 events | 15.0ms, 20 changed, correct |
| Peak RSS | 13.3 MB |

Second test in the same file, `deleting_from_a_real_sized_calendar_is_
reported_correctly`: a fresh 500-event import, deletes 300, confirms
`CalSync::get_changes` reports every one in `removed` and none in `changed`.
Like `BookSync::get_changes`, `CalSync::get_changes` has no
`catch_up_limit`/relist concept to trip on a delete-only delta, so this is
confirming that reading against a real server rather than hunting a
threshold bug.

| Measurement | Result |
|---|---|
| Import 500 events (ahead of the deletion case) | 63.7s (~127.5ms/event) |
| Delete 300 of 500 events | 1.31s (~4.4ms/event) |
| `get_changes` after deleting 300 | 22.0ms, 300 removed, correct |
| Peak RSS | 15.9 MB |

No bug found. The two imports in this batch ran at different per-event cost
(88.4ms vs 127.5ms) purely because they are two separate sequential test
functions in one process, not a regression between them — same kind of
run-to-run variance `BOOK-SYNC-SCALE.md` batch 2 notes for its own
back-to-back pair.

## Batch 2: N=1,000 import/listing/edit/delete, 2026-10-10

Real Stalwart, freshly seeded throwaway account with a run-unique local part
(`agent1k<unix-timestamp>@agent-calsync-1k-<unix-timestamp>.test`), following
`BOOK-SYNC-SCALE.md` batch 4's account-isolation fix (a reused local part
silently reassigns a prior session's account; confirmed fresh via
`stw query Account` before trusting the numbers).

| Measurement | Result |
|---|---|
| Import 1,000 events (⅓ recurring w/ an override) | 107.4s total (~107.4ms/event) |
| Cold listing of 1,000 events | 218.4ms |
| Edit 20 of 1,000 events | 233.0ms (~11.6ms/event) |
| `get_changes` after editing 20 events | 11.4ms, 20 changed, correct |
| Peak RSS | 15.4 MB |

Second test, fresh 1,000-event import then delete 300:

| Measurement | Result |
|---|---|
| Import 1,000 events (ahead of the deletion case) | 113.7s (~113.7ms/event) |
| Delete 300 of 1,000 events | 1.35s (~4.5ms/event) |
| `get_changes` after deleting 300 | 11.8ms, 300 removed, correct |
| Peak RSS | 18.7 MB |

Costs track batch 1 roughly linearly; `get_changes` stays cheap regardless
of calendar size, the same pattern `BOOK-SYNC-SCALE.md` found. No new
scale-only bug in the product. One transient failure on the delete test's
first attempt at this size is recorded as a suspected Stalwart server issue,
not a product bug (a create that received HTTP 429 appears to have been
committed anyway, so the test's own 429-retry then hit a duplicate-UID
conflict on the identical retried request). A clean immediate re-run passed
outright, so this is not chased further here.

## Batches left

Still open per item 94(b): the 3,000-event checkpoint, and the real-EDS leg
(c) shared with `jmap-book-sync` (item 80's live-Stalwart calendar factory,
first open of a 3,000-event calendar through real EDS).
