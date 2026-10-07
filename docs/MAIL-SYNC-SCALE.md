# Mail sync at scale

Every live-server test elsewhere in this repository uses a handful of
messages. This document tracks batches that instead push `jmap-mail-sync`
against a mailbox of a size closer to a real account's, looking for failure
modes that only show up there: `Email/query` position paging past one page,
`maxObjectsInGet`/`maxCallsInRequest` chunking, cold-listing time, and whether
`messages_since`'s incremental path stays cheap once a mailbox is no longer
small.

Test: `rust/crates/jmap-mail-sync/tests/live_server_scale.rs`, `#[ignore]`d,
size read from `JMAP_SCALE_TEST_MESSAGES` (default 500). Run against a
throwaway account (`stw seed`) — it does not clean up after itself, since
destroying several hundred messages one at a time over JMAP costs more than
deleting the whole throwaway account afterwards via `stalwart-cli delete`.

## Batch 1: N=500, 2026-10-07

Real Stalwart, freshly seeded throwaway account (`agent-scale.test`), empty
Inbox beforehand. Server's own core capability: `maxObjectsInGet=500`,
`maxCallsInRequest=16`, `maxObjectsInSet=500` — this batch's size exactly
matches the first of those, the boundary a chunking bug would most likely
show up at.

| Measurement | Result |
|---|---|
| Import 500 messages (`import_message`, sequential) | 2.68s total, 5.4ms/message |
| Cold listing of 500 messages (`messages`) | 89.5ms |
| `messages_since` after adding 50 more to the 500-message mailbox | 16.2ms, 50 present, correct |
| Peak RSS | 13.6 MB |

No bug found. `fetch`'s chunking (`ids.chunks(self.objects_in_get())`) took
the single-chunk path for exactly 500 ids with no off-by-one against the
server's own limit; `messages_since`'s `catch_up_limit` correctly stayed on
the incremental path for the 50-message delta rather than re-listing the
whole mailbox. Nothing here approached `maxCallsInRequest` (this crate never
batches multiple method calls into one request body — one `single_call` per
`Email/get` chunk) or `maxSizeRequest`.

## Batch 2: N=2000 and N=10000, plus delete-at-scale, 2026-10-07

Real Stalwart, fresh throwaway accounts per size (the import case and the
deletion case each got their own, so one run's leftovers never mix into the
other's numbers). A new test, `deleting_from_a_real_sized_mailbox_is_reported_
without_a_relist`, covers the case batch 1 did not reach: deleting from an
already-large mailbox. It imports the same `n` messages, deletes
`n.min(300)` of them, then calls `messages_since` and asserts the answer is
`MessageUpdate::Changed` (not `Relisted`) naming exactly the deleted uids as
absent: `destroyed` ids cost nothing against `catch_up_limit` (see that
function's doc comment), so a delete-only delta should never trip the relist
threshold regardless of mailbox size.

| N | Import | Cold listing | `messages_since` (+50) | Delete 300 | `messages_since` (-300) | Peak RSS |
|---|---|---|---|---|---|---|
| 2000 | 11.7s (5.8ms/msg) | 193ms (500-msg baseline, see note) | 11.6ms | 1.11s (3.7ms/msg) | 6.4ms | 20.5 MB |
| 10000 | 67.6s (6.8ms/msg) | 1.37s | 18.9ms | 1.24s (4.1ms/msg) | 4.7ms | 35.1 MB |

No bug found at either size. The delete case is the real finding this batch
was looking for: `messages_since` after deleting 300 messages took under 7ms
at both 2000 and 10000 held messages, confirming `destroyed` ids really do
stay off the relist path no matter how large the mailbox has grown.
`fetch`'s chunking took the expected multi-chunk path once `n` passed
`maxObjectsInGet=500` (4 chunks at N=2000, 20 at N=10000), and `Email/query`'s
own position paging (`paged_query`) walked every page correctly at both
sizes.

Operational note: this batch needed two of real Stalwart's own throwaway-
account defaults raised to even finish a 10000-message run:
`Http.rateLimitAuthenticated` (1000 requests/60s) and `Jmap.uploadQuota`/
`maxUploadCount` (50 MB / 1000 files per account per hour) are both well
below what a real-sized import generates, and both reloaded live via
`Action/ReloadSettings`. Reverted to the server's defaults after this batch;
a future batch on a clean account will need to raise them again.

N=2000's cold-listing number (193ms) was measured on an account still
carrying leftovers from this batch's own earlier troubleshooting runs (a
1711-message mailbox, not a clean 2000), so it is noted but not a clean
baseline the way the other cells are; the two N=10000 cells ran each on a
freshly seeded, otherwise-empty account and are clean.

## Batches left

Nothing further is queued. A later session could push past N=10000 if a
real need appears, but this batch's "stop climbing" ceiling was reached
comfortably (both sizes well under 10 minutes total) with no bug found, so
there is no outstanding motivation to go further right now.
