# Manual test: a real JMAP server

Every other test in this repository runs against `jmap-mockd`, which answers
exactly what its fixtures told it to. That is the right default — fast,
deterministic, no account anywhere — but it cannot show what a real
deployment's own quirks would: capability objects with fields this client has
never seen, limits that are actually enforced, a session shaped by a server
this project does not control. `rust/crates/jmap-client/tests/live_server.rs`
is the other half, and this is its recipe.

Almost all of it is read-only — session discovery, `Core/echo`, or listing
what already exists. Seven tests write: `mailbox_create_rename_then_destroy_
round_trips_through_the_real_api` (`Mailbox/set` create, then a rename via
`mailbox_update`, then destroy),
`address_book_create_then_destroy_round_trips_through_the_real_api`
(`AddressBook/set` create then destroy — the collection itself, not a card
inside one),
`contact_card_create_update_then_destroy_round_trips_through_the_real_api`
(`ContactCard/set` create, then a rename via `contact_update`, then destroy),
`calendar_create_then_destroy_round_trips_through_the_real_api`
(`Calendar/set` create then destroy — the calendar counterpart of the
address-book test),
`calendar_event_create_update_then_destroy_round_trips_through_the_real_api`
(`CalendarEvent/set` create, then a title change via `event_update`, then
destroy),
`a_recurring_event_created_with_the_singular_recurrence_rule_round_trips_through_the_real_api`
(`CalendarEvent/set` create with a `recurrenceRule`, then destroy — proves the
singular-object wire shape, jscalendarbis §3.3.3, against a real server), and
`email_import_update_then_destroy_round_trips_through_the_real_api`
(`Email/import` via `upload_blob`, a mark-as-read via `email_update`, then
`download_blob`). All seven run only against a separate, dedicated throwaway
account (see step 3) so they can never touch whatever account the read-only
tests are pointed at.

One further test, `send_email_delivers_to_a_second_account_on_the_real_server`,
writes to *two* throwaway accounts: it sends a message via `Client::send_email`
from the write-test account to a second one (step 3's "send-email test"
below) and polls the recipient's Inbox until the message actually arrives —
proof of intra-server delivery, not just that `EmailSubmission/set` was
accepted. It needs no outbound SMTP relay: both accounts live on the same
Stalwart deployment, so delivery never leaves it.

## 1. Get a server

Two ways, either is fine:

- **The disposable Stalwart VM** the harness repository provisions for exactly this:
  ```console
  $ ./harness/gcp/create-stalwart.sh
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
in this repository yet):

```console
$ export JMAP_LIVE_SERVER_TOKEN=...
```

`JMAP_LIVE_SERVER_TOKEN` takes priority if both are set.

If the deployment's session document names an `apiUrl` (and
`downloadUrl`/`uploadUrl`/`eventSourceUrl`) this runner cannot route to, even
though `JMAP_LIVE_SERVER_URL` itself is reachable — a reverse proxy, a NAT
boundary, or a configured public hostname advertised over `https` when only a
plain-`http` listener on a different address actually answers, as a
disposable Stalwart 0.16 test deployment does — set:

```console
$ export JMAP_LIVE_SERVER_REBASE_URLS=1
```

This makes the client keep every URL the session names *pathwise*, but
rewrite its scheme and authority to the one `JMAP_LIVE_SERVER_URL` actually
reached, rather than trusting the address the server states. Leave it unset
by default: it exists for exactly this apiUrl/hostname mismatch, not as a
routine option.

## 3. (optional) Enable the write-path tests

The seven mutating tests are skipped, not failed, unless a *separate*
login is given for them — deliberately not the same variables step 2
sets, so pointing the read-only tests at a real mailbox can never also
point the mutating tests at it. Seed a throwaway account for this alone
(the harness repository's `harness/stalwart/stw seed` wrapper needs
`stalwart-cli` on `PATH`; see that script's header for where to get it):

```console
$ ./harness/stalwart/stw seed agent-livewrite.net agent1 '<a fresh password>'
$ export JMAP_LIVE_SERVER_WRITE_USER=agent1@agent-livewrite.net
$ export JMAP_LIVE_SERVER_WRITE_PASSWORD='<that password>'
```

`stw seed` is idempotent (an upsert), so re-running it — e.g. from a later
session that lost the password — just resets that one account's password
rather than erroring or duplicating anything. Use a domain name that is
obviously a test fixture (`agent-*`), never the operator's own
(`example.com`, `alice@example.com`).

## 3a. (optional) Enable the send-email test

`send_email_delivers_to_a_second_account_on_the_real_server` needs a
*second* account distinct from the write-test one above — it sends *to*
it — on the same domain, so delivery stays intra-server and needs no
outbound relay:

```console
$ ./harness/stalwart/stw seed agent-livewrite.net agent2 '<a different fresh password>'
$ export JMAP_LIVE_SERVER_RECIPIENT_USER=agent2@agent-livewrite.net
$ export JMAP_LIVE_SERVER_RECIPIENT_PASSWORD='<that password>'
```

Skipped, not failed, when unset — same shape as step 3.

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
- `address_book_create_then_destroy_round_trips_through_the_real_api` and
  `calendar_create_then_destroy_round_trips_through_the_real_api` —
  `AddressBook/set`/`Calendar/set` create a *collection* (what a user's "New
  Address Book"/"New Calendar" sends, and what the collection backend's
  `create_resource_sync`/`delete_resource_sync` vfuncs issue), confirm the
  new one shows up in
  `AddressBook/get`/`Calendar/get` with the right name, then destroy it and
  confirm it is gone. Unlike the tests below, these do not also check
  `Client::all_changes`: neither type has a `_get` method here that exposes a
  `state` token (`address_books`/`calendars` return only the list), and
  adding one was out of scope for this coverage-only pair.
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
  making, relying on Stalwart auto-provisioning both per account. The
  mailbox, contact, and calendar-event tests additionally check
  `Client::all_changes` (RFC 8620 §5.2's `/changes`, the primitive every EDS
  meta-backend's `get_changes_sync` drives) after each of the three steps:
  the record's id must show up in the right bucket
  (`created`/`updated`/`destroyed`) since the state captured just before
  that step — exercising this client's incremental-sync state tokens and
  pagination against a real server rather than `jmap-mockd`'s own invention
  of them, for `Mailbox/changes`, `ContactCard/changes`, and
  `CalendarEvent/changes`. Skipped, not failed, when
  `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` (step 3) are not set.
  The mailbox test additionally checks `all_changes` since the state
  captured *before the create*, spanning both the create and the rename:
  RFC 8620 §5.2's fold rule says an object created and updated within one
  `/changes` window is reported as created only, and this checks that
  against Stalwart's own `/changes` rather than just the mock (the client
  itself already folds pages this way via `ChangeSet::classify`).
  The contact test additionally checks `Client::contact_query` right after
  the create and right after the destroy, filtered to the card's address
  book (`ContactCardQueryFilter::in_address_book`) — the exact call
  `jmap-book-sync::list_existing_sync` makes to enumerate an address book,
  which had no live-server coverage from the `get`/`set`/`changes` checks
  alone. The calendar test makes the same check with `Client::event_query`
  and `CalendarEventQueryFilter::in_calendar`, mirroring
  `jmap-cal-sync::list_existing_sync`'s equivalent call to enumerate a
  calendar.
- `a_recurring_event_created_with_the_singular_recurrence_rule_round_trips_through_the_real_api`
  — creates a `CalendarEvent` whose `recurrenceRule` is set (a daily rule,
  three occurrences), confirms it via `CalendarEvent/get`, then destroys it.
  This property's wire shape changed from RFC 8984's plural `recurrenceRules`
  array to jscalendarbis §3.3.3's singular `recurrenceRule` object; a server
  that still expected the old array would reject this create with
  `invalidProperties`, exactly the failure mode this format change guards
  against.
  Narrower than the plain create/update/destroy test above (no update, no
  `all_changes`/`query` checks) — its only job is proving the property shape
  itself against a real, independent JSCalendar implementation, which
  nothing else in this suite exercises.
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
  subject survived. Also checks `Client::all_changes` for `Email` after each
  of import/update/destroy, same as the other three tests — via
  `Client::email_state` rather than a state field on `email_get`'s own
  response, since `email_get` splits large id lists across several
  `Email/get` calls and keeps only their `list`s, not a `state`. Additionally
  checks `Client::email_query` right after the import and right after the
  destroy, filtered to the Inbox and sorted `receivedAt` ascending
  (`EmailQueryFilter::in_mailbox` + `Comparator::ascending("receivedAt")`) —
  the exact call `jmap-mail-sync::message_ids` makes to enumerate a
  mailbox's messages, mirroring the `contact_query`/`event_query` checks
  above but for mail's own listing path, which had no live-server coverage
  from `get`/`set`/`changes` alone. Skipped under the same condition as the
  other three.
- `send_email_delivers_to_a_second_account_on_the_real_server` — sends a
  message via `Client::send_email` (`Email/set` + `EmailSubmission/set`,
  chained) from the write-test account to the second account step 3a seeds,
  then polls the recipient's `Email/query` (filtered by the message's unique
  subject, in its Inbox) until the message shows up, confirming actual
  delivery rather than only an accepted submission. Along the way this is
  also the test that first caught a real client bug: Stalwart's created
  `EmailSubmission` omits `identityId`/`emailId` (RFC 8620 §5.3 permits this
  — the client supplied both itself, so neither is server-set), which
  `Client::send_email`/`submit_email` now backfill from what they sent
  before deserializing (`jmap-client/src/mail.rs`'s
  `backfill_submission_created`, regression-tested against `jmap-mockd`'s
  new `MockServerBuilder::terse_submission_create` in
  `jmap-client/tests/mail_send.rs`). Skipped, not failed, when
  `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` (step 3a) are not set.

Anything short of that is a finding, not a nuisance — report it as you
would any other bug in this project.

## `jmap-cal-sync`'s free/busy test

`rust/crates/jmap-cal-sync/tests/live_server.rs` is a second, separate
live-server file, in the crate that actually implements
`ECalBackendSync::get_free_busy_sync` — `jmap-client`'s own file proves the
wire calls (`Calendar/set`, `CalendarEvent/set`) round-trip, but nothing
there drives `CalSync::free_busy` itself (`Principal/query` by email, then
`Principal/getAvailability`, then `jmap_ical::busy_periods_to_vfreebusy`'s
marshalling) — the actual decision the meeting-scheduler vfunc makes.

It reads the *same* `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD`/
`_REBASE_URLS` variables step 2/3 above already set up — no new credentials
needed if you already enabled the write-path tests — but any account with
the calendars capability works, since this test only asks about its *own*
address (every real account has a `Principal` of its own, so there is
nothing second to provision the way step 3a's recipient account is). Run it
with:

```console
$ cargo test -p evolution-jmap-cal-sync -- --ignored
```

No `--features live-server` gate: unlike `jmap-client`, this crate defines
no such feature — `#[ignore]` alone already keeps it out of a plain `cargo
test`, the same mechanism that gates `jmap-client`'s own file.

`free_busy_of_the_calendar_owner_reflects_a_real_event_against_the_real_server`
creates a one-hour event on the account's default calendar, asks
`free_busy` about the account's own address (plus a second, nonexistent
address in the same call, confirming it is silently absent — the "invited
someone outside the organisation" case, per
`jmap-cal-sync/tests/freebusy.rs`'s equivalent mock test), confirms the
answer's `VFREEBUSY` names the account and carries a
`FREEBUSY;FBTYPE=BUSY:` line for exactly the event's window (`CalendarEvent::
simple` anchors the event to `Etc/UTC`, so the digits are expected to match
exactly, the same bar the mock-based tests hold to — not merely "some busy
period exists"), then destroys the event. Skipped, not failed, when
`JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are unset.

## `jmap-book-sync`'s save/remove test

`rust/crates/jmap-book-sync/tests/live_server.rs` is the address-book
counterpart of the free/busy file above, in the crate that actually
implements `EBookMetaBackendSync::save_contact_sync`/`remove_contact_sync` —
`jmap-client/tests/live_server.rs` already proves `ContactCard/set` round-trips
through `Client` directly, but nothing there drives `BookSync::save_contact`
itself: the vCard-to-`ContactCard` mapping and the create/update decision.

It reads the same `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD`/
`_REBASE_URLS` variables step 2/3 above already set up. Run it with:

```console
$ cargo test -p evolution-jmap-book-sync -- --ignored
```

No `--features live-server` gate, for the same reason as `jmap-cal-sync`'s file.

`saving_then_removing_a_contact_round_trips_through_the_real_server` saves a
new vCard via `BookSync::save_contact`, confirms it via `list_existing`,
edits it (a name change, mirroring an Evolution contact rename), confirms the
edit via `load_contact`, then removes it via `remove_contact` and confirms
it is gone. Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/
`_PASSWORD` are unset.

While building this test against real Stalwart, it also caught a real client
bug (fixed, not just found): `ContactCard/set`'s `created` response held only
`id`, omitting every property the client had just sent (RFC 8620 §5.3
permits this — none of those properties is server-set). `BookSync::
save_contact`'s create branch rendered its return value straight from that
terse object, so the vCard handed back to EDS immediately after a save was
missing the name and everything else just written. Fixed by rendering from a
fresh `ContactCard/get` after create, the same way the update branch already
did — see `jmap-book-sync/tests/terse_create.rs` for the headless,
`jmap-mockd`-reproducible regression test (`MockServerBuilder::
terse_contact_create`).

## `jmap-collection-sync`'s create/delete test

`rust/crates/jmap-collection-sync/tests/live_server.rs` is the collection
counterpart of the two files above, in the crate that actually implements
`ECollectionBackendClass::create_resource_sync`/`delete_resource_sync` —
`jmap-client/tests/live_server.rs` already proves `AddressBook/set` and
`Calendar/set` round-trip through `Client` directly, but nothing there drives
this crate's own `create_collection`/`delete_collection`: the account
resolution through `CollectionLayout`, the create/destroy dispatch by
`ChildKind`, and the `Child` a create derives from what the server answered.

It reads the same `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD`/
`_REBASE_URLS` variables step 2/3 above already set up. Run it with:

```console
$ cargo test -p evolution-jmap-collection-sync -- --ignored
```

No `--features live-server` gate, for the same reason as the two files above.

`creating_then_deleting_a_collection_round_trips_through_the_real_server`
creates an address book and a calendar via `create_collection`, confirms
each is listed by a fresh `Fanout::discover`, then destroys both via
`delete_collection` and confirms neither is listed anymore. Skipped, not
failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are unset.

While building this test against real Stalwart, it also caught a real client
bug (fixed, not just found): a freshly created `AddressBook`/`Calendar` came
back `isSubscribed: false` from Stalwart when the create did not say
otherwise, and `jmap-collection-sync`'s own discovery deliberately drops
`isSubscribed == Some(false)` (the same rule that keeps a collection the
user unsubscribed from out of the sidebar). So a collection Evolution's "New
Address Book"/"New Calendar" had just created would immediately vanish from
the next discovery — this test's first run against real Stalwart failed
exactly that way. `jmap-mockd` never caught it because it always seeds its
address books/calendars `isSubscribed: Some(true)` and a create through it
that names no `isSubscribed` is stored as `None`, which the discovery filter
also treats as subscribed — a real divergence between the mock's and a real
server's default for an unspecified property. Fixed: `create_collection` now
asks for `isSubscribed: true` explicitly on both the `AddressBook` and
`Calendar` create, rather than relying on the server's silence. See
`jmap-collection-sync/tests/create.rs`'s
`a_created_collection_is_discoverable_even_when_the_server_defaults_new_collections_to_unsubscribed`
for the headless, `jmap-mockd`-reproducible regression test
(`MockServerBuilder::new_collections_default_unsubscribed`).

## `jmap-mail-sync`'s import/expunge test

`rust/crates/jmap-mail-sync/tests/live_server.rs` is the mail counterpart of
the three files above, in the crate that actually implements
`append_message_sync`/`expunge_sync` (`MailSync::import_message`/
`expunge_message`) — `jmap-client/tests/live_server.rs` already proves
`Email/import` and `Mailbox/set` round-trip through `Client` directly, but
nothing there drives this crate's own `import_message`/`expunge_message`:
the upload-then-import sequencing, and `expunge_message`'s
read-before-write mailbox-membership decision.

It reads the same `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD`/
`_REBASE_URLS` variables step 2/3 above already set up. Run it with:

```console
$ cargo test -p evolution-jmap-mail-sync -- --ignored
```

No `--features live-server` gate, for the same reason as the three files
above.

`importing_then_expunging_a_message_round_trips_through_the_real_server`
imports a message into the write-test account's Inbox via
`MailSync::import_message`, confirms it via `MailSync::messages`, then
expunges it via `MailSync::expunge_message` and confirms it is gone.
Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
unset.

## `jmap-mail-sync`'s create/delete-folder test

`rust/crates/jmap-mail-sync/tests/live_server_folder.rs` is the
folder-management counterpart of the import/expunge test above, in the same
crate: it exercises `create_folder_sync`/`delete_folder_sync`
(`MailSync::create_folder`/`delete_folder`), a materially different code
path (`Mailbox/set` create/destroy, not `Email/import`/`Email/set`). Only
`jmap-mockd` had exercised these before (`jmap-mail-sync/tests/{create,
delete}_folder.rs`).

Same environment variables as the import/expunge test. Run it with:

```console
$ cargo test -p evolution-jmap-mail-sync --test live_server_folder -- --ignored
```

No `--features live-server` gate, for the same reason as the other files.

`creating_then_deleting_a_folder_round_trips_through_the_real_server`
creates a top-level folder via `MailSync::create_folder`, confirms it via
`folder_tree`, then deletes it via `MailSync::delete_folder` and confirms it
is gone. Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD`
are unset.

## `jmap-mail-sync`'s send test

`rust/crates/jmap-mail-sync/tests/live_server_send.rs` is the
`MailSync::send_message` counterpart of `jmap-client/tests/
live_server.rs::send_email_delivers_to_a_second_account_on_the_real_server`:
that test already proves `Client::send_email` (`Email/set` +
`EmailSubmission/set`, chained) delivers between two real accounts, but
nothing had driven `MailSync::send_message` itself — the upload-then-stage-
then-submit sequencing through `import_message`, `MailSync::identity_for`'s
address-to-identity lookup, and `MailSync::outgoing_mailboxes`'s Drafts/Sent
staging decision. Only `jmap-mockd` had exercised `send_message` before
(`jmap-mail-sync/tests/send.rs`).

It needs both the write-test account (step 3) and the recipient account
(step 3a) — the same pair `send_email_delivers_to_a_second_account_on_the_
real_server` uses. Run it with:

```console
$ cargo test -p evolution-jmap-mail-sync --test live_server_send -- --ignored
```

No `--features live-server` gate, for the same reason as the other files.

`sending_a_message_delivers_to_a_second_account_on_the_real_server` builds an
`Outgoing` from `MailSync::identity_for`'s lookup of the write-test account's
own address and `MailSync::outgoing_mailboxes`'s Drafts/Sent decision, sends
it via `MailSync::send_message`, then polls the recipient's own
`MailSync::messages` (not raw `Client::email_query`, to keep the assertion on
this crate's own read path) until the message actually lands in its Inbox —
proof of delivery, not merely that the server accepted the submission.
Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` or
`JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` are unset.

## `jmap-cal-sync`'s save/remove test

`rust/crates/jmap-cal-sync/tests/live_server_save.rs` is the calendar
counterpart of `jmap-book-sync`'s save/remove test above, in the crate that
actually implements `ECalMetaBackend::save_component_sync`/
`remove_component_sync` — `jmap-client/tests/live_server.rs` already proves
`CalendarEvent/set` round-trips through `Client` directly, and this crate's
own `live_server.rs` already proves `CalSync::free_busy` (a read-side
decision), but nothing had driven `CalSync::save_component`/
`remove_component` themselves: the iCalendar-to-`CalendarEvent` mapping and
the create/update decision `save_component` makes.

It reads the same `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD`/
`_REBASE_URLS` variables step 2/3 above already set up. Run it with:

```console
$ cargo test -p evolution-jmap-cal-sync --test live_server_save -- --ignored
```

No `--features live-server` gate, for the same reason as the other files.

`saving_then_removing_an_event_round_trips_through_the_real_server` saves a
new iCalendar VEVENT via `CalSync::save_component`, confirms it via
`list_existing`, edits it (a summary change, mirroring an Evolution
appointment rename) via `save_component` with `existing_uid`, confirms the
edit via `load_component`, then removes it via `remove_component` and
confirms it is gone. Skipped, not failed, when
`JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are unset.

While building this test against real Stalwart, it also caught a real client
bug (fixed, not just found): `CalendarEvent/set`'s `created` response held
only `id`, omitting every property the client had just sent (RFC 8620 §5.3
permits this — none of those properties is server-set), the same class of
bug `jmap-book-sync`'s save/remove test found for contacts.
`CalSync::save_component`'s create branch rendered its return value straight
from that terse object, so the iCalendar object handed back to EDS
immediately after a save was missing the summary, start time, and everything
else just written. Fixed by rendering from a fresh `load_component` after
create, the same way the update branch already did — see
`jmap-cal-sync/tests/terse_create.rs` for the headless, `jmap-mockd`-
reproducible regression test (`MockServerBuilder::
terse_calendar_event_create`).

## `jmap-mail-sync`'s keywords test

`rust/crates/jmap-mail-sync/tests/live_server_keywords.rs` is the
`MailSync::set_keywords` counterpart of the import/expunge and
create/delete-folder tests above, in the same crate: it exercises the
function `CamelFolder::set_message_flags` actually calls — the single
most-executed write in an ordinary mail client's life (mark read, star,
flag) — which only `jmap-mockd` had exercised before
(`jmap-mail-sync/tests/keywords.rs`).

Same environment variables as the import/expunge test. Run it with:

```console
$ cargo test -p evolution-jmap-mail-sync --test live_server_keywords -- --ignored
```

No `--features live-server` gate, for the same reason as the other files.

`setting_keywords_on_a_message_reaches_the_real_server` imports a message
into the write-test account's Inbox, flags it via `MailSync::set_keywords`,
confirms the flag via `MailSync::messages`' returned
`MessageSummary::flags`, then in a second patch clears `flagged` and sets
`seen` in the same call and confirms both — proving the diff names both a
removal and an addition correctly, not just whichever came first. Cleans up
via `MailSync::expunge_message`. Skipped, not failed, when
`JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are unset.

## `jmap-mail-sync`'s filing test

`rust/crates/jmap-mail-sync/tests/live_server_filing.rs` is the
`MailSync::file_message` counterpart of the tests above, in the same crate:
it exercises the function `transfer_messages_to_sync` actually calls (a
copy or a move between mailboxes), which only `jmap-mockd` had exercised
before (`jmap-mail-sync/tests/mailboxes.rs`, `tests/updates.rs`).

Same environment variables as the import/expunge test. Run it with:

```console
$ cargo test -p evolution-jmap-mail-sync --test live_server_filing -- --ignored
```

No `--features live-server` gate, for the same reason as the other files.

`filing_a_message_into_another_folder_reaches_the_real_server` imports a
message into the write-test account's Inbox, creates a second folder via
`MailSync::create_folder`, copies the message into it via
`Filing::copied_into` and confirms `MailSync::messages` lists it in both
mailboxes, then moves it out of the Inbox into the new folder via
`Filing::moved` in one patch and confirms the Inbox no longer lists it while
the new folder still does. Cleans up via `MailSync::expunge_message` and
`MailSync::delete_folder`. Skipped, not failed, when
`JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are unset.

## `jmap-mail-sync`'s folder-settings test

`rust/crates/jmap-mail-sync/tests/live_server_folder_settings.rs` covers the
two `jmap-mail-sync` writes none of the tests above touch:
`MailSync::set_subscribed` (the write behind `CamelSubscribable`'s two
vfuncs) and `MailSync::rename_folder` (`CamelStore::rename_folder_sync`).
Only `jmap-mockd` had exercised either before
(`jmap-mail-sync/tests/{subscribe,rename_folder}.rs`).

Same environment variables as the import/expunge test. Run it with:

```console
$ cargo test -p evolution-jmap-mail-sync --test live_server_folder_settings -- --ignored
```

No `--features live-server` gate, for the same reason as the other files.

`subscribing_and_renaming_a_folder_reach_the_real_server` creates a
top-level folder, unsubscribes it and confirms via a fresh `folder_tree`
listing, resubscribes it and confirms that too, renames it and confirms
both the returned Camel path and the listing's new display name, then
deletes it. Skipped, not failed, when
`JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are unset.
