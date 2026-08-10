# Functional tests: real EDS daemons against the mock

Every other test in this repository stops at the edge of Evolution Data
Server. It calls a vfunc body directly, or holds a mapping against a fixture.
That is most of the value for most of the cost — but it leaves one layer
untested, and it is the layer a user meets first: EDS deciding *when* to call
those vfuncs, and what it makes of what they said.

These tests cover that layer. They start a real `evolution-source-registry`
and a real host for the module under test — `evolution-addressbook-factory`,
`evolution-calendar-factory`, or, for the Camel mail provider, the client
program itself — hand it a real build of this repository's modules, and drive
the result through EDS's ordinary client API: the same calls Evolution makes.
The server on the other side is the in-repo mock, so no account anywhere is
involved.

This is M9 layer 1 of `docs/ROADMAP.md`. The GUI tier is a separate thing and
is not here.

## Running them

They are off by default; see "Why they are gated" below.

```console
$ cmake -S . -B build -DENABLE_FUNCTIONAL_TESTS=ON
$ cmake --build build
$ ctest --test-dir build -L functional --output-on-failure
```

`-L functional` selects exactly these; a plain `ctest` in a build configured
this way runs them alongside everything else, which is also fine — they take
under a second.

Nothing is installed. Nothing needs `sudo`. Nothing touches the Evolution data
of whoever runs them.

## What you need installed

- The EDS **runtime**, not just the development headers every other target
  here builds against: `evolution-source-registry`,
  `evolution-addressbook-factory` and `evolution-calendar-factory`. On Debian
  and Ubuntu that is the `evolution-data-server` package; the `-dev` packages
  do not carry the daemons.
- `dbus-run-session`, from `dbus-daemon`.

Configuring with `-DENABLE_FUNCTIONAL_TESTS=ON` without these is a configure
error naming the missing one. That is deliberate: see below.

## How one runs

`rust/crates/jmap-functional` is the harness. For each test it:

1. starts a mock JMAP server in-process, on an ephemeral port, and seeds it
   with what the test needs — one address book or one calendar flagged as the
   account default, or a few mailboxes with some mail in them;
2. builds a throwaway EDS installation in a directory under the crate's
   target tmpdir: a scratch `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and
   `XDG_CACHE_HOME`, a `.source` keyfile naming the mock's port, and a module
   directory holding the one module under test, named by
   `EDS_ADDRESS_BOOK_MODULES`, `EDS_CALENDAR_MODULES` or
   `EDS_CAMEL_PROVIDER_DIR`;
3. runs the client program on a **private session bus** from
   `dbus-run-session`, with that environment and nothing inherited. The
   daemons D-Bus activates are this test's daemons, started with this test's
   environment, and they die with the bus when the client exits;
4. holds both ends to what they should have said: what EDS gave the client,
   and what the backend asked the server for.

The client programs live in `tests/functional/`. They are C, and they are
ordinary libebook/libecal/libcamel consumers — that is the surface under
test, and no crate in this repository binds it (`eds-sys` carries what the
backends *implement*). Binding a second FFI surface only to call it from a
test would put a layer of our own making between EDS and the thing being
tested.

Each client prints `key=value` lines and exits non-zero the moment a call
fails. It holds no opinion about what is correct; the Rust side has all of
those.

### Why the clients pass "do not wait for connected"

Both call `e_*_client_connect_sync` with `(guint32) -1`, and then wait for
`ESource:connection-status` themselves, with a main loop, in
`tests/functional/connection-status.c`. That is not belt-and-braces; the
built-in wait cannot work in a program shaped like these:

`ESource` does not apply a connection-status change where it learns of it. It
queues an idle on the source's `GMainContext`, which is whatever was
thread-default when `ESourceRegistry` was constructed — in a synchronous
program, the default context on the main thread.
`e_client_wait_for_connected_sync` then blocks *that thread* on an `EFlag`
until `notify::connection-status` fires. The signal comes from the idle; the
idle needs the context iterated; the only thread that would iterate it is the
one blocked on the flag. So the wait always expires, whatever the backend did.

Evolution never meets this: it has a main loop and does the wait on a worker
thread. But it means a 30-second stall on opening a JMAP address book from a
small synchronous test program is a property of the *program*, not evidence
about the backend — which is exactly how it was read for a while. Running a
main loop is the whole fix, and it is on the client's side of the contract.

## Why they are gated behind an option

The shared CI image has the EDS development headers and neither daemon. A
test registered unconditionally would therefore either fail every CI run, or
— the tempting fix, and the worse one — be written to skip itself when the
runtime is missing, and report green on a machine where it never ran.

`ENABLE_FUNCTIONAL_TESTS` keeps "did not run" distinguishable from "passed".
With it off, the tests do not exist. With it on, a missing runtime is a
configure error. There is no arrangement in which they quietly pass.

The consequence, stated plainly: **CI does not run these today**, so a change
that breaks the EDS-facing behaviour they cover goes green until someone runs
them here. Closing that needs `evolution-data-server` and `dbus-daemon` in
the CI image (`Containerfile.ci`, rebuilt via `ci-image.yml`) and a
`workflow_dispatch`-gated job that configures with the option on — a
maintainer decision, because it grows the image every job pulls.

## What the address book test asserts

`rust/crates/jmap-functional/tests/address-book.rs`, against
`tests/functional/book-client.c`:

- the factory found and loaded `libebookbackendjmap.so`, and matched its
  factory to the keyfile's `BackendName=jmap` — a failure here is a client
  that cannot connect at all;
- **EDS saw the source reach `connected`.** `ESource:connection-status` is
  EDS's own verdict on the connect — what Evolution shows as a connected
  account, and what an EDS client that waits for a backend waits on. The meta
  backend sets it to `connected` only when the backend's `connect_sync`
  returned TRUE, and to `disconnected` when it did not;
- **the opened book is writable.** EDS derives this from what the backend
  said during its connect; a backend that connects happily and never claims
  the book is writable gives an address book Evolution greys out and whose
  every write comes back as "Permission denied". This assertion is why the
  test exists — that was a real bug, invisible to every unit test in the tree
  and to a build that compiled cleanly;
- a contact added through `e_book_client_add_contact_sync` reaches the
  server: the mock recorded a `ContactCard/set`, and the card in its store
  has the name, the email address and the address book it was given;
- the same contact reads back out of EDS with its name and email intact.

The read path is deliberately *not* asserted. `EBookMetaBackend` schedules its
refresh rather than running it, so whether a `ContactCard/query` has happened
by the time the test looks is a race; the write is synchronous. A test that
asserted it would be a flake waiting for a slow machine.

## What the calendar test asserts

`rust/crates/jmap-functional/tests/calendar.rs`, against
`tests/functional/cal-client.c`, is the same test one collection over, and it
was written because the two backends are mirrors of each other — which is
exactly the shape in which one of them carries a bug the other's tests would
have caught. It did: `jmap-backend-cal` had the address book's writable bug,
line for line, and this test is what found it.

- the factory found and loaded `libecalbackendjmap.so` and matched it to
  `BackendName=jmap`;
- **EDS saw the source reach `connected`**, which carries more here than it
  does for the address book: `e_cal_client_connect_sync` succeeds even when the
  backend's `connect_sync` failed — `ECalMetaBackend` opens the calendar and
  schedules the connect — so this is the one observation that tells a calendar
  the backend could not open from one it opened and forgot to claim writable.
  It is therefore asserted *before* the writable check, whose net covers both;
- **the opened calendar is writable**, for the reason above;
- an event added through `e_cal_client_create_object_sync` reaches the server:
  the mock recorded a `CalendarEvent/set`, and the event in its store has the
  summary, the start time and the calendar it was given;
- the same event reads back out of EDS with its summary intact.

The read path is left alone for the same reason as the address book's.

## What the mail test asserts

`rust/crates/jmap-functional/tests/mail.rs`, against
`tests/functional/mail-client.c`, and it is not a third mirror of the other
two — the loading story is different, and that difference is most of why it is
worth having.

An address book or calendar backend is dlopened by a factory daemon EDS ships,
found by being a file in a directory that daemon scans. A Camel provider is
dlopened by the **mail client's own process**, and only when something asks for
a protocol that a `.urls` file in Camel's provider directory claims. So the
client program here is not a consumer talking to a daemon that hosts the module
— it *is* the host. Nothing links the provider in; it is a file in a directory,
found the way Camel finds one, which is a mechanism `jmap-mail`'s own tests
cannot exercise because they link it.

- **`protocol=jmap` and the store connected.** Between them these say that the
  keyfile's `BackendName`, the one line in `libcameljmap.urls` and the string
  `camel_provider_module_init` registers all agree. They live in three files,
  and when they do not agree every later step fails with *No provider available
  for protocol* — a message about the connect that is really about a typo. The
  `.urls` file is staged from the source tree rather than written by the test,
  so this is the installed file being checked and not a copy of it;
- **the folder tree is the mock's three mailboxes**, from a single
  `Mailbox/get`;
- **the inbox is the mailbox with the JMAP `inbox` role**, which is what Camel
  asks the store for and what lets Evolution treat it as the account's inbox
  rather than as a folder that happens to be called one;
- **the summaries are the seeded messages** — `Email/query` and `Email/get`,
  through Camel's own summary machinery;
- **every message body downloads**, which is a different request again: a blob
  download is a plain HTTP GET rather than a method call, and a provider that
  lists mail it cannot open is a common enough failure to be worth fetching all
  of them.

Every list the client reports is sorted before it is printed. The order Camel
hands folders or message uids over is the provider's business, and a test that
compared it as given would be asserting an order nobody promised — which it did
once, before this was written down.

Sending is not covered. A transport is a second `CamelService` configured from
a second source, and the thing most worth testing about it is that it kept a
server of its own; that is the next leg rather than this one.

## Debugging a failure

The failure message carries the client's whole stdout and stderr, including
D-Bus activation lines, which is usually enough to tell "the module was never
loaded" from "the module was loaded and misbehaved".

The scratch tree is left behind — `rust/target/tmp/<test name>/` — and
wiped at the *start* of the next run, not the end, so it can be looked at
afterwards. It holds the keyfile the test wrote, the module it staged, and
the meta backend's cache database as EDS left it.

For more from the daemons, add `G_MESSAGES_DEBUG=all` to the session's
environment in `rust/crates/jmap-functional/src/lib.rs` and run the test with
`--nocapture`.

## Related

- `docs/manual-test-book-backend.md`, `docs/manual-test-cal-backend.md` and
  `docs/manual-test-mail-provider.md` — the same paths by hand, in a real
  Evolution, with a real Contacts, Calendar or mail view to look at. These
  tests do not replace them: they check the machinery, those check that a
  person can use it.
