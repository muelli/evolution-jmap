# Address book sync at scale

Every other live-server test in `jmap-book-sync` uses a single contact; item
94(a) asks the same question of address books that item 89 already asked of
mailboxes: cold listing time, incremental `get_changes` cost, and bulk
mutation, once a book is no longer tiny.

Test: `rust/crates/jmap-book-sync/tests/live_server_scale.rs`, `#[ignore]`d,
size read from `JMAP_SCALE_TEST_CARDS` (default 500). Run against a
throwaway account (`stw seed`) — it does not clean up after itself, since
destroying several hundred cards one at a time over JMAP costs more than
deleting the whole throwaway account afterwards.

## Batch 1: N=500 import/listing/edit, 2026-10-09

Real Stalwart, freshly seeded throwaway account. `Client::contact_get` had
no `maxObjectsInGet` chunking at all before this batch, so the cold listing
of 500 cards came back `requestTooLarge` from the real server instead of a
listing — the one real bug this batch found, fixed in `jmap-client`
(mirroring `email_get`'s own chunking) with mock-backed regression coverage,
then re-verified live.

| Measurement | Result |
|---|---|
| Import 500 cards (`save_contact`, sequential) | 17.8s total (~35.6ms/card) |
| Cold listing of 500 cards (`list_existing`) | 257ms |
| Edit 20 of 500 cards | 179ms |
| `get_changes` after editing 20 cards | 9ms, 20 changed, correct |
| Peak RSS | 17.7 MB |

Hit Stalwart's `Http.rateLimitAuthenticated` (1000 req/60s) partway through
the 500-card import (a 429 on call #498, 2.2 seconds in) even on the default
throwaway plan; the test's own `retrying` helper absorbs that with a short
sleep, same shape `jmap-mail-sync`'s equivalent needed at far larger N.

## Batch 2: bulk delete, 2026-10-09

A fresh throwaway account (`agent1@agent-booksync-del.net`), so this batch's
own import never mixes with batch 1's leftovers. New test,
`deleting_from_a_real_sized_address_book_is_reported_correctly`: imports N
cards, deletes `n.min(300)` of them, then calls `BookSync::get_changes` and
asserts every deleted uid is reported in `removed` (not `changed`).

Unlike `jmap-mail-sync`'s `messages_since`, `BookSync::get_changes` has no
`catch_up_limit`/relist concept to trip in the first place: `classify` only
ever calls `ContactCard/get` for the created/updated union, so a delete-only
delta costs nothing beyond the `/changes` call itself, by construction
rather than by a threshold that could be gotten wrong. Worth checking against
a real server anyway rather than trusting that reading of the mock-tested
code — confirmed clean.

| N | Import | Delete 300 | `get_changes` (-300) | Peak RSS |
|---|---|---|---|---|
| 500 | 57.7s (115ms/card, see note) | 852ms (2.8ms/card) | 6.4ms, 300 removed, correct | 16.7 MB |

No bug found. `get_changes` after deleting 300 of 500 cards took under 7ms,
confirming deletions stay cheap regardless of book size, the same finding
`messages_since` gave for mail at far larger N (`MAIL-SYNC-SCALE.md` batch
2) — expected here, since book-sync's `/changes` handling never had a
size-dependent relist path to begin with.

Note on the 115ms/card import figure: this run shared the same 60-second
rate-limit window as batch 1's own import test when both ran in the same
`cargo test` invocation, so a larger fraction of its 500 creates landed on
the 5-second `retrying` sleep than batch 1 saw alone; the per-card cost is a
rate-limit artifact of running both tests back to back, not a regression in
import itself (batch 1's own number, 500 cards with the window to itself,
stands at ~35.6ms/card).

## Batch 3: PHOTO-blob variant, 2026-10-10

A fresh throwaway account, so this batch's own import never mixes with
batches 1 or 2's leftovers. New test,
`a_real_sized_address_book_with_photos_round_trips_through_the_real_server`:
imports 500 cards, a tenth of them (50) with an inline vCard 3.0
`PHOTO;ENCODING=b;TYPE=JPEG:` of ~50 KB base64, then checks every photo card
is in the cold listing and its PHOTO payload round-trips byte-identical.

A PHOTO never goes through JMAP's `Blob/upload` in this codebase: `jmap-vcard`
inlines it straight into `ContactCard/set`'s JSContact `media` map
(`BookSync::save_contact`, `jmap-vcard`'s `vcard_to_card`), and no size limit
is enforced anywhere in `jmap-mock` for this path (`ContactsCapability`'s
`max_size_attachments_per_card` is advertised but never read).

| Measurement | Result |
|---|---|
| Import 500 cards (50 with a ~50 KB PHOTO) | 29.7s total (~59.5ms/card) |
| Cold listing | 1.0s |
| Peak RSS | 32.5 MB |

No scale-only bug found: a 2.5 MB-ish PHOTO total across the batch costs more
than an all-text batch (peak RSS roughly doubled versus batch 1's 17.7 MB)
but nothing broke or hit a server limit. One test-harness snag, not a product
bug: the first version of this test compared the server's returned vCard
text directly against the original base64 string and failed, because
`jmap-vcard`'s `card_to_vcard` folds any physical line over 75 octets per RFC
2426 §2.6 (`fold_overlong_lines`), so a ~67 KB base64 payload always comes
back line-folded even though its octets are unchanged. Fixed by unfolding
(`vcard.replace("\r\n ", "")`) before comparing; this is a test-design
correction, not a code change to the product.

## Batches left

Still open per item 94(a): the 1,000/5,000-card checkpoints, `jmap-cal-sync`'s
(b), and the real-EDS leg (c) (item 80's live-Stalwart book factory, first
open of a 5,000-card book through real EDS).
