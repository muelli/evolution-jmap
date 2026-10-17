# Manual test: the JMAP address book backend

The test suite covers every layer of the address book backend except
`evolution-addressbook-factory` loading the module and opening an address
book from a `.source` file. That step needs a running registry, so it cannot
be a `cargo test`. This recipe covers it.

It uses `jmap-mockd`, not a real server, so you need no account.

## What automated tests already cover

`rust/crates/jmap-backend-book/tests/recipe.rs` loads the keyfile below with
`e_server_side_source_new` (the call `evolution-source-registry` makes for
each file in its sources directory). It checks the origin, the anonymous
connection and that `BackendName` matches the factory name. It also checks
that the keyfile in this document is identical to
`docs/examples/jmap-mock.source`.

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

It seeds one account with an address book named "Personal", which is the
account default. Leave it running in its own terminal. It logs each request
that the backend makes.

Do not pass `--basic` or `--bearer` for the first run. The account below has
no user, so the backend connects anonymously and there is no password prompt.

## 2. Build and install the backend

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component book-backend
```

This installs `libebookbackendjmap.so` into
`pkg-config --variable=backenddir libedata-book-1.2` (on Debian and Ubuntu,
`/usr/lib/evolution-data-server/addressbook-backends`).

To avoid a system directory, install into a scratch tree and point the
factory at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component book-backend
$ export EDS_ADDRESS_BOOK_MODULES=/tmp/jmap/usr/lib/evolution-data-server/addressbook-backends
```

`EDS_ADDRESS_BOOK_MODULES` replaces the backend directory, so the factory
then has only the JMAP backend. Set it in the environment that starts the
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
- `Method=none` allows plain HTTP. The backend allows that only for a
  loopback host. **The key is `Method`, not `Secure`.** EDS ignores
  `Secure=true`, which leaves the method at `none`, and the backend then
  refuses a remote host. Use `Method=tls`, or leave out the `[Security]`
  group, for TLS.
- There is no `User=`. With a user, the backend asks EDS for a password
  before it connects. To test that path, start the mock with
  `--basic alice:secret`, add `User=alice` and `Method=plain/password` to
  `[Authentication]`, and let Evolution prompt you.

Without `[Resource] Identity=`, the backend uses the account's default
address book. To name one, `curl http://127.0.0.1:8080/jmap/session`, take an
address book id, and add `Identity=<id>` in a `[Resource]` group.

The source has no `Parent=`, so it appears on its own.
`docs/manual-test-collection-backend.md` covers the account form.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-addressbook-factory
$ pkill -f evolution-source-registry
$ evolution
```

Both daemons are D-Bus activated and start again on demand. In Evolution's
Contacts view, "JMAP mock" is in the list. It is empty on a fresh mock. Add a
contact in Evolution and watch the mock log `ContactCard/set`.

To watch the backend instead, start the factory by hand before Evolution:

```console
$ G_MESSAGES_DEBUG=all evolution-addressbook-factory -r -w
```

## What "it worked" means

- `evolution-addressbook-factory` loaded `libebookbackendjmap.so` (see its
  debug output).
- Opening the book made the mock serve `/jmap/session` and `ContactCard`
  requests.
- A contact added in Evolution made the mock serve `ContactCard/set`, and the
  contact is still there after you close and open the book again.

Anything less is a bug in this repository. Report it.
