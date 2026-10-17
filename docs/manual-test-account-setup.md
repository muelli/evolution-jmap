# Manual test: the JMAP account setup UI

`module-jmap-configuration.so` is loaded by Evolution's shell, not by a data
server process, and it builds GTK widgets in `insert_widgets`. This recipe
checks those widgets in a real account assistant.

It uses `jmap-mockd`, not a real server, so you need no account.

## What automated tests already cover

- `rust/crates/jmap-config/tests/backend.rs` calls every
  `EMailConfigServiceBackendClass` vfunc except `insert_widgets`, without a
  display: `new_collection` offers a JMAP account, `setup_defaults` fills it
  from the identity address, `check_complete` refuses an account with no
  address or an unusable server, and `commit_changes` writes the mail
  account, identity and transport sources.
- `rust/crates/jmap-config/tests/oauth2_module.rs` and
  `rust/crates/jmap-functional/tests/config-lookup.rs` run the "Look Up
  Account Details" worker against a real `EConfigLookup`, including OAuth 2.0
  discovery and registration against `jmap-mockd --oauth2`.

This recipe checks what you can see: the widgets show what
`setup_defaults` offered, and editing a field makes *Next* sensitive or
insensitive at once.

## Prerequisites

- Evolution Data Server 3.52 or newer and Evolution, installed.
- The `book-backend`, `cal-backend`, `camel-provider` and
  `collection-backend` components installed, so that the account committed in
  step 5 opens. Steps 3 and 4 need only the configuration module.
- A D-Bus session bus.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd -- --oauth2
jmap-mockd listening on http://127.0.0.1:8080
```

`--oauth2` turns on the RFC 8414 and RFC 7591 endpoints that step 4 needs.
Leave it out if you test only the manual fields (steps 3 and 5).

## 2. Build and install the module

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component config-module
```

This installs `module-jmap-configuration.so` into
`pkg-config --variable=moduledir evolution-shell-3.0` (on Debian and Ubuntu,
`/usr/lib/evolution/modules`). Evolution loads modules only from that
directory, so there is no scratch-tree option.

To keep your real mail profile out of the test, set these in every terminal
before each command below:

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

Open *Edit ▸ Preferences ▸ Mail Accounts ▸ Add* (or the first-run assistant
on an empty profile). Choose to set up a new account and type an address on
the identity page, for example `alice@example.com`.

**What "it worked" means:** the *Receiving Email* page's server type list has
"JMAP", with the description "For reading and storing mail on JMAP servers."
(`NAME` and `DESCRIPTION` in `rust/crates/jmap-mail/src/provider.rs`). If it
is missing, look for a module load failure in `journalctl --user -f` before
you suspect the widgets.

## 4. "Look Up Account Details"

On the identity page, click **Look Up Account Details** with
`jmap-mockd --oauth2` running.

**What "it worked" means:** the assistant offers a JMAP result that uses
OAuth 2.0. Accepting it leads to a filled-in *Receiving Email* page,
which step 5 checks. If the lookup hangs or finds nothing, run
`rust/crates/jmap-functional/tests/config-lookup.rs` first to separate a UI
bug from a discovery bug.

The lookup needs a server that it can reach. The automated test gives it the
mock's origin directly, but the assistant derives the server from the
address. If your Evolution cannot point the lookup at `127.0.0.1:8080`, go to
step 5 and fill in the fields by hand.

### Troubleshooting: no OAuth 2.0 offered by a self-hosted server

If a self-hosted server comes back without OAuth 2.0 (no error, just a
*Receiving Email* page on **Password**), check the server's advertised
hostname.

`jmap-client` refuses OAuth metadata whose `issuer` does not exactly match
the address it fetched it from (RFC 8414 §3.3). This check has no opt-out.
The lookup treats the refusal as "OAuth 2.0 not available" and logs it only
at debug level. Run with `EVOLUTION_JMAP_LOG=debug` and look for
`requires them to be identical` in `journalctl --user -f`.

**Fix the server, not Evolution:** its advertised issuer must be the address
that clients use. For example, a Stalwart server whose
`SystemSettings.defaultHostname` is still `example.com` fails this check.
Password sign-in to the same server still works.

## 5. The server settings page

The *Receiving Email* page shows, from top to bottom:

- **Server**, **Port** and **Username** entries with mnemonics (`_Server`,
  `_Port`, `_Username`).
- An **Authentication** combo: **Password**, **OAuth 2.0** and **API Token**.
- A **Use a secure connection (TLS)** check button.
- A status line, empty when *Next* would accept the account.

Point it at the mock:

1. Set **Server** to `127.0.0.1` and **Port** to `8080`, and clear **Use a
   secure connection (TLS)**. The mock speaks plain HTTP, which the backend
   allows only for loopback.
2. Leave **Username** as offered, or clear it. The mock accepts anonymous
   connections unless you start it with `--basic` or `--bearer`.

**What "it worked" means:**

- The fields start with what the identity address implies: **Server** is the
  address's domain, **Username** is the address, and **Authentication** is
  **Password**.
- Finishing with **Password** writes `Method=none` under `[Authentication]`
  in the account's `.source` file (the one in
  `$XDG_CONFIG_HOME/evolution/sources/` with `BackendName=jmap`).
  **API Token** writes `Method=bearer`.
- Choosing **OAuth 2.0** by hand, without step 4, keeps *Next* insensitive
  and the status line says: OAuth 2.0 can't be set up manually. Go back and
  use "Look Up Account Details" instead.
- The status line reads `This account has no email address yet.` when the
  identity address is empty, and `"<what you typed>" is not an email
  address.` for text without an `@`.
- A non-loopback host with TLS off (for example `mail.example.com`) makes the
  status line explain the refusal and makes *Next* insensitive. Turning TLS on
  or going back to `127.0.0.1` clears the message and makes *Next* sensitive
  again, without leaving the page. No `cargo test` can see this live update.
- Finishing with the mock's settings commits an account that behaves like the
  hand-written one in `docs/manual-test-collection-backend.md`.

Anything less is a bug in this repository. Report it.

## What this does not cover

The browser consent round trip of a real OAuth 2.0 sign-in. That needs a real
OAuth 2.0 provider, not the mock.
