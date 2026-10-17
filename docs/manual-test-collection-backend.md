# Manual test: the JMAP collection backend

The test suite covers every layer of the collection backend except
`evolution-source-registry` loading the module and turning one account
`.source` file into the address books and calendars in Evolution's sidebar.
An `ECollectionBackend` needs a running registry, so this cannot be a
`cargo test`. This recipe covers it.

It uses `jmap-mockd`, not a real server, so you need no account.

The address book and calendar recipes
(`docs/manual-test-book-backend.md`, `docs/manual-test-cal-backend.md`) write
standalone sources with no `Parent=`. Here the backend writes the child
sources, and you check that they appear.

## What automated tests already cover

`rust/crates/jmap-backend-collection/tests/recipe.rs` loads the keyfiles in
this document with `e_server_side_source_new`. It checks the origin, the
anonymous connection, which parts are on, and that `BackendName` matches the
factory name. It also checks that each `ini` block in this document is
identical to its file in `docs/examples/`. `tests/factory.rs` checks that the
module registers a factory with that name.

## Prerequisites

- Evolution Data Server 3.52 or newer, installed (the
  `evolution-data-server` package). This recipe runs
  `evolution-source-registry`.
- The address book and calendar backends, also installed
  (`--component book-backend` and `--component cal-backend`). Without them,
  the child sources appear in the sidebar but fail to open.
- A D-Bus session bus.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

It seeds one account with an address book and a calendar, both named
"Personal" and both the default of their kind, plus three mailboxes (Inbox,
Sent, Drafts). Leave it running in its own terminal.

Do not pass `--basic` or `--bearer` for the first run.

## 2. Build and install the module

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component collection-backend
```

This installs `module-jmap-backend.so` into
`pkg-config --variable=moduledir libebackend-1.2` (on Debian and Ubuntu,
`/usr/lib/evolution-data-server/registry-modules`).

To avoid a system directory, install into a scratch tree and point the
registry at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component collection-backend
$ export EDS_REGISTRY_MODULES=/tmp/jmap/usr/lib/evolution-data-server/registry-modules
```

`EDS_REGISTRY_MODULES` replaces the module directory, so the registry then
lacks its own modules (cache reaper, WebDAV and Google collections, OAuth2).
That is fine for this test, not for daily use. Start the registry by hand in
that environment (step 4).

## 3. Write the account

Copy `docs/examples/jmap-mock-collection.source` to
`~/.config/evolution/sources/jmap-mock-collection.source`. The file name
becomes the source UID, and the children name it in `Parent=`.

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

Notes on the keys:

- `[Collection]` makes this an account, not a single address book.
- `BackendName=jmap` selects this backend. A typo here gives no error: the
  account sits in the sidebar with no children.
- `ContactsEnabled` and `CalendarEnabled` select what the backend creates.
  Try turning one off in a second run: the children of that kind are removed,
  not only hidden.
- `MailEnabled=false`, because the collection backend does not create mail
  sources. Evolution's account assistant creates them. The second run below
  writes them by hand.
- `Method=none` allows plain HTTP, and only for a loopback host. **The key is
  `Method`, not `Secure`**: EDS ignores `Secure=true`, which leaves the method
  at `none`, and the backend then refuses a remote host.
- There is no `User=`. To test password sign-in, start the mock with
  `--basic alice:secret`, add `User=alice` and `Method=plain/password` to
  `[Authentication]`, and let Evolution prompt you. No automated test covers
  that credentials round trip.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-source-registry
$ pkill -f evolution-addressbook-factory
$ pkill -f evolution-calendar-factory
$ SOURCE_REGISTRY_DEBUG=1 G_MESSAGES_DEBUG=all evolution-source-registry
```

Then start Evolution in another terminal. With `SOURCE_REGISTRY_DEBUG=1`,
EDS and this backend log each populate and each new child.

In Evolution, "JMAP mock account" appears with an address book in the
Contacts view and a calendar in the Calendar view.

## What "it worked" means

- The registry loaded `module-jmap-backend.so` and logged a populate for the
  account. If no factory claims the `BackendName`, there is no populate.
- The mock served `/jmap/session`, then `AddressBook/get` and `Calendar/get`.
  It served no `Mailbox/get`, because mail is off.
- New files appeared in `~/.config/evolution/sources/`, one per collection,
  each with `Parent=jmap-mock-collection` and an `[Address Book]` or
  `[Calendar]` group with `BackendName=jmap`. The backend wrote them.
- The children open. If a child opens but reaches the wrong server, look at
  `child_source`.
- Restarting the registry does not duplicate the children.
- Setting `ContactsEnabled=false` and restarting removes the address book
  children and their `.source` files.
- Changing `[Authentication] Host` (or `Port`, `User`, or `[Security]
  Method`) in the account's file and restarting the registry changes every
  child file too. `child_added` binds these properties from the account onto
  each child. A child with the old server keeps working only until that
  server goes away, so check this one.

Anything less is a bug in this repository. Report it.

## A second run: the mail sources

The collection backend does not create mail sources, but it gives them their
server. Evolution's assistant creates a transport with the backend name
`jmap` and nothing else, because the *Sending Email* page is hidden for a
provider that both stores and sends. The transport reaches a server only
after `child_added` binds the account's host, port, user and security method
onto it. This run writes the three mail sources by hand and checks that
binding without a GUI.

### Additional prerequisites

- The Camel provider: `sudo cmake --install build --component
  camel-provider`. Without it, Evolution ignores the mail sources.
- Remove the account from step 3
  (`rm ~/.config/evolution/sources/jmap-mock-collection.source`), so you do
  not have two accounts against the same mock.

### 1. The account

Copy `docs/examples/jmap-mock-mail-collection.source` to
`~/.config/evolution/sources/jmap-mock-mail-collection.source`:

```ini
[Data Source]
DisplayName=JMAP mock mail account
Enabled=true

[Collection]
BackendName=jmap
ContactsEnabled=true
CalendarEnabled=true
MailEnabled=true

[Authentication]
Host=127.0.0.1
Port=8080

[Security]
Method=none
```

This is the account from step 3 with `MailEnabled=true`. EDS binds each mail
source's `enabled` to the account's `mail-enabled`, so under an account with
mail off the three files below load disabled, do not appear in Evolution and
log nothing.

### 2. The three mail sources

Copy all three into the same directory. Their file names are their UIDs, and
the `Parent=`, `IdentityUid=` and `TransportUid=` lines refer to those UIDs.
If you rename one file, edit the others.

`docs/examples/jmap-mock-mail-account.source`, the receiving account:

```ini
[Data Source]
DisplayName=JMAP mock inbox
Enabled=true
Parent=jmap-mock-mail-collection

[Mail Account]
BackendName=jmap
IdentityUid=jmap-mock-mail-identity
```

`docs/examples/jmap-mock-mail-identity.source`, the sender:

```ini
[Data Source]
DisplayName=JMAP mock identity
Enabled=true
Parent=jmap-mock-mail-collection

[Mail Identity]
Name=JMAP mock user
Address=alice@example.com

[Mail Submission]
TransportUid=jmap-mock-mail-transport
```

`docs/examples/jmap-mock-mail-transport.source`, the sending service:

```ini
[Data Source]
DisplayName=JMAP mock transport
Enabled=true
Parent=jmap-mock-mail-collection

[Mail Transport]
BackendName=jmap
```

Notes on the keys:

- `Parent=jmap-mock-mail-collection` on all three makes them the account's
  children, so `child_added` sees them. A typo here gives orphan sources that
  appear nowhere and log nothing.
- `BackendName=jmap` on the account and the transport names the Camel
  provider. The backend binds only children with this backend name. A
  transport of another provider under this account keeps its own server and
  password.
- No `[Authentication]` or `[Security]` on any of the three. The backend
  writes them from the account. A host that you write by hand would hide the
  bug that this run checks for.
- The identity has no backend name, because it is a person, not a service.
- You can change `Address=`. It is the `From:` that Evolution uses.

### 3. Restart and look

Close Evolution, restart the registry as in step 4, then start Evolution.

### What "it worked" means

- `~/.config/evolution/sources/jmap-mock-mail-transport.source` now has two
  groups that you did not write:

  ```
  [Authentication]
  Host=127.0.0.1
  Port=8080

  [Security]
  Method=none
  ```

  The backend bound them from the account and EDS wrote them to the file. The
  same applies to `jmap-mock-mail-account.source`, but not to
  `jmap-mock-mail-identity.source`, which stays as you copied it.
- With TLS on the account (any `Method` other than `none`), the mail sources
  get `Method=ssl-on-alternate-port`, not `tls`. Camel reads that key as a
  `CamelNetworkSecurityMethod` nick, and `tls` is not a valid nick (see
  `rust/crates/jmap-backend-collection/src/mail_child.rs`).
- The account appears in Evolution's mail view with the mock's three
  mailboxes, and a mailbox opens to show its messages. If the folders appear
  but the account cannot connect, compare the transport's `[Authentication]
  Host` with the account's first. `docs/manual-test-mail-provider.md` tests
  the provider without a collection account, which helps to tell a provider
  bug from a binding bug.
- Changing `[Authentication] Host` on the account and restarting the
  registry changes both mail files too. A stale host is also a second
  password prompt, because EDS compares the host strings to decide whether a
  mail source shares the account's password.

`tests/recipe.rs` also checks the parts of this run that need no daemon:
the parents, the UIDs that link the sources, which sources are services, and
that the transport, after `child_added`, gives `jmap-mail`'s reader
`http://127.0.0.1:8080`. What remains for you is that a running registry
dispatches `child-added` for these files and that Evolution shows the result.
