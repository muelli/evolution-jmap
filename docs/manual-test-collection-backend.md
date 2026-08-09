# Manual test: the JMAP collection backend

The test suite exercises every layer of the collection backend except the one
that matters to a user: `evolution-source-registry` loading the module, picking
the factory out of it, and turning one account `.source` file into the address
books and calendars that appear in Evolution's sidebar. Constructing an
`ECollectionBackend` needs a running registry — `new_backend` hands it the
server itself — so that last step cannot be a `cargo test`; it is this recipe.

It talks to `jmap-mockd`, not to a real server, so it can be run on a laptop
with nothing installed and no account anywhere.

This is the recipe the other two hang off. `docs/manual-test-book-backend.md`
and `docs/manual-test-cal-backend.md` each write a standalone source with no
`Parent=`, because until this backend existed there was no account to hang one
off; here the children are written *by* the backend, and what you are checking
is that they appear at all.

## What is already checked without you

`rust/crates/jmap-backend-collection/tests/recipe.rs` loads the keyfile below
through `e_server_side_source_new` — the same call the registry makes for every
file in its sources directory — and asserts that it means what this document
says it means: the origin, the anonymous connection, which parts are switched
on, and a `BackendName` that matches the name the factory registers.
`tests/factory.rs` asserts the other half, that the module registers a factory
answering to that name and building this crate's backend. So neither the
keyfile nor the name can rot silently; the steps around them are what you are
testing.

## Prerequisites

- Evolution Data Server 3.52 or newer, *installed*: the development headers are
  enough to build, but this recipe runs `evolution-source-registry`, which comes
  with the `evolution-data-server` package.
- The address book and calendar backends installed too — `--component
  book-backend` and `--component cal-backend`, per the other two recipes.
  Without them the children this backend writes are sources no factory can
  open: they appear in the sidebar and fail when clicked, which is a different
  bug from the one being tested here.
- A session bus. The account below names no user, so nothing asks libsecret for
  a password, but the registry expects a session it can talk on.

## 1. Start the mock server

```console
$ cargo run -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

It seeds one account with an address book named "Personal" and a calendar of
the same name, each flagged as its kind's default, plus three mailboxes this
recipe deliberately ignores. Leave it running in its own terminal;
every request it serves is one the backend made.

Do not pass `--basic` or `--bearer` for the first run — see the account below.

## 2. Build and install the module

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component collection-backend
```

That installs `module-jmap-backend.so` into the directory
`pkg-config --variable=moduledir libebackend-1.2` reports — on Debian and
Ubuntu, `/usr/lib/evolution-data-server/registry-modules`.

If you would rather not install into a system directory, install into a scratch
tree and point the registry at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component collection-backend
$ export EDS_REGISTRY_MODULES=/tmp/jmap/usr/lib/evolution-data-server/registry-modules
```

`EDS_REGISTRY_MODULES` *replaces* the module directory rather than adding to
it, and the registry's own modules live in it — the cache reaper, the WebDAV
and Google collection backends, the OAuth2 support. A registry started with the
variable set has the JMAP module and none of those, which is fine for this test
and the reason it is not a way to run Evolution day to day. It has to be set in
the environment the registry is started from, which for a D-Bus activated one
means starting it by hand (step 4).

## 3. Write the account

Copy `docs/examples/jmap-mock-collection.source` to
`~/.config/evolution/sources/jmap-mock-collection.source` — the file name
matters, it becomes the source UID, and it is what the children will name as
their `Parent=`:

```ini
[Data Source]
DisplayName=JMAP mock account
Enabled=true

[Collection]
BackendName=jmap
ContactsEnabled=true
CalendarEnabled=true
MailEnabled=false

[Authentication]
Host=127.0.0.1
Port=8080

[Security]
Method=none
```

What each load-bearing line does:

- `[Collection]` is what makes this an account rather than a single address
  book. Its presence is what `e_source_registry_server_ref_backend_factory`
  looks for before it looks at anything else; a file without it is never
  offered to a collection factory at all.
- `BackendName=jmap` selects this backend. The registry files each collection
  factory under `"<factory_name>:Collection"` and looks up the key built from
  this string, so a typo here is not an error — it is an account that sits in
  the sidebar with no children and nothing in any log.
- `ContactsEnabled` and `CalendarEnabled` are the two parts this backend fans
  out to. Switching one off is worth trying as a second run: the children of
  that kind are *removed*, not merely hidden, which is the populate's other
  half.
- `MailEnabled=false` because nothing yet *creates* an account's mail sources.
  M5's Camel provider exists and works from a hand-written mail account, and the
  factory's `prepare_mail` now names that provider on the mail account and
  transport it is handed — but the three sources (account, identity, transport)
  are the setup UI's to create, as they are in every EDS backend, and that is
  M7. Until then an account claiming mail would claim a part nothing serves.
- `Method=none` is what allows plain HTTP, and it is refused for any host that
  is not loopback. **The key is `Method`, not `Secure`**: `ESourceSecurity:secure`
  is a boolean *over* the `Method` string, so a keyfile saying `Secure=true`
  sets nothing EDS reads and comes back as no method at all — which is `none`.
  Against a real server that is an account that refuses to connect, blaming
  TLS.
- No `User=`: with one, the backend asks EDS to resolve a password and does not
  contact anything until it has one. To test *that* path instead, start the
  mock with `--basic alice:secret`, add `User=alice@example.com` and
  `Method=plain/password` to `[Authentication]`, and let Evolution prompt you —
  that is the only way to exercise the credentials round trip, which is the
  half of `authenticate_sync` no test here can reach.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-source-registry
$ pkill -f evolution-addressbook-factory
$ pkill -f evolution-calendar-factory
$ SOURCE_REGISTRY_DEBUG=1 G_MESSAGES_DEBUG=all evolution-source-registry
```

started by hand, with the mock's terminal next to it, and Evolution launched
afterwards in a third. `SOURCE_REGISTRY_DEBUG=1` is EDS's own channel and the
one this backend writes its populate and fan-out lines to, including the
`e_collection_backend_new_child` pairing line per child.

In Evolution, "JMAP mock account" should appear as an account with an address
book under it in the Contacts view and a calendar in the Calendar view.

## What "it worked" means

- The registry logged loading `module-jmap-backend.so`, and the account got a
  backend: with `SOURCE_REGISTRY_DEBUG=1` there is a `populate:` line for it.
  An account whose `BackendName` nothing claims produces *no* such line, which
  is the failure this recipe is mostly here to make visible.
- The mock served `/jmap/session`, then `AddressBook/get` and `Calendar/get`,
  each with `ids: null` — the fan-out asking each account for its collections.
  No `Mailbox/get`, because `MailEnabled=false` gates it.
- New files appeared in `~/.config/evolution/sources/`, one per collection, each
  with `Parent=jmap-mock-collection` and a `[Address Book]` or `[Calendar]`
  group naming `BackendName=jmap`. These are written by the backend, not by you;
  they are the whole product of a fan-out.
- Opening one of those children works — which is the address book and calendar
  backends doing their job through the settings this backend wrote into the
  child. If a child opens but reaches the wrong server, the bug is in
  `child_source`, not here.
- Restarting the registry does *not* duplicate them: the second populate finds
  the cached children, claims them, and adds nothing.
- Setting `ContactsEnabled=false` and restarting removes the address book
  children, and their `.source` files with them.

Anything short of that is a bug in this repository, not in the recipe;
`docs/NIGHT-LOG.md` is where the ones found this way get written down.
