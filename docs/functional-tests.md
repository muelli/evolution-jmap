# Functional tests: real EDS daemons against the mock

Most tests in this repository stop at the edge of Evolution Data Server: they
call a vfunc body directly or check a mapping against a fixture. They do not
show when EDS calls those vfuncs, or what it does with the results.

The functional tests cover that layer. They start a real
`evolution-source-registry` and a real host for the module under test
(`evolution-addressbook-factory`, `evolution-calendar-factory`, or, for the
Camel provider, the client program itself). They load a real build of the
module and drive it through EDS's ordinary client API, the same calls that
Evolution makes. The server is the in-repo mock, so no account is needed.
The GUI tests are separate (`docs/gui-smoke-test.md`).

## Running them

They are off by default (see "Why they are gated behind an option").

```console
$ cmake -S . -B build -G Ninja -DENABLE_FUNCTIONAL_TESTS=ON
$ cmake --build build
$ ctest --test-dir build -L functional --output-on-failure
```

`ci/functional.sh` runs exactly these commands. Nothing is installed, nothing
needs `sudo`, and your own Evolution data is not touched.

## What you need installed

- The EDS **runtime**, not only the development headers:
  `evolution-source-registry`, `evolution-addressbook-factory` and
  `evolution-calendar-factory` (Debian and Ubuntu: `evolution-data-server`).
- `dbus-run-session` (`dbus-daemon`).
- `gnome-keyring-daemon` (`gnome-keyring`). Each test session gets its own
  unlocked login keyring, so EDS's credential lookups reach a real
  `org.freedesktop.secrets` provider. See
  `rust/crates/jmap-functional/tests/secret-store.rs`.

`ci/install-deps-functional.sh` installs all three. With
`-DENABLE_FUNCTIONAL_TESTS=ON` and one of them missing, configuration fails
with a message that names it.

## How one runs

`rust/crates/jmap-functional` is the harness. For each test it:

1. starts a mock JMAP server in-process on an ephemeral port and seeds it
   with what the test needs;
2. builds a throwaway EDS setup under the crate's target tmpdir: scratch
   `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME`, the `.source`
   keyfiles that point at the mock, and a module directory with only the
   module under test, named by `EDS_ADDRESS_BOOK_MODULES`,
   `EDS_CALENDAR_MODULES`, `EDS_CAMEL_PROVIDER_DIR` or
   `EDS_REGISTRY_MODULES`;
3. runs a client program on a **private session bus** from
   `dbus-run-session`, with only that environment. The daemons that D-Bus
   activates belong to this test and stop with the bus;
4. checks both ends: what EDS gave the client, and what the backend asked
   the server for.

The client programs are in `tests/functional/`. They are C programs that use
libebook, libecal and libcamel, because that client API is the surface under
test and no crate here binds it. Each client prints `key=value` lines and
exits non-zero when a call fails. The Rust side makes all the assertions.

The tests do not work with a plain `cargo test`: CTest passes them the paths
of the client programs and modules in environment variables.

### Why the clients pass "do not wait for connected"

The clients call `e_*_client_connect_sync` with `(guint32) -1` and then wait
for `ESource:connection-status` with a main loop
(`tests/functional/connection-status.c`). The built-in wait cannot work in a
synchronous program: `ESource` applies a status change in an idle on the
`GMainContext` that was thread-default when `ESourceRegistry` was created,
which is the main thread's. `e_client_wait_for_connected_sync` blocks that
same thread, so the idle never runs and the wait always times out.
Evolution has a main loop, so it never sees this.

## Why they are gated behind an option

The ordinary CI jobs (`checks`, `build`, `reproducible`) install only the EDS
development headers (`ci/install-deps.sh`). A test that is always
registered would fail there, or would skip itself and report green without
having run. With `ENABLE_FUNCTIONAL_TESTS` off the tests do not exist; with
it on, a missing runtime is a configure error. So "did not run" never looks
like "passed".

The `functional` job in `.github/workflows/ci.yml` runs them on
`workflow_dispatch` or on a pull request with the label
`run-functional-tests`. It installs the runtime with
`ci/install-deps-functional.sh` and then runs `ci/functional.sh`. The job is
not in `.gitlab-ci.yml`.

## The tests

Each CTest test runs one file in `rust/crates/jmap-functional/tests/`.
`cmake/Functional.cmake` lists them with their clients, and each file's
header comment explains what it asserts and why.

| CTest name | Checks |
| --- | --- |
| `functional-book` | The factory loads the book backend; the source reaches `connected`; the book is writable; a contact added through EDS reaches the server and reads back. Further legs edit server-seeded cards and check that data a vCard cannot carry (name components, photos, calendaring addresses, related persons, notes, URI-only handles) survives, and that removing a contact reaches the server. |
| `functional-book-changes` | A second connect on the same cache reaches `get_changes_sync`. |
| `functional-cal` | The calendar counterpart of `functional-book`: writable calendar, events, recurrences and exceptions, an edit of a server-seeded event, and time zones that only the server or only the client can name. |
| `functional-cal-changes` | A second connect on the same cache reaches `get_changes_sync`. |
| `functional-cal-color` | Changing a calendar's colour in EDS reaches the server. |
| `functional-cal-free-busy` | `get_free_busy_sync` through a running backend. |
| `functional-cal-scheduling` | Who sends a meeting's invitations to its guests. |
| `functional-mail` | Camel loads the provider through its `.urls` file, opens the store and lists the Inbox. |
| `functional-transport` | Camel sends through the transport that the identity names; a transport without an `[Authentication]` group cannot send. |
| `functional-config-lookup` | The "Look Up Account Details" worker (`JmapConfigLookup`), including OAuth 2.0 discovery, against a real `EConfigLookup`. |
| `functional-collection` | `evolution-source-registry` loads `module-jmap-backend.so` and creates the child sources of a collection account. |
| `functional-collection-edit-host` | Editing a live account's host makes it authenticate again. |
| `functional-collection-create`, `functional-collection-create-calendar` | Evolution's "New Address Book"/"New Calendar" and "Delete" (`e_source_remote_create_sync`, `e_source_remote_delete_sync`) reach the server. |
| `functional-book-stale-token`, `functional-cal-stale-token`, `functional-mail-stale-token` | A stale OAuth 2.0 access token is refreshed and the operation retried silently; a refresh that does not help fails once instead of looping. |
| `functional-redirect-credentials-withheld` | A redirect to another origin that drops the credentials fails without a new consent prompt. |
| `functional-stale-source-uid`, `functional-oauth2-stale-proxy`, `functional-oauth2-token-collision` | Registry and OAuth 2.0 edge cases that a user reported. |
| `functional-secret-store`, `functional-secret-store-lock` | The test environment's own keyring. |

The `*-live-stalwart` tests are listed in the next section.

## Debugging a failure

The failure message contains the client's whole stdout and stderr, with the
D-Bus activation lines. That usually shows whether the module was never
loaded or was loaded and misbehaved.

The scratch tree stays in `rust/target/tmp/<test name>/` until the start of
the next run. It holds the keyfiles, the staged module and the meta backend's
cache database.

For more output from the daemons, add `G_MESSAGES_DEBUG=all` to the session
environment in `rust/crates/jmap-functional/src/lib.rs` and run the test with
`--nocapture`.

## Against a real server instead of the mock

The `functional-*-live-stalwart` tests (`live-stalwart-*.rs`) drive real EDS
against a real server. They are `#[ignore]`, so in an ordinary run CTest
reports them as ignored. To run one, take its command and environment from
`ctest --test-dir build -R <name> --show-only=json-v1`, set the
live-server variables from `docs/manual-test-live-server.md` (steps 2, 3 and
3a), and run that `cargo test` command with `-- --ignored`.
`live-stalwart-config-lookup.rs` needs `STALWART_URL` instead.

`.source` keyfiles allow plain HTTP only to loopback, so
`spawn_loopback_proxy` (`rust/crates/jmap-functional/src/lib.rs`) forwards a
loopback port to the real server. The client seeds the server password into
the session keyring (`JMAP_FUNCTIONAL_STORE_PASSWORD`).

## Related

`docs/manual-test-book-backend.md`, `docs/manual-test-cal-backend.md` and
`docs/manual-test-mail-provider.md` test the same paths by hand in a real
Evolution. These tests check the machinery; the recipes check that a person
can use it.
