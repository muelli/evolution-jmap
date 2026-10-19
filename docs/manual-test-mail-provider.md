# Manual test: the JMAP mail provider

The test suite covers every layer of the Camel provider except three items:

- Camel finds `libcameljmap.so` through its `.urls` file.
- Camel opens a store from a `.source` file.
- Evolution shows the folders.

These items need an installed module and a running Evolution. They cannot be a
`cargo test`. This recipe covers them.

The recipe uses `jmap-mockd`, not a real server. You do not need an account.

Like the address book and calendar recipes, this recipe writes standalone
sources with no `Parent=`. There is no collection account. You therefore write
the server twice: once for the store and once for the transport. The mail run
in `docs/manual-test-collection-backend.md` is the version where the sources
get their server from a JMAP account.

## What automated tests already cover

`rust/crates/jmap-mail/tests/recipe.rs` loads the three keyfiles below with
`e_server_side_source_new`. The test checks these items:

- Both services name the protocol that the provider registers.
- The UIDs link the sources correctly.
- No source has a parent.
- The store and the transport both reach `http://127.0.0.1:8080` through
  `ServerConfig::from_settings` of the provider.
- Each `ini` block here is identical to its file in `docs/examples/`.

`tests/provider.rs` checks that the provider registers that protocol and that
`libcameljmap.urls` claims it. The CTest test `install-camel-provider` checks
that the `.urls` file is installed where Camel looks.

## Prerequisites

- Evolution Data Server 3.52 or newer, and Evolution, installed. This
  repository has no client-side test harness for mail. You therefore need the
  mail view.
- A D-Bus session bus.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

The mock server seeds one account with these items:

- Inbox, Sent and Drafts (each with its JMAP role)
- an identity for `alice@example.com`
- two messages in the Inbox, from Bob and Carol

Leave the server running in its own terminal.

Do not use `--basic` or `--bearer` for the first run. The sources below have
no user. The provider therefore connects anonymously.

## 2. Build and install the provider

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component camel-provider
```

This installs `libcameljmap.so` and `libcameljmap.urls` into
`pkg-config --variable=camel_providerdir camel-1.2` (on Debian and Ubuntu,
`/usr/lib/evolution-data-server/camel-providers`). Camel finds providers only
through `.urls` files. Camel never loads a `.so` without its `.urls` file.

To avoid a system directory, install into a scratch tree. Then point Camel at
it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component camel-provider
$ export EDS_CAMEL_PROVIDER_DIR=/tmp/jmap/usr/lib/evolution-data-server/camel-providers
```

`EDS_CAMEL_PROVIDER_DIR` replaces the provider directory of Camel. The session
then has no IMAP, POP, SMTP or local mail. This is acceptable for this test.
It is not acceptable for daily use. Set the variable in the environment that
starts Evolution, because the provider loads in the process of Evolution.

## 3. Write the three sources

Copy all three files into `~/.config/evolution/sources/`. The file names are
the UIDs. The lines `IdentityUid=` and `TransportUid=` refer to them. If you
rename one file, edit the other files.

`docs/examples/jmap-mock-standalone-mail.source`, the receiving account:

```ini
[Data Source]
DisplayName=JMAP mock mail
Enabled=true

[Mail Account]
BackendName=jmap
IdentityUid=jmap-mock-standalone-identity

[Authentication]
Host=127.0.0.1
Port=8080

[Security]
Method=none
```

`docs/examples/jmap-mock-standalone-identity.source`, the sender:

```ini
[Data Source]
DisplayName=JMAP mock identity
Enabled=true

[Mail Identity]
Name=JMAP mock user
Address=alice@example.com

[Mail Submission]
TransportUid=jmap-mock-standalone-transport
```

`docs/examples/jmap-mock-standalone-transport.source`, the sending service:

```ini
[Data Source]
DisplayName=JMAP mock transport
Enabled=true

[Mail Transport]
BackendName=jmap

[Authentication]
Host=127.0.0.1
Port=8080

[Security]
Method=none
```

Notes on the keys:

- `BackendName=jmap` on the account and the transport. The provider table of
  Camel, the first line of `libcameljmap.urls` and the registration of the
  provider must all use this string. A typo gives no load error. The account
  fails at connect time with *No provider available for protocol*.
- **`[Authentication]` twice.** The store and the transport are two
  `CamelService`s. Two sources configure them. Without a collection, nothing
  copies the server from one source to the other. If the transport does not
  have `[Authentication]`, the account receives mail, but it fails when you
  press Send.
- `Method=none` allows plain HTTP, and only for a loopback address. **The key
  is `Method`, not `Secure`**: EDS ignores `Secure=true`. Camel reads the key
  as a `CamelNetworkSecurityMethod` nick. The value for TLS is therefore
  `ssl-on-alternate-port`, not `tls`.
- No `Parent=`. The registry drops a source whose parent does not exist. It
  gives no message. The collection recipe has the version with parents
  (`docs/examples/jmap-mock-mail-*.source`).
- No `User=`. To test password sign-in, do these steps:
  1. Start the mock server with `--basic alice:secret`.
  2. Add `User=alice` to both `[Authentication]` groups.

  Expect two password prompts. EDS treats the two sources as separate
  credentials, unless a collection account links them.
- The identity has no backend name, because it is a person. It is not a
  service.
- `Address=alice@example.com` matches the identity that the mock server seeds.
  A sent message therefore matches a known server identity.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-source-registry
$ SOURCE_REGISTRY_DEBUG=1 evolution-source-registry &
$ CAMEL_DEBUG=all evolution
```

Start Evolution after the registry. If you used the scratch tree, set
`EDS_CAMEL_PROVIDER_DIR`.

"JMAP mock mail" appears in the mail view with Inbox, Sent and Drafts. The
Inbox lists two messages.

## What "it worked" means

- The account appears. If it does not appear, check the `.source` files
  (`Enabled=true`, file names that match the UIDs).
- Camel loaded the module: the mock server served `/jmap/session` when the
  account was first used.
- The folder tree shows the three mailboxes of the mock server, from
  `Mailbox/get`. Evolution shows the Inbox as the inbox of the account (from
  its JMAP role).
- When you opened the Inbox, the mock server served `Email/query` and
  `Email/get`. Evolution shows the messages from Bob and Carol. When you open
  a message, Evolution downloads its body as a blob (a plain HTTP GET in the
  log of the mock server).
- If you mark a message as read, the mock server serves `Email/set`. The flag
  is still set after you close and open the folder again.
- If you send a message (*File ▸ New ▸ Mail Message*), the mock server serves
  `EmailSubmission/set`, and Evolution puts a copy in Sent. If the transport
  has no `[Authentication]` group, this step fails with an error that names
  the transport.

Any other result is a bug in this repository. Report it.
