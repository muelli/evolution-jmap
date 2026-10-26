# Manual test: a real JMAP server

All other tests run against `jmap-mockd`. The mock server answers only what
its fixtures say. The live-server tests run the same code against a real
deployment. They find what the mock server cannot show:

- unknown capability fields
- enforced limits
- terse `/set` responses
- other server behavior

The live-server tests never run in CI or in a plain `cargo test`. Every live
test is `#[ignore]`. The file of `jmap-client` also needs the `live-server`
feature.

## 1. Get a server

Use any JMAP server where you can create temporary accounts. The tests are
written against Stalwart and Cyrus IMAP.

### Cyrus IMAP oracle container (local Podman)

Run the Cyrus test server in rootless Podman:

```console
$ ./ci/cyrus-oracle.sh start
$ eval $(./ci/cyrus-oracle.sh env)
```

This starts the container and exports `JMAP_LIVE_SERVER_*` variables for
read-only, write, and recipient accounts, ready for running the test suite.
To stop the Cyrus container:

```console
$ ./ci/cyrus-oracle.sh stop
```

### Stalwart or other deployments

In Stalwart, you can create accounts from the admin UI or with
`stalwart-cli`. A Fastmail account works for the read-only tests. Basic
authentication against Fastmail needs an app password.

## 2. Set the environment

```console
$ export JMAP_LIVE_SERVER_URL=https://jmap.example.com
$ export JMAP_LIVE_SERVER_USER=me@example.com
$ export JMAP_LIVE_SERVER_PASSWORD=...
```

For a Bearer token that you got in another way, use this command:

```console
$ export JMAP_LIVE_SERVER_TOKEN=...
```

If both variables are set, `JMAP_LIVE_SERVER_TOKEN` has priority.

In some cases, the session document names an `apiUrl` (or `downloadUrl`,
`uploadUrl`, `eventSourceUrl`) that you cannot reach, but `JMAP_LIVE_SERVER_URL`
works. Possible causes are a reverse proxy, NAT, or a configured public
hostname that differs from the listener. In these cases, set this variable:

```console
$ export JMAP_LIVE_SERVER_REBASE_URLS=1
```

The client then keeps the path of each URL. It uses the scheme and authority
of `JMAP_LIVE_SERVER_URL`. In all other cases, do not set the variable.

## 3. (optional) Enable the write-path tests

Tests that change data run only with a separate login. The read-only settings
of step 2 can therefore never point them at a real mailbox. Create a temporary
account for these tests, on an obvious test domain:

```console
$ export JMAP_LIVE_SERVER_WRITE_USER=agent1@agent-livewrite.net
$ export JMAP_LIVE_SERVER_WRITE_PASSWORD='<that password>'
```

If these variables are not set, the write tests are skipped. They do not fail.

## 3a. (optional) Enable the send-email test and the two-account tests

The sending, sharing and scheduling tests need a second temporary account on
the same server. Delivery then stays on that server, and no outbound relay is
necessary:

```console
$ export JMAP_LIVE_SERVER_RECIPIENT_USER=agent2@agent-livewrite.net
$ export JMAP_LIVE_SERVER_RECIPIENT_PASSWORD='<a different password>'
```

If these variables are not set, those tests are skipped.

## 3b. (optional) Enable the quota test

The quota tests need a disk quota for each account on the write-test account.
A server can advertise `urn:ietf:params:jmap:quota` and still report no
`Quota` object for an account without a quota. Set a quota on the server first
(in Stalwart, `maxDiskQuota` of the account). If the account has no quota, the
tests are skipped.

## 4. Run it

Run the protocol client with the feature gate:

```console
$ cd rust
$ cargo test -p evolution-jmap-client --features live-server --test live_server -- --ignored
```

In these crates, only the live-server tests are ignored. `--ignored` therefore
selects them:

```console
$ cargo test -p evolution-jmap-book-sync -- --ignored
$ cargo test -p evolution-jmap-cal-sync -- --ignored
$ cargo test -p evolution-jmap-collection-sync -- --ignored
$ cargo test -p evolution-jmap-mail-sync -- --ignored
$ cargo test -p jmap-config --test live_server_discover_and_register -- --ignored
```

To run one file, use `--test <file>`. For example:
`cargo test -p evolution-jmap-cal-sync --test live_server_shared_freebusy -- --ignored`.

`jmap-mail`, `jmap-backend-core`, `jmap-backend-book`, `jmap-backend-cal` and
`jmap-backend-collection` also have `tests/live_server*.rs` files. They need
the EDS development headers, like the other tests of those crates. The
`live-stalwart-*` tests of `jmap-functional` drive real EDS daemons against the
server. For their environment, see `docs/functional-tests.md`.

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
| CalendarEvent/parse differential fidelity across iCalendar fixtures | `jmap-client/tests/live_server.rs` | step 3 |
| Meeting invitations | `jmap-cal-sync/tests/live_server_scheduling.rs` | steps 3 and 3a |
| Undo window of a delete: no destroy before the due time, undo, refused destroy (takes about one minute) | `jmap-backend-collection/tests/live_server_deferred_delete.rs` | steps 3 and 3a |
| OAuth 2.0 discovery and dynamic client registration | `jmap-config/tests/live_server_discover_and_register.rs` | step 2 (URL only) |

### The write-path test

Each write test creates its own data on the write-test account. It checks the
data with `/get` (and, for most types, `/query` and `/changes`). Then it
destroys the data. The contact and event tests use the default address book
and the default calendar of the account.

### The send-email test

`send_email_delivers_to_a_second_account_on_the_real_server` sends a message
with `Client::send_email` (`Email/set` and `EmailSubmission/set`). The message
goes from the write-test account to the recipient account. The test then polls
the Inbox of the recipient until the message arrives. This proves delivery. An
accepted submission alone does not prove delivery.

### The free/busy test

`jmap-cal-sync/tests/live_server.rs` creates an event that lasts one hour. It
calls `CalSync::free_busy` for the own address of the account and for an
address that does not exist. The `VFREEBUSY` must name the account with a
`FREEBUSY;FBTYPE=BUSY` line for exactly the window of the event. The address
that does not exist must be absent.

`jmap-cal-sync/tests/live_server_shared_freebusy.rs` asks about another
account, as a meeting scheduler does. The recipient account asks about the
write-test account three times:

1. Before a share: no `FREEBUSY` line. RFC 9670 makes this look like an empty
   calendar.
2. After a `shareWith` grant with only `mayReadFreeBusy`: the window of the
   event appears.
3. After the grant is revoked: no line again.

## What "it worked" means

All tests pass, or they report that they were skipped because an optional
account is missing. A failure is a finding. Report it like any other bug.
