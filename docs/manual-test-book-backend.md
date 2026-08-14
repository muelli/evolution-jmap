# Manual test: the JMAP address book backend

The test suite exercises every layer of the address book backend except the
one that matters to a user: `evolution-addressbook-factory` loading the
module, picking the factory out of it, and opening an address book from a
`.source` file. Constructing an `EBookMetaBackend` needs a running registry,
so that last step cannot be a `cargo test` — it is this recipe.

It talks to `jmap-mockd`, not to a real server, so it can be run on a laptop
with nothing installed and no account anywhere.

## What is already checked without you

`rust/crates/jmap-backend-book/tests/recipe.rs` loads the keyfile below
through `e_server_side_source_new` — the same call `evolution-source-registry`
makes for every file in its sources directory — and asserts that it means what
this document says it means: the origin, the anonymous connection, and a
`BackendName` that matches the name the factory registers. So the keyfile
cannot rot silently; the steps around it are what you are testing.

## Prerequisites

- Evolution Data Server 3.52 or newer, *installed*: the development headers
  are enough to build, but this recipe runs `evolution-source-registry` and
  `evolution-addressbook-factory`, which come with the `evolution-data-server`
  package (Debian/Ubuntu) or `evolution-data-server` (Fedora).
- A session bus and a running `gnome-keyring`/`libsecret` provider — the
  anonymous account below never asks for a password, but the registry expects
  a session it can talk on.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

It seeds one account with an address book named "Personal", flagged as the
account default. Leave it running in its own terminal; every request it serves
is one the backend made.

Do not pass `--basic` or `--bearer` for the first run. The account below names
no user, so the backend connects anonymously and no password prompt gets in
the way of the thing being tested.

## 2. Build and install the backend

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component book-backend
```

That installs `libebookbackendjmap.so` into the directory
`pkg-config --variable=backenddir libedata-book-1.2` reports — on Debian and
Ubuntu, `/usr/lib/evolution-data-server/addressbook-backends`.

If you would rather not install into a system directory, install into a
scratch tree and point the factory at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component book-backend
$ export EDS_ADDRESS_BOOK_MODULES=/tmp/jmap/usr/lib/evolution-data-server/addressbook-backends
```

`EDS_ADDRESS_BOOK_MODULES` *replaces* the backend directory rather than adding
to it, so a factory started with it set has the JMAP backend and no other —
fine for this test, and the reason the variable is not a way to run Evolution
day to day. It has to be set in the environment the factory is started from,
which for a D-Bus activated one means restarting the daemons (step 4).

## 3. Write the account

Copy `docs/examples/jmap-mock.source` to
`~/.config/evolution/sources/jmap-mock.source` — the file name matters, it
becomes the source UID:

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

Three of those lines are load-bearing:

- `BackendName=jmap` is what makes the address book factory pick this backend
  and not another. It is matched against the factory's own name; nothing else
  in the file selects a backend.
- `Method=none` is what allows plain HTTP. The backend refuses it for any host
  that is not loopback, so this line only ever works for a local mock or a
  development server on the same machine. **The key is `Method`, not
  `Secure`**: `ESourceSecurity:secure` is a boolean *over* the `Method`
  string, and a keyfile that says `Secure=true` is not read as an error, it is
  read as no method at all — which is `none`. Against a real server that is an
  account that refuses to connect at all, complaining about TLS. `Method=tls`
  is the other value, and leaving the whole `[Security]` group out means TLS
  here too.
- No `User=`: with one, the backend asks EDS for a password and refuses to
  connect until it gets one. To test *that* path instead, start the mock with
  `--basic alice:secret`, add `User=alice@example.com` and
  `Method=plain/password` to `[Authentication]`, and let Evolution prompt you.

There is no `[Resource] Identity=`, so the backend asks the server for the
account's default address book. Naming one is the other half of the recipe:
`curl` the mock's `/jmap/session`, take an id out of the address book list,
and add `Identity=<id>` under a `[Resource]` group.

Until M6's collection backend exists there is no account to hang this off, so
the source has no `Parent=` and appears on its own.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-addressbook-factory
$ pkill -f evolution-source-registry
$ evolution
```

Both are D-Bus activated and come back on demand. Then, in Evolution's
Contacts view, "JMAP mock" should be in the list and open to show the contacts
the mock has (an empty book on a fresh mock — add one from Evolution and watch
the mock log the `ContactCard/set`).

To watch the backend instead of using the UI:

```console
$ G_MESSAGES_DEBUG=all evolution-addressbook-factory -r -w
```

started by hand *before* Evolution, with the mock's terminal next to it: the
factory logs each module it loads, and every request the backend makes shows
up in the mock's output.

## What "it worked" means

- `evolution-addressbook-factory` loaded `libebookbackendjmap.so` — it appears
  in the factory's debug output, and a `BackendName` nothing claims produces a
  visible "No backend factory for ..." error instead.
- Opening the book made the mock serve `/jmap/session` and a
  `ContactCard/query` + `ContactCard/get` pair.
- Adding a contact in Evolution made it serve `ContactCard/set`, and the
  contact survives closing and reopening the book — which also means it went
  through the meta backend's cache and came back.

Anything short of that is a bug in this repository, not in the recipe;
`docs/NIGHT-LOG.md` is where the ones found this way get written down.
