# Manual test: the JMAP mail provider

The test suite exercises every layer of the Camel provider except the one that
matters to a user: Camel finding `libcameljmap.so` by the `.urls` file next to
it, opening a store from a `.source` file, and Evolution showing the account's
folders in the mail view. Camel only dlopens a provider when something asks for
a protocol one of its installed `.urls` files claims, so that path needs an
install tree and a running Evolution — it cannot be a `cargo test`, and it is
this recipe.

It talks to `jmap-mockd`, not to a real server, so it can be run on a laptop
with nothing installed and no account anywhere.

This is the mail counterpart of `docs/manual-test-book-backend.md` and
`docs/manual-test-cal-backend.md`, and it writes a standalone account with no
`Parent=` for the same reason they do: until M7's setup UI exists there is
nothing that mints the three sources a mail account is made of. The other end of
that is the mail run in `docs/manual-test-collection-backend.md`, where the same
three sources hang off a JMAP account and get their server *from* it. Here there
is no account, so the server is written by hand — twice, once for what receives
and once for what sends.

## What is already checked without you

`rust/crates/jmap-mail/tests/recipe.rs` loads the three keyfiles below through
`e_server_side_source_new` — the same call `evolution-source-registry` makes for
every file in its sources directory — and asserts that they mean what this
document says they mean: that both services name the protocol
`camel_provider_module_init` registers, that the account and the identity point
at the identity and the transport by the right uids, that none of the three
hangs off a collection, and that the store and the transport each reach
`http://127.0.0.1:8080` when read through the provider's own
`ServerConfig::from_settings` — the call `connect_sync` makes, off the
`CamelSettings` object `e_source_camel_configure_service` would hand the service.

`tests/provider.rs` asserts the other half, that the entry point registers a
provider for that same protocol and that `libcameljmap.urls` claims it, and
CTest's `install-camel-provider` asserts the `.urls` file reaches the directory
Camel scans. So neither the keyfiles nor the protocol can rot silently; the
steps around them are what you are testing.

## Prerequisites

- Evolution Data Server 3.52 or newer, *installed*: the development headers are
  enough to build, but this recipe runs `evolution-source-registry` and
  Evolution's mail view, which come with the `evolution-data-server` and
  `evolution` packages.
- A session bus. The account below names no user, so nothing asks libsecret for
  a password, but the registry expects a session it can talk on.
- Evolution itself, and not just EDS: unlike an address book, a mail account has
  no client-side test harness in this repository, and the mail view is where a
  store becomes visible.

## 1. Start the mock server

```console
$ cargo run -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

It seeds one account with three mailboxes — Inbox, Sent and Drafts, each with
its JMAP role — and two messages in the Inbox, from Bob and Carol. Leave it
running in its own terminal; every request it serves is one the provider made.

Do not pass `--basic` or `--bearer` for the first run. The sources below name no
user, so the provider connects anonymously and no password prompt gets in the
way of the thing being tested.

## 2. Build and install the provider

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component camel-provider
```

That installs `libcameljmap.so` *and* `libcameljmap.urls` into the directory
`pkg-config --variable=camel_providerdir camel-1.2` reports — on Debian and
Ubuntu, `/usr/lib/evolution-data-server/camel-providers`. Both files, and the
`.urls` is not documentation: `camel_provider_init()` reads the `.urls` files and
nothing else, and a `.so` installed without its `.urls` beside it is a provider
Camel never opens, because nothing ever tells it the protocol exists.

If you would rather not install into a system directory, install into a scratch
tree and point Camel at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component camel-provider
$ export EDS_CAMEL_PROVIDER_DIR=/tmp/jmap/usr/lib/evolution-data-server/camel-providers
```

`EDS_CAMEL_PROVIDER_DIR` *replaces* Camel's provider directory rather than
adding to it — the same shape as `EDS_ADDRESS_BOOK_MODULES` in the address book
recipe, with a larger consequence, because every other mail provider lives in the
directory being replaced. A session started with it set has JMAP and no IMAP,
POP, SMTP or local mail at all, which is fine for this test and the reason it is
not a way to run Evolution day to day. It has to be set in the environment
Evolution itself is started from — the provider is loaded in the mail client's
process, not in a factory.

## 3. Write the three sources

Copy all three into `~/.config/evolution/sources/`. Their file names are their
uids, and those uids are what the `IdentityUid=` and `TransportUid=` lines below
refer to, so renaming one file means editing another.

`docs/examples/jmap-mock-standalone-mail.source` — what Evolution receives
through:

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

`docs/examples/jmap-mock-standalone-identity.source` — who the mail is from:

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

`docs/examples/jmap-mock-standalone-transport.source` — what Evolution sends
through:

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

What each load-bearing line does:

- `BackendName=jmap` on the account and on the transport, and on neither more
  nor fewer. It is the protocol Camel keys its provider table by, the first line
  of `libcameljmap.urls`, and the string `camel_provider_module_init` registers —
  three places that have to agree. A typo here is not a load error: Camel never
  learns of the protocol, and the account fails at connect time with *No provider
  available for protocol '…'*.
- **`[Authentication]` twice.** This is the difference from the collection
  recipe. A store and a transport are two `CamelService`s configured from two
  different sources, and with no collection above them there is nobody to copy a
  server from one to the other. A transport that lost its copy is the quietest
  failure here — the account receives mail perfectly and fails only when the user
  presses Send.
- `Method=none` is what allows plain HTTP, and it is refused for any host that is
  not loopback. **The key is `Method`, not `Secure`**: `ESourceSecurity:secure`
  is a boolean *over* the `Method` string, so a keyfile saying `Secure=true` sets
  nothing EDS reads and comes back as no method at all — which is `none`. Against
  a real server that is an account that refuses to connect, blaming TLS. Camel
  reads this key as a `CamelNetworkSecurityMethod` enum nick, so the other
  spelling that means TLS here is `ssl-on-alternate-port` and *not* EDS's own
  word `tls`.
- No `Parent=` on any of the three. There is no account to hang them off; a
  `Parent=` naming a collection that is not there is a source the registry drops
  on load, which is three files present on disk, no account in Evolution, and
  nothing said about it. The version of these files that *does* have a parent is
  `docs/examples/jmap-mock-mail-*.source`, in the collection recipe.
- No `User=` on either service: with one, the provider asks Camel's session to
  resolve a password and does not contact anything until it has one. To test
  *that* path instead, start the mock with `--basic alice:secret` and add
  `User=alice@example.com` to both `[Authentication]` groups. Expect to be asked
  twice — two sources with two `[Authentication]` groups are two credential
  sources as far as EDS is concerned, and the rule that collapses them
  (`e_util_can_use_collection_as_credential_source`, which compares a child's
  host with its collection's) needs a collection to apply. That the shared-password
  shape is the nicer one is the argument for M6's account rather than this one.
- The identity names no backend, because it is a person rather than a service.
  It reaches no server and needs neither group.
- `Address=` is yours to change; it is the `From:` Evolution would use, and the
  mock seeds an identity for `alice@example.com`, so leaving it alone is what
  makes a send match an identity the server already knows about.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-source-registry
$ SOURCE_REGISTRY_DEBUG=1 evolution-source-registry &
$ CAMEL_DEBUG=all evolution
```

Evolution has to be started *after* the registry, and in an environment carrying
`EDS_CAMEL_PROVIDER_DIR` if you used the scratch tree above — the provider is
dlopened in Evolution's own process.

"JMAP mock mail" should appear in the mail view's account list, with Inbox, Sent
and Drafts under it, and clicking the Inbox should list two messages.

## What "it worked" means

- The account appears at all. If it does not, the bug is in the `.source` files
  or the registry, not in the provider: check `Enabled=true`, and that the file
  names match the uids the other two name.
- Camel opened the module. The mock served `/jmap/session` the moment the
  account was first touched — that request is the provider's `connect_sync`, and
  a provider Camel never opened makes no request at all.
- The folder tree is the mock's three mailboxes, from a single `Mailbox/get`.
  Inbox, Sent and Drafts arrive with their JMAP roles, which is what lets
  Evolution mark the Inbox as the account's inbox rather than as a folder that
  happens to be called one.
- Opening the Inbox made the mock serve `Email/query` and `Email/get` — the
  summaries — and lists Bob's and Carol's messages. Opening one of them makes it
  serve a blob download for the body, which is a plain HTTP GET rather than a
  method call, and shows up in the mock's log as such.
- Marking a message read makes it serve `Email/set`, and the flag survives
  closing and reopening the folder — which also means it went through the
  provider's summary cache and came back.
- Sending a message (*File ▸ New ▸ Mail Message*, addressed anywhere) makes it
  serve `EmailSubmission/set` and lands a copy in Sent. This is the half that
  goes through the transport, and so the half that a transport with no
  `[Authentication]` group fails at — with an error naming the *transport*, which
  is worth reading rather than dismissing, because the account it was sent from
  is fine.

Anything short of that is a bug in this repository, not in the recipe —
report it as you would any other bug in this project.

What is left to you rather than to the test suite is that whole list: the tests
in `rust/crates/jmap-mail/tests/recipe.rs` prove the keyfiles say what this
document says they say, and stop at the point where a daemon would have to run.
That Camel loads the installed module, that Evolution offers the account, and
that the mock sees the requests above are the three things this recipe exists to
have a human check.
