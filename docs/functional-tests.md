# Functional tests: real EDS daemons against the mock

Most tests in this repository stop at the edge of Evolution Data Server. They
call a vfunc body directly, or they check a mapping against a fixture. They do
not show when EDS calls those vfuncs. They do not show what EDS does with the
results.

The functional tests cover that layer. They start a real
`evolution-source-registry` and a real host for the module under test. The
host is `evolution-addressbook-factory` or `evolution-calendar-factory`. For
the Camel provider, the host is the client program itself. The tests load a
real build of the module. They drive it through the ordinary client API of
EDS, with the same calls that Evolution makes. The server is the mock server
in the repository. No account is necessary. The GUI tests are separate
(`docs/gui-smoke-test.md`).

## Running them

The tests are off by default (see "Why they are gated behind an option").

```console
$ cmake -S . -B build -G Ninja -DENABLE_FUNCTIONAL_TESTS=ON
$ cmake --build build
$ ctest --test-dir build -L functional --output-on-failure
```

`ci/functional.sh` runs exactly these commands. The commands do not install
anything. They do not need `sudo`. They do not touch your own Evolution data.

## What you need installed

- The EDS **runtime**, not only the development headers:
  `evolution-source-registry`, `evolution-addressbook-factory` and
  `evolution-calendar-factory` (Debian and Ubuntu: `evolution-data-server`).
- `dbus-run-session` (`dbus-daemon`).
- `gnome-keyring-daemon` (`gnome-keyring`). Each test session gets its own
  unlocked login keyring. The credential lookups of EDS therefore reach a real
  `org.freedesktop.secrets` provider. See
  `rust/crates/jmap-functional/tests/secret-store.rs`.

`ci/install-deps-functional.sh` installs all three. If you use
`-DENABLE_FUNCTIONAL_TESTS=ON` and one of them is missing, the configuration
fails. The error message names the missing item.

## How one test runs

`rust/crates/jmap-functional` is the harness. For each test it does these
steps:

1. It starts a mock JMAP server in the same process on an ephemeral port. It
   seeds the server with the data that the test needs.
2. It builds a temporary EDS setup under the tmpdir of the crate target. The
   setup contains these items:
   - scratch `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME`
   - the `.source` keyfiles that point at the mock server
   - a module directory that contains only the module under test. The
     variable `EDS_ADDRESS_BOOK_MODULES`, `EDS_CALENDAR_MODULES`,
     `EDS_CAMEL_PROVIDER_DIR` or `EDS_REGISTRY_MODULES` names it.
3. It runs a client program on a **private session bus** from
   `dbus-run-session`, with only that environment. The daemons that D-Bus
   activates belong to this test. They stop with the bus.
4. It checks both ends: what EDS gave to the client, and what the backend
   asked the server for.

The client programs are in `tests/functional/`. They are C programs that use
libebook, libecal and libcamel. The client API is the surface under test, and
no crate here binds it. Each client prints `key=value` lines. It exits with a
non-zero status if a call fails. The Rust side makes all the assertions.

The tests do not work with a plain `cargo test`. CTest gives them the paths of
the client programs and modules in environment variables.

### Why the clients pass "do not wait for connected"

The clients call `e_*_client_connect_sync` with `(guint32) -1`. Then they wait
for `ESource:connection-status` with a main loop
(`tests/functional/connection-status.c`). The built-in wait cannot work in a
synchronous program. `ESource` applies a status change in an idle on the
`GMainContext` that was thread-default when `ESourceRegistry` was created.
This is the `GMainContext` of the main thread.
`e_client_wait_for_connected_sync` blocks the same thread. The idle therefore
never runs, and the wait always times out. Evolution has a main loop. It never
sees this problem.

## Why they are gated behind an option

The ordinary CI jobs (`checks`, `build`, `reproducible`) install only the EDS
development headers (`ci/install-deps.sh`). If the build always registered a
test, one of two things would happen in these jobs. The test would fail, or
it would skip itself and report green without a run. If
`ENABLE_FUNCTIONAL_TESTS` is off, the tests do not exist. If it is on, a
missing runtime is a configure error. "Did not run" therefore never looks like
"passed".

The `functional` job in `.github/workflows/ci.yml` runs the tests on
`workflow_dispatch` or on a pull request with the label
`run-functional-tests`. It installs the runtime with
`ci/install-deps-functional.sh`. Then it runs `ci/functional.sh`. The job is
not in `.gitlab-ci.yml`.

## The tests

Each CTest test runs one file in `rust/crates/jmap-functional/tests/`.
`cmake/Functional.cmake` lists the tests with their clients. The header
comment of each file explains what the file asserts and why.

| CTest name | Checks |
| --- | --- |
| `functional-book` | The factory loads the book backend. The source reaches `connected`. The book is writable. A contact that is added through EDS reaches the server and reads back. More legs edit server-seeded cards. They check that data which a vCard cannot carry survives (name components, photos, calendaring addresses, related persons, notes, URI-only handles). They also check that the removal of a contact reaches the server. |
| `functional-book-changes` | A second connect on the same cache reaches `get_changes_sync`. |
| `functional-cal` | The calendar counterpart of `functional-book`: writable calendar, events, recurrences and exceptions, an edit of a server-seeded event, and time zones that only the server or only the client can name. |
| `functional-cal-changes` | A second connect on the same cache reaches `get_changes_sync`. |
| `functional-cal-color` | A change of the colour of a calendar in EDS reaches the server. |
| `functional-cal-free-busy` | `get_free_busy_sync` through a running backend. |
| `functional-cal-scheduling` | Which party sends the invitations of a meeting to the guests. |
| `functional-mail` | Camel loads the provider through its `.urls` file, opens the store and lists the Inbox. |
| `functional-transport` | Camel sends through the transport that the identity names. A transport without an `[Authentication]` group cannot send. |
| `functional-config-lookup` | The "Look Up Account Details" worker (`JmapConfigLookup`), including OAuth 2.0 discovery, against a real `EConfigLookup`. |
| `functional-collection` | `evolution-source-registry` loads `module-jmap-backend.so` and creates the child sources of a collection account. |
| `functional-collection-edit-host` | An edit of the host of a live account makes the account authenticate again. |
| `functional-collection-create`, `functional-collection-create-calendar` | "New Address Book", "New Calendar" and "Delete" in Evolution (`e_source_remote_create_sync`, `e_source_remote_delete_sync`) reach the server. |
| `functional-book-stale-token`, `functional-cal-stale-token`, `functional-mail-stale-token` | The backend refreshes a stale OAuth 2.0 access token and retries the operation silently. A refresh that does not help fails once. It does not loop. |
| `functional-redirect-credentials-withheld` | A redirect to another origin that drops the credentials fails without a new consent prompt. |
| `functional-stale-source-uid`, `functional-oauth2-stale-proxy`, `functional-oauth2-token-collision` | Registry and OAuth 2.0 edge cases that a user reported. |
| `functional-secret-store`, `functional-secret-store-lock` | The own keyring of the test environment. |

The next section lists the `*-live-stalwart` tests.

## Debugging a failure

The failure message contains the whole stdout and stderr of the client, with
the D-Bus activation lines. This usually shows one of two things: the module
was never loaded, or the module was loaded and failed.

The scratch tree stays in `rust/target/tmp/<test name>/` until the next run
starts. It holds the keyfiles, the staged module and the cache database of the
meta backend.

To get more output from the daemons, do these steps:

1. Add `G_MESSAGES_DEBUG=all` to the session environment in
   `rust/crates/jmap-functional/src/lib.rs`.
2. Run the test with `--nocapture`.

## Against a real server instead of the mock

The `functional-*-live-stalwart` tests (`live-stalwart-*.rs`) drive real EDS
against a real server. They are `#[ignore]`. In an ordinary run, CTest
therefore reports them as ignored. To run one of them, do these steps:

1. Take its command and environment from
   `ctest --test-dir build -R <name> --show-only=json-v1`.
2. Set the live-server variables from `docs/manual-test-live-server.md`
   (steps 2, 3 and 3a).
3. Run that `cargo test` command with `-- --ignored`.

`live-stalwart-config-lookup.rs` needs `STALWART_URL` instead.

`.source` keyfiles allow plain HTTP only to a loopback address.
`spawn_loopback_proxy` (`rust/crates/jmap-functional/src/lib.rs`) therefore
forwards a loopback port to the real server. The client seeds the server
password into the session keyring (`JMAP_FUNCTIONAL_STORE_PASSWORD`).

## Related

`docs/manual-test-book-backend.md`, `docs/manual-test-cal-backend.md` and
`docs/manual-test-mail-provider.md` test the same paths by hand in a real
Evolution. The functional tests check the machinery. The recipes check that a
person can use it.
