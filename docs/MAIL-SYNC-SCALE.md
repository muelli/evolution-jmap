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

## Batches left

N=2000 and N=10000 are still open — this batch's numbers (sub-3s import,
sub-100ms listing) suggest both are comfortably reachable in one sitting
("stop climbing when a step takes over ~10 minutes" has a lot of headroom
left). A future batch should also exercise an incremental `/changes` after
*deleting* messages from an already-large mailbox, which this batch did not
reach.
