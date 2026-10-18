# Manual test: the JMAP address book backend

The test suite covers every layer of the address book backend except one step.
This step is the loading of the module by `evolution-addressbook-factory` and
the opening of an address book from a `.source` file. The step needs a running
registry. It cannot be a `cargo test`. This recipe covers it.

The recipe uses `jmap-mockd`, not a real server. You do not need an account.

## What automated tests already cover

`rust/crates/jmap-backend-book/tests/recipe.rs` loads the keyfile below with
`e_server_side_source_new`. `evolution-source-registry` makes the same call for
each file in its sources directory. The test checks these items:

- the origin
- the anonymous connection
- that `BackendName` matches the factory name
- that the keyfile in this document is identical to
  `docs/examples/jmap-mock.source`

## Prerequisites

- Evolution Data Server 3.52 or newer, installed (the
  `evolution-data-server` package). This recipe runs
  `evolution-source-registry` and `evolution-addressbook-factory`.
- A D-Bus session bus.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

The mock server seeds one account with an address book named "Personal". This
is the default of the account. Leave the server running in its own terminal.
It logs each request that the backend makes.

Do not use `--basic` or `--bearer` for the first run. The account below has no
user. The backend therefore connects anonymously, and no password prompt
appears.

## 2. Build and install the backend

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component book-backend
```

This installs `libebookbackendjmap.so` into
`pkg-config --variable=backenddir libedata-book-1.2` (on Debian and Ubuntu,
`/usr/lib/evolution-data-server/addressbook-backends`).

To avoid a system directory, install into a scratch tree. Then point the
factory at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component book-backend
$ export EDS_ADDRESS_BOOK_MODULES=/tmp/jmap/usr/lib/evolution-data-server/addressbook-backends
```

`EDS_ADDRESS_BOOK_MODULES` replaces the backend directory. The factory then
has only the JMAP backend. Set the variable in the environment that starts the
factory (step 4).

## 3. Write the account

Copy `docs/examples/jmap-mock.source` to
`~/.config/evolution/sources/jmap-mock.source`. The file name becomes the
source UID.

```ini
[Data Source]
DisplayName=JMAP mock
Enabled=true

[Address Book]
BackendName=jmap

[Authentication]
Host=127.0.0.1
Port=8080

[Security]
Method=none
```

Notes on the keys:

- `BackendName=jmap` makes the address book factory use this backend.
- `Method=none` allows plain HTTP. The backend allows plain HTTP only for a
  loopback address. **The key is `Method`, not `Secure`.** EDS ignores
  `Secure=true`. The method then stays `none`, and the backend refuses a
  remote server. For TLS, use `Method=tls`, or omit the `[Security]` group.
- There is no `User=`. If a user is set, the backend asks EDS for a password
  before it connects. To test that path, do these steps:
  1. Start the mock server with `--basic alice:secret`.
  2. Add `User=alice` and `Method=plain/password` to `[Authentication]`.
  3. Let Evolution prompt you.

If `[Resource] Identity=` is not set, the backend uses the default address
book of the account. To name an address book, do these steps:

1. Run `curl http://127.0.0.1:8080/jmap/session`.
2. Take an address book id.
3. Add `Identity=<id>` in a `[Resource]` group.

The source has no `Parent=`. It therefore appears on its own.
`docs/manual-test-collection-backend.md` covers the account form.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-addressbook-factory
$ pkill -f evolution-source-registry
$ evolution
```

D-Bus activates both daemons. They start again on demand. In the Contacts view
of Evolution, "JMAP mock" is in the list. On a fresh mock server, it is empty.
Add a contact in Evolution. Watch the mock server log `ContactCard/set`.

To watch the backend instead, start the factory by hand before Evolution:

```console
$ G_MESSAGES_DEBUG=all evolution-addressbook-factory -r -w
```

## What "it worked" means

- `evolution-addressbook-factory` loaded `libebookbackendjmap.so` (see its
  debug output).
- When you opened the book, the mock server served `/jmap/session` and
  `ContactCard` requests.
- When you added a contact in Evolution, the mock server served
  `ContactCard/set`. The contact is still there after you close and open the
  book again.

Any other result is a bug in this repository. Report it.
