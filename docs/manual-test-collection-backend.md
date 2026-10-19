# Manual test: the JMAP collection backend

The test suite covers every layer of the collection backend except one step.
In this step, `evolution-source-registry` loads the module. It turns one
account `.source` file into the address books and calendars in the sidebar of
Evolution. An `ECollectionBackend` needs a running registry. This step cannot
be a `cargo test`. This recipe covers it.

The recipe uses `jmap-mockd`, not a real server. You do not need an account.

The address book and calendar recipes (`docs/manual-test-book-backend.md`,
`docs/manual-test-cal-backend.md`) write standalone sources with no `Parent=`.
In this recipe, the backend writes the child sources. You check that they
appear.

## What automated tests already cover

`rust/crates/jmap-backend-collection/tests/recipe.rs` loads the keyfiles in
this document with `e_server_side_source_new`. The test checks these items:

- the origin
- the anonymous connection
- which parts are on
- that `BackendName` matches the factory name
- that each `ini` block in this document is identical to its file in
  `docs/examples/`

`tests/factory.rs` checks that the module registers a factory with that name.

## Prerequisites

- Evolution Data Server 3.52 or newer, installed (the
  `evolution-data-server` package). This recipe runs
  `evolution-source-registry`.
- The address book and calendar backends, also installed
  (`--component book-backend` and `--component cal-backend`). Without them,
  the child sources appear in the sidebar, but they fail to open.
- A D-Bus session bus.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

The mock server seeds one account. The account has an address book and a
calendar, both named "Personal". Each is the default of its kind. The account
also has three mailboxes (Inbox, Sent, Drafts). Leave the server running in
its own terminal.

Do not use `--basic` or `--bearer` for the first run.

## 2. Build and install the module

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component collection-backend
```

This installs `module-jmap-backend.so` into
`pkg-config --variable=moduledir libebackend-1.2` (on Debian and Ubuntu,
`/usr/lib/evolution-data-server/registry-modules`).

To avoid a system directory, install into a scratch tree. Then point the
registry at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component collection-backend
$ export EDS_REGISTRY_MODULES=/tmp/jmap/usr/lib/evolution-data-server/registry-modules
```

`EDS_REGISTRY_MODULES` replaces the module directory. The registry then lacks
its own modules (cache reaper, WebDAV and Google collections, OAuth2). This is
acceptable for this test. It is not acceptable for daily use. Start the
registry by hand in that environment (step 4).

## 3. Write the account

Copy `docs/examples/jmap-mock-collection.source` to
`~/.config/evolution/sources/jmap-mock-collection.source`. The file name
becomes the source UID. The children name it in `Parent=`.

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

- `[Collection]` makes this source an account. It is not a single address
  book.
- `BackendName=jmap` selects this backend. A typo here gives no error. The
  account stays in the sidebar with no children.
- `ContactsEnabled` and `CalendarEnabled` select what the backend creates.
  In a second run, try to turn one of them off. The backend removes the
  children of that kind. It does not only hide them.
- `MailEnabled=false`, because the collection backend does not create mail
  sources. The account assistant of Evolution creates them. The second run
  below writes them by hand.
- `Method=none` allows plain HTTP, and only for a loopback address. **The key
  is `Method`, not `Secure`**: EDS ignores `Secure=true`. The method then
  stays `none`, and the backend refuses a remote server.
- There is no `User=`. To test password sign-in, do these steps:
  1. Start the mock server with `--basic alice:secret`.
  2. Add `User=alice` and `Method=plain/password` to `[Authentication]`.
  3. Let Evolution prompt you.

  No automated test covers that credentials round trip.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-source-registry
$ pkill -f evolution-addressbook-factory
$ pkill -f evolution-calendar-factory
$ SOURCE_REGISTRY_DEBUG=1 G_MESSAGES_DEBUG=all evolution-source-registry
```

Then start Evolution in another terminal. If `SOURCE_REGISTRY_DEBUG=1` is set,
EDS and this backend log each populate and each new child.

In Evolution, "JMAP mock account" appears with an address book in the Contacts
view and a calendar in the Calendar view.

## What "it worked" means

- The registry loaded `module-jmap-backend.so` and logged a populate for the
  account. If no factory claims the `BackendName`, no populate occurs.
- The mock server served `/jmap/session`, then `AddressBook/get` and
  `Calendar/get`. It served no `Mailbox/get`, because mail is off.
- New files appeared in `~/.config/evolution/sources/`, one for each
  collection. Each file has `Parent=jmap-mock-collection` and an
  `[Address Book]` or `[Calendar]` group with `BackendName=jmap`. The backend
  wrote them.
- The children open. If a child opens but reaches the wrong server, look at
  `child_source`.
- A restart of the registry does not duplicate the children.
- If you set `ContactsEnabled=false` and restart, the backend removes the
  address book children and their `.source` files.
- If you change `[Authentication] Host` (or `Port`, `User`, or `[Security]
  Method`) in the file of the account and restart the registry, every child
  file changes too. `child_added` binds these properties from the account onto
  each child. A child with the old server keeps working only until that
  server goes away. Check this item.

Any other result is a bug in this repository. Report it.

## Shared address books and calendars

Another user can share an address book or a calendar with the account
(RFC 9670). The JMAP session then lists the account of that user with
`isPersonal: false`. The backend makes one child source for each address book
and calendar in each such account. Mail in a shared account does not appear.

- The name of the child is `<collection> (<owner>)`. `<owner>` is the `name`
  of the shared account in the session. Usually it is the email address of
  the owner.
- The child is read-only if the shared account is read-only or if `myRights`
  of the collection does not give write access.
- "Delete" is available only if `myRights.mayDelete` is `true`. A delete
  removes the collection of the owner for every user.
- The child appears also if `isSubscribed` is `false`. A sharee often cannot
  change `isSubscribed`, and Evolution has no control for it.
- If the owner withdraws the share, the next populate removes the child.

`[Resource] Identity` of a shared child is `<accountId>/<collectionId>`.
The address book and calendar backends use it to find the account. An
identity is unique only for one kind of collection: a server can give an
address book and a calendar the same id.

`jmap-mockd` starts with no shared accounts. The test
`functional-collection-shared-live-stalwart` (`docs/functional-tests.md`)
checks this behaviour with two accounts on a real server.

## A second run: the mail sources

The collection backend does not create mail sources, but it gives them their
server. The assistant of Evolution creates a transport that has only the
backend name `jmap`. The *Sending Email* page is hidden for a provider that
both stores and sends. The transport reaches a server only after `child_added`
binds the host, port, user and security method of the account onto it. This
run writes the three mail sources by hand. It checks that binding without a
GUI.

### Additional prerequisites

- The Camel provider: `sudo cmake --install build --component
  camel-provider`. Without it, Evolution ignores the mail sources.
- Remove the account from step 3
  (`rm ~/.config/evolution/sources/jmap-mock-collection.source`). You then do
  not have two accounts against the same mock server.

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

This is the account from step 3 with `MailEnabled=true`. EDS binds the
`enabled` property of each mail source to the `mail-enabled` property of the
account. Under an account with mail off, the three files below load disabled.
They do not appear in Evolution, and they log nothing.

### 2. The three mail sources

Copy all three files into the same directory. The file names are the UIDs.
The lines `Parent=`, `IdentityUid=` and `TransportUid=` refer to those UIDs.
If you rename one file, edit the other files.

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

- `Parent=jmap-mock-mail-collection` on all three files makes them children of
  the account. `child_added` therefore sees them. A typo here gives orphan
  sources. They appear nowhere and log nothing.
- `BackendName=jmap` on the account and the transport names the Camel
  provider. The backend binds only the children with this backend name. A
  transport of another provider under this account keeps its own server and
  password.
- None of the three files has `[Authentication]` or `[Security]`. The backend
  writes them from the account. A host that you write by hand would hide the
  bug that this run checks for.
- The identity has no backend name, because it is a person. It is not a
  service.
- You can change `Address=`. It is the `From:` address that Evolution uses.

### 3. Restart and look

1. Close Evolution.
2. Restart the registry as in step 4.
3. Start Evolution.

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

  The backend bound them from the account. EDS wrote them to the file. The
  same applies to `jmap-mock-mail-account.source`. It does not apply to
  `jmap-mock-mail-identity.source`. This file stays as you copied it.
- If TLS is on for the account (any `Method` other than `none`), the mail
  sources get `Method=ssl-on-alternate-port`, not `tls`. Camel reads that key
  as a `CamelNetworkSecurityMethod` nick. `tls` is not a valid nick (see
  `rust/crates/jmap-backend-collection/src/mail_child.rs`).
- The account appears in the mail view of Evolution with the three mailboxes
  of the mock server. A mailbox opens and shows its messages. If the folders
  appear but the account cannot connect, first compare `[Authentication]
  Host` of the transport with `[Authentication] Host` of the account.
  `docs/manual-test-mail-provider.md` tests the provider without a collection
  account. This test helps you to find out whether the fault is in the
  provider or in the binding.
- If you change `[Authentication] Host` on the account and restart the
  registry, both mail files change too. A stale host also causes a second
  password prompt. EDS compares the host strings to decide whether a mail
  source shares the password of the account.

`tests/recipe.rs` also checks the parts of this run that need no daemon:

- the parents
- the UIDs that link the sources
- which sources are services
- that the transport, after `child_added`, gives `http://127.0.0.1:8080` to
  the reader of `jmap-mail`

You must check two items: a running registry dispatches `child-added` for
these files, and Evolution shows the result.
