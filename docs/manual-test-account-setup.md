# Manual test: the JMAP account setup UI

The shell of Evolution loads `module-jmap-configuration.so`. A data server
process does not load it. The module builds GTK widgets in `insert_widgets`.
This recipe checks those widgets in a real account assistant.

The recipe uses `jmap-mockd`, not a real server. You do not need an account.

## What automated tests already cover

- `rust/crates/jmap-config/tests/backend.rs` calls every
  `EMailConfigServiceBackendClass` vfunc except `insert_widgets`. It needs no
  display:
  - `new_collection` offers a JMAP account.
  - `setup_defaults` fills the account from the identity address.
  - `check_complete` refuses an account with no address or an unusable
    server.
  - `commit_changes` writes the mail account, the identity and the transport
    sources.
- `rust/crates/jmap-config/tests/oauth2_module.rs` and
  `rust/crates/jmap-functional/tests/config-lookup.rs` run the "Look Up
  Account Details" worker against a real `EConfigLookup`. This includes OAuth
  2.0 discovery and registration against `jmap-mockd --oauth2`.

This recipe checks what you can see. The widgets show what `setup_defaults`
offered. If you edit a field, *Next* becomes sensitive or insensitive at once.

## Prerequisites

- Evolution Data Server 3.52 or newer, and Evolution, installed.
- The components `book-backend`, `cal-backend`, `camel-provider` and
  `collection-backend` installed. They are necessary so that the account that
  step 5 commits opens. Steps 3 and 4 need only the configuration module.
- A D-Bus session bus.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd -- --oauth2
jmap-mockd listening on http://127.0.0.1:8080
```

`--oauth2` turns on the RFC 8414 and RFC 7591 endpoints that step 4 needs.
If you test only the manual fields (steps 3 and 5), omit it.

## 2. Build and install the module

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component config-module
```

This installs `module-jmap-configuration.so` into
`pkg-config --variable=moduledir evolution-shell-3.0` (on Debian and Ubuntu,
`/usr/lib/evolution/modules`). Evolution loads modules only from that
directory. A scratch-tree option does not exist.

To keep your real mail profile out of the test, set these variables in every
terminal before each command below:

```console
$ export XDG_CONFIG_HOME=/tmp/jmap-config-test/config
$ export XDG_DATA_HOME=/tmp/jmap-config-test/data
$ export XDG_CACHE_HOME=/tmp/jmap-config-test/cache
```

## 3. Open the assistant and select JMAP

```console
$ pkill -f evolution-source-registry
$ evolution &
```

1. Open *Edit ▸ Preferences ▸ Mail Accounts ▸ Add*. On an empty profile, you
   can use the first-run assistant.
2. Choose to set up a new account.
3. On the identity page, type an address, for example `alice@example.com`.

**What "it worked" means:** the server type list of the *Receiving Email* page
has "JMAP", with the description "For reading and storing mail on JMAP
servers." (`NAME` and `DESCRIPTION` in `rust/crates/jmap-mail/src/provider.rs`).
If the entry is missing, look for a module load failure in
`journalctl --user -f` before you suspect the widgets.

## 4. "Look Up Account Details"

Make sure that `jmap-mockd --oauth2` runs. On the identity page, click
**Look Up Account Details**.

**What "it worked" means:** the assistant offers a JMAP result that uses OAuth
2.0. If you accept it, the *Receiving Email* page is filled in. Step 5 checks
this page. If the lookup hangs or finds nothing, first run
`rust/crates/jmap-functional/tests/config-lookup.rs`. This separates a UI bug
from a discovery bug.

The lookup needs a server that it can reach. The automated test gives the
origin of the mock server to the lookup directly. The assistant derives the
server from the address. If your Evolution cannot point the lookup at
`127.0.0.1:8080`, go to step 5 and fill in the fields by hand.

### Troubleshooting: no OAuth 2.0 offered by a self-hosted server

Use this section if a self-hosted server returns no OAuth 2.0. In this case,
no error occurs. The *Receiving Email* page shows **Password**. Check the
advertised hostname of the server.

`jmap-client` refuses OAuth metadata if the `issuer` is not exactly the same as
the address from which the client fetched it (RFC 8414 §3.3). This check has no
opt-out. The lookup treats the refusal as "OAuth 2.0 not available". It logs
the refusal only at debug level. To find it, do these steps:

1. Run Evolution with `EVOLUTION_JMAP_LOG=debug`.
2. Look for `requires them to be identical` in `journalctl --user -f`.

**Fix the server, not Evolution.** The advertised issuer of the server must be
the address that clients use. For example, this check fails on a Stalwart
server whose `SystemSettings.defaultHostname` is still `example.com`. Password
sign-in to the same server still works.

## 5. The server settings page

The *Receiving Email* page shows these items from top to bottom:

- **Server**, **Port** and **Username** entries with mnemonics (`_Server`,
  `_Port`, `_Username`).
- An **Authentication** combo box: **Password**, **OAuth 2.0** and **API
  Token**.
- A **Use a secure connection (TLS)** check button.
- A status line. It is empty if *Next* accepts the account.

Point the page at the mock server:

1. Set **Server** to `127.0.0.1`.
2. Set **Port** to `8080`.
3. Clear **Use a secure connection (TLS)**. The mock server speaks plain HTTP.
   The backend allows plain HTTP only for a loopback address.
4. Leave **Username** as offered, or clear it. The mock server accepts
   anonymous connections, unless you start it with `--basic` or `--bearer`.

**What "it worked" means:**

- The fields start with the values that the identity address implies.
  **Server** is the domain of the address. **Username** is the address.
  **Authentication** is **Password**.
- If you finish with **Password**, the module writes `Method=none` under
  `[Authentication]` in the `.source` file of the account. This is the file in
  `$XDG_CONFIG_HOME/evolution/sources/` with `BackendName=jmap`. **API Token**
  writes `Method=bearer`.
- If you choose **OAuth 2.0** by hand, without step 4, *Next* stays
  insensitive. The status line says: OAuth 2.0 can't be set up manually. Go
  back and use "Look Up Account Details" instead.
- If the identity address is empty, the status line reads `This account has no
  email address yet.`. If you typed text without an `@`, the status line reads
  `"<what you typed>" is not an email address.`.
- If the host is not a loopback address and TLS is off (for example
  `mail.example.com`), the status line explains the refusal. *Next* becomes
  insensitive. If you turn TLS on, or go back to `127.0.0.1`, the message
  clears and *Next* becomes sensitive again. You do not leave the page. No
  `cargo test` can see this live update.
- If you finish with the settings of the mock server, the assistant commits an
  account. This account behaves like the hand-written account in
  `docs/manual-test-collection-backend.md`.

Any other result is a bug in this repository. Report it.

## What this does not cover

This recipe does not cover the browser consent round trip of a real OAuth 2.0
sign-in. That test needs a real OAuth 2.0 provider. The mock server is not
sufficient.
