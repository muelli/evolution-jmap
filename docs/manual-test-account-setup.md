# Manual test: the JMAP account setup UI

M7's `module-jmap-configuration.so` is the one module in this repository
Evolution's own shell loads rather than a data-server process, and the one
whose vfuncs build GTK widgets — `insert_widgets` needs a display connection
this project's CI and VM do not have, so nothing here has ever run it and
looked at the result. This is that look.

It talks to `jmap-mockd`, not to a real server, so it can be run on a laptop
with nothing installed and no account anywhere.

## What is already checked without you

`rust/crates/jmap-config/tests/backend.rs` drives every vfunc except
`insert_widgets` itself directly against the `EMailConfigServiceBackendClass`
struct, with no display required: `new_collection` offers a fresh JMAP
account, `setup_defaults` fills it from the identity page's address,
`check_complete` refuses to commit one with no address or an unusable server,
and `commit_changes` writes the mail account/identity/transport sources the
collection backend (`docs/manual-test-collection-backend.md`) then fans
address books and calendars out from. `tests/oauth2_module.rs` and
`rust/crates/jmap-functional/tests/config-lookup.rs` cover the "Look Up
Account Details" step's `JmapConfigLookup` worker end to end against a real
`EConfigLookup`, including OAuth 2.0 discovery and registration against
`jmap-mockd --oauth2` — that step's *network* behaviour is proven; only
seeing it inside the actual assistant window is left to this recipe.

What no test can do is construct the widgets `insert_widgets` builds, see
that they show what `setup_defaults` offered, or watch editing one flip
*Next* from insensitive to sensitive. That is what steps 3–5 below check.

## Prerequisites

- Evolution Data Server 3.52 or newer, *installed*, and Evolution itself —
  the development headers are enough to build, but this recipe runs the
  actual shell and account assistant.
- The address book, calendar, mail (Camel) and collection-backend components
  installed too (`--component book-backend`, `cal-backend`, `camel-provider`,
  `collection-backend`), so an account committed in step 5 is one whose
  children actually open rather than sources with no backend behind them.
  Optional for steps 3–4 alone, which only look at the *Receiving Email*
  page.
- A session bus, for the same reason `docs/manual-test-collection-backend.md`
  needs one: the registry the assistant talks to expects a session, even
  though the account below names no user and asks libsecret for nothing.

## 1. Start the mock server

```console
$ cargo run -p evolution-jmap-mock --bin jmap-mockd -- --oauth2
jmap-mockd listening on http://127.0.0.1:8080
```

`--oauth2` turns on the RFC 8414/7591 discovery endpoints the "Look Up
Account Details" step (step 4) needs; leave it off if you are only looking
at the plain server-settings fields (steps 3 and 5). Leave it running in its
own terminal.

## 2. Build and install the module

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component config-module
```

That installs `module-jmap-configuration.so` into
`pkg-config --variable=moduledir evolution-shell-3.0` — on Debian and
Ubuntu, `/usr/share/evolution/${VERSION}/modules` or similar, whatever that
command prints. Unlike the registry modules, there is no environment
variable to redirect this one at a scratch tree: Evolution's shell loads
every `.so` in the one directory it knows, so installing anywhere else means
it is never found at all.

If you already have another Evolution profile you care about, consider
running this against a scratch one instead of your real mail:

```console
$ export XDG_CONFIG_HOME=/tmp/jmap-config-test/config
$ export XDG_DATA_HOME=/tmp/jmap-config-test/data
$ export XDG_CACHE_HOME=/tmp/jmap-config-test/cache
```

set before every command below, in every terminal, including the registry
restart in step 4's collection-backend prerequisites if you get that far.

## 3. Open the assistant and reach the JMAP provider

```console
$ pkill -f evolution-source-registry
$ evolution &
```

*Edit → Preferences → Mail Accounts → Add*, or the first-run assistant on a
profile with no accounts yet. On the *Restore or set up a new mail account*
page (or wherever Evolution's version puts it), pick set up a new account,
then type an address on the identity page — `alice@example.com` is fine, it
does not have to resolve to anything for this step.

**What "it worked" means:** the *Receiving Email* page's provider combo
lists an entry named "JMAP", with the one-line description
`rust/crates/jmap-mail/src/provider.rs`'s `DESCRIPTION` names, and picking it
is what makes the rest of this recipe possible. Its absence is `NAME` in
`jmap_mail::provider` not matching the string `check_complete` and friends
were registered under, or the module not being loaded at all — check
`journalctl --user -f` or Evolution's own debug output
(`EDS_DEBUG=1 evolution &`) for a load failure before assuming the widgets
are the bug.

## 4. The "Look Up Account Details" step

Still on the identity page, or wherever this Evolution version puts the
button, look for **Look Up Account Details** (evolution-ews's Exchange
autodiscovery uses the same button; JMAP shares the mechanism, not the
provider). Click it with `jmap-mockd --oauth2` running.

**What "it worked" means:** the assistant's own progress spinner runs
briefly, then offers a result naming the mock's collection settings —
`rust/crates/jmap-functional/tests/config-lookup.rs` is the automated version
of exactly this network round trip, so if this hangs or comes back empty,
run that test first to tell a UI-only bug from a discovery-logic one.
Accepting the result is expected to carry you to a *Receiving Email* page
already filled in, which folds into step 5's checks below.

This step needs `servers` (the `EConfigLookup`'s target list) to name a host
the lookup worker can actually reach — the automated test hands it the
mock's own origin directly. The assistant UI has no field for this; if it
does not offer a way to point discovery at `127.0.0.1:8080` in your
Evolution version, skip to step 5 and fill the fields in by hand instead —
that is the fallback path this whole recipe exists to prove works too.

## 5. The server-settings page

However you arrived at the *Receiving Email* page — by hand or via step 4 —
it should show, top to bottom:

- **Server**, **Port**, **Username** entries, in that order, each with a
  mnemonic label (`_Server`, `_Port`, `_Username` — <kbd>Alt</kbd>+the
  underlined letter should focus the entry).
- An **Authentication** combo below them, defaulting to **Password**, with
  **OAuth 2.0** as its other choice.
- A **Use a secure connection (TLS)** check button below that.
- A status line below that, blank when the account is one *Next* would
  accept.

Edit the fields to point at the mock and clear its refusal:

1. Set **Server** to `127.0.0.1`, **Port** to `8080`, and *uncheck* **Use a
   secure connection (TLS)** — the mock speaks plain HTTP on loopback, and
   `jmap-backend-core`'s TLS rule (M3) refuses plaintext to anything else.
2. Leave **Username** as whatever the identity address offered, or clear it
   — either is a legitimate account (`jmap-mockd` accepts an anonymous
   connection unless started with `--basic`/`--bearer`).

**What "it worked" means:**

- The three entries, the authentication combo and the check button appear at
  all, filled in with what the identity's address implied (`Server` starts
  as the address's domain, `Username` as the address itself, and
  **Authentication** starts on **Password** for a fresh account) —
  `jmap-config`'s `setup_defaults` doing its job, now visible rather than
  only asserted in `tests/backend.rs`.
- Switching **Authentication** to **OAuth 2.0** and finishing the assistant
  writes `Method=JMAP` under `[Authentication]` in the committed account's
  `.source` file (find it under
  `$XDG_CONFIG_HOME/evolution/sources/*.source` — the one whose
  `BackendName=jmap`); switching back to **Password** and finishing writes
  `Method=none` there instead. Nothing here proves the OAuth 2.0 sign-in
  itself works — see "What this does not cover" below.
- The status line reads `This account has no email address yet.` if you
  clear the identity page's address and come back, or
  `"<whatever you typed>" is not an email address.` for something that is
  not one (an address with no `@`) — `complete::status_message`'s exact
  strings, worth checking verbatim since a translator will see them
  literally.
- Typing a non-loopback host with TLS unchecked (e.g. `mail.example.com`
  with **Use a secure connection** off) makes the status line non-empty
  with a message naming the refusal, and greys out *Next* — clearing it
  (checking TLS, or pointing the host back at `127.0.0.1`) makes the message
  disappear and *Next* sensitive again, live, with no page revisit needed.
  This is the one behaviour no `cargo test` here can see: the `notify`
  handler `insert_entries` connects on `[Authentication]`/`[Security]`
  actually firing and updating the label GTK is showing.
- Finishing the assistant with the mock's settings above commits an account
  that, once the collection, book, calendar and camel-provider components
  are installed and the registry restarted, behaves exactly as
  `docs/manual-test-collection-backend.md` describes for a hand-written
  `.source` file of the same shape — because `commit_changes` writes
  precisely that file.

Anything short of the above is a bug in this repository, not in the recipe —
report it as you would any other bug in this project.

## What this does not cover

- **Whether choosing OAuth 2.0 here actually connects.** The combo writes
  `[Authentication] Method`, which is what `EOAuth2Service::can_process`'s
  default implementation and `e_source_get_oauth2_access_token_sync` key
  off of — but nothing on this page drives `discover_and_register` (that is
  what step 4's "Look Up Account Details" does). Picking **OAuth 2.0** by
  hand without running that step no longer reaches a committed account
  though: `check_complete` (`complete::check`) now refuses to go sensitive
  while `[Authentication] Method` names OAuth 2.0 and no `[JMAP OAuth2]`
  client is registered, with the status label saying so
  (`OAuth2NotRegistered`, tested in `tests/backend.rs`). What is still
  worth eyeballing on the first real run is the label actually appearing —
  the `notify` wiring that repaints it is real-Evolution-only, per the
  section above.
- **The consent browser round trip.** Even once discovery has registered a
  client, watching a real authorization prompt and `REDIRECT_URI` land back
  in Evolution needs a real OAuth2 provider, not the mock, and is out of
  scope for this document.
