# Manual test: the JMAP mail provider

The test suite covers every layer of the Camel provider except Camel finding
`libcameljmap.so` through its `.urls` file, opening a store from a `.source`
file, and Evolution showing the folders. That needs an installed module and a
running Evolution, so it cannot be a `cargo test`. This recipe covers it.

It uses `jmap-mockd`, not a real server, so you need no account.

Like the address book and calendar recipes, it writes standalone sources with
no `Parent=`. Because there is no collection account, you write the server
twice: once for the store and once for the transport. The mail run in
`docs/manual-test-collection-backend.md` is the version where the sources get
their server from a JMAP account.

## What automated tests already cover

`rust/crates/jmap-mail/tests/recipe.rs` loads the three keyfiles below with
`e_server_side_source_new`. It checks that both services name the protocol
that the provider registers, that the UIDs link the sources correctly, that
no source has a parent, and that the store and the transport both reach
`http://127.0.0.1:8080` through the provider's `ServerConfig::from_settings`.
It also checks that each `ini` block here is identical to its file in
`docs/examples/`.

`tests/provider.rs` checks that the provider registers that protocol and
that `libcameljmap.urls` claims it. The CTest test `install-camel-provider`
checks that the `.urls` file is installed where Camel looks.

## Prerequisites

- Evolution Data Server 3.52 or newer and Evolution, installed. Mail has no
  client-side test harness in this repository, so you need the mail view.
- A D-Bus session bus.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

It seeds one account with Inbox, Sent and Drafts (each with its JMAP role),
an identity for `alice@example.com`, and two messages in the Inbox, from Bob
and Carol. Leave it running in its own terminal.

Do not pass `--basic` or `--bearer` for the first run. The sources below
have no user, so the provider connects anonymously.

## 2. Build and install the provider

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component camel-provider
```

This installs `libcameljmap.so` and `libcameljmap.urls` into
`pkg-config --variable=camel_providerdir camel-1.2` (on Debian and Ubuntu,
`/usr/lib/evolution-data-server/camel-providers`). Camel finds providers
only through `.urls` files, so a `.so` without its `.urls` is never loaded.

To avoid a system directory, install into a scratch tree and point Camel at
it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component camel-provider
$ export EDS_CAMEL_PROVIDER_DIR=/tmp/jmap/usr/lib/evolution-data-server/camel-providers
```

`EDS_CAMEL_PROVIDER_DIR` replaces Camel's provider directory, so the session
then has no IMAP, POP, SMTP or local mail. That is fine for this test, not
for daily use. Set it in the environment that starts Evolution, because the
provider loads in Evolution's process.

## 3. Write the three sources

Copy all three into `~/.config/evolution/sources/`. Their file names are
their UIDs, and the `IdentityUid=` and `TransportUid=` lines refer to them.
If you rename one file, edit the others.

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

- `BackendName=jmap` on the account and the transport. Camel's provider
  table, the first line of `libcameljmap.urls` and the provider's
  registration must all use this string. A typo gives no load error: the
  account fails at connect time with *No provider available for protocol*.
- **`[Authentication]` twice.** The store and the transport are two
  `CamelService`s configured from two sources, and without a collection
  nothing copies the server from one to the other. Without it on the
  transport, the account receives mail but fails when you press Send.
- `Method=none` allows plain HTTP, and only for a loopback host. **The key is
  `Method`, not `Secure`**: EDS ignores `Secure=true`. Camel reads the key as
  a `CamelNetworkSecurityMethod` nick, so the value for TLS is
  `ssl-on-alternate-port`, not `tls`.
- No `Parent=`. The registry drops a source whose parent does not exist,
  without a message. The collection recipe has the parented version
  (`docs/examples/jmap-mock-mail-*.source`).
- No `User=`. To test password sign-in, start the mock with
  `--basic alice:secret` and add `User=alice` to both `[Authentication]`
  groups. Expect two password prompts: EDS treats the two sources as separate
  credentials unless a collection account links them.
- The identity has no backend name, because it is a person, not a service.
- `Address=alice@example.com` matches the identity that the mock seeds, so a
  sent message matches a known server identity.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-source-registry
$ SOURCE_REGISTRY_DEBUG=1 evolution-source-registry &
$ CAMEL_DEBUG=all evolution
```

Start Evolution after the registry, with `EDS_CAMEL_PROVIDER_DIR` set if you
used the scratch tree.

"JMAP mock mail" appears in the mail view with Inbox, Sent and Drafts.
The Inbox lists two messages.

## What "it worked" means

- The account appears. If it does not, check the `.source` files
  (`Enabled=true`, file names that match the UIDs).
- Camel loaded the module: the mock served `/jmap/session` when the account
  was first used.
- The folder tree is the mock's three mailboxes, from `Mailbox/get`, and
  Evolution shows the Inbox as the account's inbox (from its JMAP role).
- Opening the Inbox made the mock serve `Email/query` and `Email/get`, and
  shows the messages from Bob and Carol. Opening a message downloads its body
  as a blob (a plain HTTP GET in the mock's log).
- Marking a message read makes the mock serve `Email/set`, and the flag is
  still set after you close and open the folder again.
- Sending a message (*File ▸ New ▸ Mail Message*) makes the mock serve
  `EmailSubmission/set` and puts a copy in Sent. If the transport has no
  `[Authentication]` group, this step fails with an error that names the
  transport.

Anything less is a bug in this repository. Report it.
