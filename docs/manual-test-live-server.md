# Manual test: a real JMAP server

Every other test in this repository runs against `jmap-mockd`, which answers
exactly what its fixtures told it to. That is the right default — fast,
deterministic, no account anywhere — but it cannot show what a real
deployment's own quirks would: capability objects with fields this client has
never seen, limits that are actually enforced, a session shaped by a server
this project does not control. `rust/crates/jmap-client/tests/live_server.rs`
is the other half, and this is its recipe.

It is read-only. Every test in that file is session discovery, `Core/echo`,
or listing what already exists — nothing here creates, renames or deletes
anything on the server it is pointed at.

## 1. Get a server

Two ways, either is fine:

- **The disposable Stalwart VM** this project provisions for exactly this:
  ```console
  $ ./infra/gcp/create-stalwart.sh
  ```
  reports the VM's IP once it is up. Read its generated admin password with
  ```console
  $ gcloud compute ssh stalwart-1 --zone europe-west3-c -- sudo cat /opt/stalwart/admin-password
  ```
  and use Stalwart's admin UI (`http://<ip>:8080`) to create a mailbox account
  and its password — that account is what the tests authenticate as. The VM
  naps itself after 60 minutes idle; wake it with
  `gcloud compute instances start stalwart-1`.
- **Any other JMAP server** you already have an account on (a Fastmail
  account works, per the roadmap's eventual second target). Basic auth against
  Fastmail needs an app password, not the account password.

## 2. Set the environment

```console
$ export JMAP_LIVE_SERVER_URL=https://jmap.example.com   # or http://<stalwart-ip>:8080
$ export JMAP_LIVE_SERVER_USER=me@example.com
$ export JMAP_LIVE_SERVER_PASSWORD=...
```

Or, for a Bearer token obtained some other way (there is no OAuth2 login flow
in this repository yet — see `docs/ROADMAP.md`'s "real-server readiness"
item and `docs/NIGHT-LOG.md`'s "two-hundred-and-seventy-seventh session" entry
for why not):

```console
$ export JMAP_LIVE_SERVER_TOKEN=...
```

`JMAP_LIVE_SERVER_TOKEN` takes priority if both are set.

## 3. Run it

```console
$ cargo test -p evolution-jmap-client --features live-server -- --ignored
```

Both gates matter: without `--features live-server` the test file is not even
compiled, and without `--ignored` it is skipped even with the feature on — so
a plain `cargo test` or `ci/checks.sh` never reaches out to a network. Missing
an environment variable is a clear panic naming which one, not a silent skip:
this test is never run by accident, so reaching it unconfigured is a mistake
in the invocation, worth failing loudly on.

## What "it worked" means

- `the_session_names_the_core_capability` — the server answered
  `/.well-known/jmap` with the core capability and at least one account for
  the credentials given.
- `echo_round_trips_through_the_real_api_endpoint` — a method call reaches the
  real `apiUrl` and comes back parsed the way this client expects, not just
  the session document.
- `mail_capable_accounts_list_a_non_empty_mailbox_set` — if the account has
  the mail capability, `Mailbox/get` lists at least an Inbox. An account with
  no mail capability at all is reported and skipped rather than failed: this
  test is about tolerating what a real deployment does and does not offer,
  which cuts both ways.
- `contacts_capable_accounts_can_list_their_address_books` and
  `calendars_capable_accounts_can_list_their_calendars` — if the account has
  the respective capability, `AddressBook/get` or `Calendar/get` deserialises
  without error. Unlike the mailbox test, neither asserts a non-empty list: a
  fresh account is not guaranteed to have created an address book or a
  calendar yet, so the round trip succeeding — proving this client's types
  read what a real server actually sends, not just `jmap-mockd`'s fixtures —
  is the claim. An account with no such capability is reported and skipped.

Anything short of that is a finding, not a nuisance — write it down in
`docs/NIGHT-LOG.md`.
