# Manual test: a real JMAP server

Every other test in this repository runs against `jmap-mockd`, which answers
exactly what its fixtures told it to. That is the right default — fast,
deterministic, no account anywhere — but it cannot show what a real
deployment's own quirks would: capability objects with fields this client has
never seen, limits that are actually enforced, a session shaped by a server
this project does not control. `rust/crates/jmap-client/tests/live_server.rs`
is the other half, and this is its recipe.

Almost all of it is read-only — session discovery, `Core/echo`, or listing
what already exists. Four tests write: `mailbox_create_rename_then_destroy_
round_trips_through_the_real_api` (`Mailbox/set` create, then a rename via
`mailbox_update`, then destroy),
`contact_card_create_update_then_destroy_round_trips_through_the_real_api`
(`ContactCard/set` create, then a rename via `contact_update`, then destroy),
`calendar_event_create_update_then_destroy_round_trips_through_the_real_api`
(`CalendarEvent/set` create, then a title change via `event_update`, then
destroy), and
`email_import_update_then_destroy_round_trips_through_the_real_api`
(`Email/import` via `upload_blob`, a mark-as-read via `email_update`, then
`download_blob`). All four run only against a separate, dedicated throwaway
account (see step 3) so they can never touch whatever account the read-only
tests are pointed at.

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

If the deployment's session document names an `apiUrl` (and
`downloadUrl`/`uploadUrl`/`eventSourceUrl`) this runner cannot route to, even
though `JMAP_LIVE_SERVER_URL` itself is reachable — a reverse proxy, a NAT
boundary, or a configured public hostname advertised over `https` when only a
plain-`http` listener on a different address actually answers, as a
disposable Stalwart 0.16 test deployment does (see `docs/NIGHT-LOG.md`,
"apiUrl's scheme is hardcoded https, not just the hostname") — set:

```console
$ export JMAP_LIVE_SERVER_REBASE_URLS=1
```

This makes the client keep every URL the session names *pathwise*, but
rewrite its scheme and authority to the one `JMAP_LIVE_SERVER_URL` actually
reached, rather than trusting the address the server states. Leave it unset
by default: it exists for exactly this apiUrl/hostname mismatch, not as a
routine option.

## 3. (optional) Enable the write-path tests

The four mutating tests are skipped, not failed, unless a *separate*
login is given for them — deliberately not the same variables step 2
sets, so pointing the read-only tests at a real mailbox can never also
point the mutating tests at it. Seed a throwaway account for this alone
(the Stalwart VM's `infra/stalwart/stw seed` wrapper needs `stalwart-cli`
on `PATH`; see that script's header for where to get it):

```console
$ ./infra/stalwart/stw seed agent-livewrite.net agent1 '<a fresh password>'
$ export JMAP_LIVE_SERVER_WRITE_USER=agent1@agent-livewrite.net
$ export JMAP_LIVE_SERVER_WRITE_PASSWORD='<that password>'
```

`stw seed` is idempotent (an upsert), so re-running it — e.g. from a later
session that lost the password — just resets that one account's password
rather than erroring or duplicating anything. Use a domain name that is
obviously a test fixture (`agent-*`), never the operator's own
(`example.com`, `alice@example.com`).

## 4. Run it

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
- `mailbox_create_rename_then_destroy_round_trips_through_the_real_api`,
  `contact_card_create_update_then_destroy_round_trips_through_the_real_api`,
  and `calendar_event_create_update_then_destroy_round_trips_through_the_real_api`
  — each creates a record (`Mailbox`/`ContactCard`/`CalendarEvent`), confirms
  it via the matching `/get`, then destroys it — the write path, not just
  reads, against a real server's own id assignment and state changes. All
  three additionally update the record — a folder rename via
  `mailbox_update`'s `PatchObject`, a name change via `contact_update`'s, a
  title change via `event_update`'s — each confirmed via another `/get`
  before destroying it (what a real folder rename, contact edit, or event
  edit in Evolution sends). The contact/event tests create into the
  account's default address book/calendar rather than one of their own
  making, relying on Stalwart auto-provisioning both per account. Skipped,
  not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` (step 3) are not
  set.
- `email_import_update_then_destroy_round_trips_through_the_real_api` — the
  mail write path's other shape: uploads a small message's bytes via
  `Client::upload_blob`, imports it into the account's Inbox via
  `Email/import`, confirms it via `Email/get`, marks it read via
  `email_update`'s `{"keywords/$seen": true}` `PatchObject` (what
  `jmap-mail-sync::MailSync::set_keywords` sends whenever a user marks a
  message read/unread or flags it), confirms the `$seen` keyword via
  another `Email/get`, downloads the blob back via `Client::download_blob`,
  then destroys it. Does not require the downloaded bytes to equal the
  uploaded bytes verbatim — RFC 8621 §4.8 allows a server to repair or
  re-serialize an imported message — only that the downloaded length
  matches the `size` `Email/get` itself reports and that the message's
  subject survived. Skipped under the same condition as the other three.

Anything short of that is a finding, not a nuisance — write it down in
`docs/NIGHT-LOG.md`.
