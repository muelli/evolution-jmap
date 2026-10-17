# Manual test: a real JMAP server

All other tests run against `jmap-mockd`, which answers only what its
fixtures say. The live-server tests run the same code against a real
deployment, to find what the mock cannot show: unknown capability fields,
enforced limits, terse `/set` responses and other server behaviour.

They never run in CI or in a plain `cargo test`: every live test is
`#[ignore]`, and `jmap-client`'s file also needs the `live-server` feature.

## 1. Get a server

Use any JMAP server where you can create throwaway accounts. The tests are
written against Stalwart, which lets you create accounts from its admin UI
or `stalwart-cli`. A Fastmail account works for the read-only tests (basic
auth against Fastmail needs an app password).

## 2. Set the environment

```console
$ export JMAP_LIVE_SERVER_URL=https://jmap.example.com
$ export JMAP_LIVE_SERVER_USER=me@example.com
$ export JMAP_LIVE_SERVER_PASSWORD=...
```

Or, for a Bearer token that you got some other way:

```console
$ export JMAP_LIVE_SERVER_TOKEN=...
```

`JMAP_LIVE_SERVER_TOKEN` wins if both are set.

If the session document names an `apiUrl` (or `downloadUrl`, `uploadUrl`,
`eventSourceUrl`) that you cannot reach, although `JMAP_LIVE_SERVER_URL`
works (a reverse proxy, NAT, or a configured public hostname that differs
from the listener), set:

```console
$ export JMAP_LIVE_SERVER_REBASE_URLS=1
```

The client then keeps each URL's path but uses the scheme and authority of
`JMAP_LIVE_SERVER_URL`. Leave it unset otherwise.

## 3. (optional) Enable the write-path tests

Tests that change data run only with a separate login, so that the
read-only settings of step 2 can never point them at a real mailbox. Create
a throwaway account for them, on an obvious test domain:

```console
$ export JMAP_LIVE_SERVER_WRITE_USER=agent1@agent-livewrite.net
$ export JMAP_LIVE_SERVER_WRITE_PASSWORD='<that password>'
```

Without these, the write tests are skipped, not failed.

## 3a. (optional) Enable the send-email test and the two-account tests

Sending, sharing and scheduling tests need a second throwaway account on the
same server, so that delivery stays on that server and needs no outbound
relay:

```console
$ export JMAP_LIVE_SERVER_RECIPIENT_USER=agent2@agent-livewrite.net
$ export JMAP_LIVE_SERVER_RECIPIENT_PASSWORD='<a different password>'
```

Without these, those tests are skipped.

## 3b. (optional) Enable the quota test

The quota tests need a per-account disk quota on the write-test account. A
server can advertise `urn:ietf:params:jmap:quota` and still report no
`Quota` object for an account without a quota, so set one on the server
first (in Stalwart, the account's `maxDiskQuota`). Without a quota, the
tests are skipped.

## 4. Run it

The protocol client, with the feature gate:

```console
$ cd rust
$ cargo test -p evolution-jmap-client --features live-server --test live_server -- --ignored
```

In these crates only the live-server tests are ignored, so `--ignored`
selects them:

```console
$ cargo test -p evolution-jmap-book-sync -- --ignored
$ cargo test -p evolution-jmap-cal-sync -- --ignored
$ cargo test -p evolution-jmap-collection-sync -- --ignored
$ cargo test -p evolution-jmap-mail-sync -- --ignored
$ cargo test -p jmap-config --test live_server_discover_and_register -- --ignored
```

To run one file, use `--test <file>`, for example
`cargo test -p evolution-jmap-cal-sync --test live_server_shared_freebusy -- --ignored`.

`jmap-mail`, `jmap-backend-core`, `jmap-backend-book`, `jmap-backend-cal`
and `jmap-backend-collection` also have `tests/live_server*.rs` files. They
need the EDS development headers, like the rest of those crates. The
`jmap-functional` `live-stalwart-*` tests drive real EDS daemons against the
server; see `docs/functional-tests.md` for their environment.

The scale tests (`live_server_scale.rs` in `jmap-book-sync`, `jmap-cal-sync`
and `jmap-mail-sync`) take their sizes from `JMAP_SCALE_TEST_CARDS`,
`JMAP_SCALE_TEST_EVENTS` and `JMAP_SCALE_TEST_MESSAGES`.

## What the tests cover

| Area | Files | Accounts |
| --- | --- | --- |
| Session, `Core/echo`, listing mailboxes, address books, calendars | `jmap-client/tests/live_server.rs` (read-only tests) | step 2 |
| Create, update, destroy and `/changes` for each type; blobs; limits; back-references; push reconnect | `jmap-client/tests/live_server.rs` | step 3 |
| Contact mapping, one vCard property per file | `jmap-book-sync/tests/live_server_*.rs` | step 3 |
| Event save and remove, colour, `/changes` | `jmap-cal-sync/tests/live_server{,_save,_color,_changes}.rs` | step 3 |
| Address book and calendar create and delete | `jmap-collection-sync/tests/live_server.rs` | step 3 |
| Mail import, expunge, folders, keywords, filing, search | `jmap-mail-sync/tests/live_server_*.rs` | step 3 |
| Sending between two accounts | `jmap-client` (`send_email_delivers_to_a_second_account_on_the_real_server`), `jmap-mail-sync/tests/live_server_send.rs` | steps 3 and 3a |
| Shared address books, calendars and mailboxes | `*/tests/live_server_shared*.rs` | steps 3 and 3a |
| Meeting invitations | `jmap-cal-sync/tests/live_server_scheduling.rs` | steps 3 and 3a |
| OAuth 2.0 discovery and dynamic client registration | `jmap-config/tests/live_server_discover_and_register.rs` | step 2 (URL only) |

### The write-path test

Each write test creates its own data on the write-test account, checks it
with `/get` (and, for most types, `/query` and `/changes`), and destroys it.
Contact and event tests use the account's default address book and calendar.

### The send-email test

`send_email_delivers_to_a_second_account_on_the_real_server` sends a message
with `Client::send_email` (`Email/set` and `EmailSubmission/set`) from the
write-test account to the recipient account. It then polls the recipient's
Inbox until the message arrives, which proves delivery, not only an accepted
submission.

### The free/busy test

`jmap-cal-sync/tests/live_server.rs` creates a one-hour event and calls
`CalSync::free_busy` for the account's own address and a nonexistent
address. The `VFREEBUSY` must name the account with a `FREEBUSY;FBTYPE=BUSY`
line for exactly the event's window. The nonexistent address must be absent.

`jmap-cal-sync/tests/live_server_shared_freebusy.rs` asks about another
account, as a meeting scheduler does. The recipient account asks about the
write-test account three times: before a share (no `FREEBUSY` line, which
RFC 9670 makes look like an empty calendar), after a `mayReadFreeBusy`-only
`shareWith` grant (the event's window appears), and after the grant is
revoked (no line again).

## What "it worked" means

All tests pass or report that they were skipped for a missing optional
account. A failure is a finding: report it like any other bug.
