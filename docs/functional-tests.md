# Functional tests: real EDS daemons against the mock

Every other test in this repository stops at the edge of Evolution Data
Server. It calls a vfunc body directly, or holds a mapping against a fixture.
That is most of the value for most of the cost — but it leaves one layer
untested, and it is the layer a user meets first: EDS deciding *when* to call
those vfuncs, and what it makes of what they said.

These tests cover that layer. They start a real `evolution-source-registry`
and a real host for the module under test — `evolution-addressbook-factory`,
`evolution-calendar-factory`, or, for the Camel mail provider, the client
program itself — hand it a real build of this repository's modules, and drive
the result through EDS's ordinary client API: the same calls Evolution makes.
The server on the other side is the in-repo mock, so no account anywhere is
involved.

This is M9 layer 1 of `docs/ROADMAP.md`. The GUI tier is a separate thing and
is not here.

## Running them

They are off by default; see "Why they are gated" below.

```console
$ cmake -S . -B build -DENABLE_FUNCTIONAL_TESTS=ON
$ cmake --build build
$ ctest --test-dir build -L functional --output-on-failure
```

`-L functional` selects exactly these; a plain `ctest` in a build configured
this way runs them alongside everything else, which is also fine — they take
under a second.

Nothing is installed. Nothing needs `sudo`. Nothing touches the Evolution data
of whoever runs them.

## What you need installed

- The EDS **runtime**, not just the development headers every other target
  here builds against: `evolution-source-registry`,
  `evolution-addressbook-factory` and `evolution-calendar-factory`. On Debian
  and Ubuntu that is the `evolution-data-server` package; the `-dev` packages
  do not carry the daemons.
- `dbus-run-session`, from `dbus-daemon`.

Configuring with `-DENABLE_FUNCTIONAL_TESTS=ON` without these is a configure
error naming the missing one. That is deliberate: see below.

## How one runs

`rust/crates/jmap-functional` is the harness. For each test it:

1. starts a mock JMAP server in-process, on an ephemeral port, and seeds it
   with what the test needs — one address book or one calendar flagged as the
   account default, or a few mailboxes with some mail in them, or the mailbox
   roles and the sending identity a submission needs, or a whole contact card
   for a test about what EDS makes of one it did not write;
2. builds a throwaway EDS installation in a directory under the crate's
   target tmpdir: a scratch `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and
   `XDG_CACHE_HOME`, the `.source` keyfiles that describe the account and name
   the mock's port — one of them, or three where a send is involved — and a
   module directory holding the one module under test, named by
   `EDS_ADDRESS_BOOK_MODULES`, `EDS_CALENDAR_MODULES` or
   `EDS_CAMEL_PROVIDER_DIR`;
3. runs the client program on a **private session bus** from
   `dbus-run-session`, with that environment and nothing inherited. The
   daemons D-Bus activates are this test's daemons, started with this test's
   environment, and they die with the bus when the client exits;
4. holds both ends to what they should have said: what EDS gave the client,
   and what the backend asked the server for.

The client programs live in `tests/functional/`. They are C, and they are
ordinary libebook/libecal/libcamel consumers — that is the surface under
test, and no crate in this repository binds it (`eds-sys` carries what the
backends *implement*). Binding a second FFI surface only to call it from a
test would put a layer of our own making between EDS and the thing being
tested.

Each client prints `key=value` lines and exits non-zero the moment a call
fails. It holds no opinion about what is correct; the Rust side has all of
those.

### Why the clients pass "do not wait for connected"

Both call `e_*_client_connect_sync` with `(guint32) -1`, and then wait for
`ESource:connection-status` themselves, with a main loop, in
`tests/functional/connection-status.c`. That is not belt-and-braces; the
built-in wait cannot work in a program shaped like these:

`ESource` does not apply a connection-status change where it learns of it. It
queues an idle on the source's `GMainContext`, which is whatever was
thread-default when `ESourceRegistry` was constructed — in a synchronous
program, the default context on the main thread.
`e_client_wait_for_connected_sync` then blocks *that thread* on an `EFlag`
until `notify::connection-status` fires. The signal comes from the idle; the
idle needs the context iterated; the only thread that would iterate it is the
one blocked on the flag. So the wait always expires, whatever the backend did.

Evolution never meets this: it has a main loop and does the wait on a worker
thread. But it means a 30-second stall on opening a JMAP address book from a
small synchronous test program is a property of the *program*, not evidence
about the backend — which is exactly how it was read for a while. Running a
main loop is the whole fix, and it is on the client's side of the contract.

## Why they are gated behind an option

The shared CI image has the EDS development headers and neither daemon. A
test registered unconditionally would therefore either fail every CI run, or
— the tempting fix, and the worse one — be written to skip itself when the
runtime is missing, and report green on a machine where it never ran.

`ENABLE_FUNCTIONAL_TESTS` keeps "did not run" distinguishable from "passed".
With it off, the tests do not exist. With it on, a missing runtime is a
configure error. There is no arrangement in which they quietly pass.

The consequence, stated plainly: **CI does not run these today**, so a change
that breaks the EDS-facing behaviour they cover goes green until someone runs
them here. Closing that needs `evolution-data-server` and `dbus-daemon` in
the CI image (`Containerfile.ci`, rebuilt via `ci-image.yml`) and a
`workflow_dispatch`-gated job that configures with the option on — a
maintainer decision, because it grows the image every job pulls.

## What the address book test asserts

`rust/crates/jmap-functional/tests/address-book.rs`, against
`tests/functional/book-client.c`:

- the factory found and loaded `libebookbackendjmap.so`, and matched its
  factory to the keyfile's `BackendName=jmap` — a failure here is a client
  that cannot connect at all;
- **EDS saw the source reach `connected`.** `ESource:connection-status` is
  EDS's own verdict on the connect — what Evolution shows as a connected
  account, and what an EDS client that waits for a backend waits on. The meta
  backend sets it to `connected` only when the backend's `connect_sync`
  returned TRUE, and to `disconnected` when it did not;
- **the opened book is writable.** EDS derives this from what the backend
  said during its connect; a backend that connects happily and never claims
  the book is writable gives an address book Evolution greys out and whose
  every write comes back as "Permission denied". This assertion is why the
  test exists — that was a real bug, invisible to every unit test in the tree
  and to a build that compiled cleanly;
- a contact added through `e_book_client_add_contact_sync` reaches the
  server: the mock recorded a `ContactCard/set`, and the card in its store
  has the name, the email address and the address book it was given;
- the same contact reads back out of EDS with its name and email intact.

The refresh path is deliberately *not* asserted. `EBookMetaBackend` schedules
its refresh rather than running it, so whether a `ContactCard/query` has
happened by the time the test looks is a race; the write is synchronous. A test
that asserted it would be a flake waiting for a slow machine.

### The second book leg: an edit of a card that came from the server

Everything above starts from a contact EDS itself wrote, which bounds what it
can say: a card that went out as a vCard cannot hold anything a vCard cannot
state. The second test (`an_edit_through_eds_keeps_the_name_parts_the_vcard_
flattened`) starts from the other end — a card **seeded into the mock's store
before EDS connects** — and edits it the way a user edits one.

What it is for: a JSContact name may hold several components of one kind (RFC
9553 §2.2.1 states a double-barrelled given name as two `given` components),
while the vCard `N` value has one field per kind. So the emitter joins them, EDS
is handed `N:Oldenburg;Jean Paul;;;`, and on the way back the save path puts the
parts in again by recognising that the field *still reads as its parts joined* —
a string comparison against text that has been through real EDS. Nothing below
this file can measure that: if EDS normalised the whitespace in the field, a
name nobody touched would read as retyped, and both halves would be replaced by
the field's text along with the pronunciations only the server holds.

The leg therefore asserts, on a card whose given name is two components each
carrying a `phonetic`:

- EDS hands the given name back as `Jean Paul`, byte for byte what the emitter
  wrote;
- an edit to the **email address** — one field, and not the name — leaves the
  server's `name/components` exactly as they were: both halves, in order, each
  still carrying its `phonetic`;
- and the edit itself reached the server, patched in place.

The contact is fetched by UID in a poll loop. In practice the first try
succeeds: `EBookMetaBackend` answers a get for a contact its cache never heard
of by asking the backend to `load_contact_sync`, so this is the one place the
*read* path is exercised — through the load, not through the refresh the
paragraph above declines to race with. The loop is there because the connect
also schedules that refresh, and a get arriving mid-refresh is the kind of
ordering worth waiting out rather than flaking on.

### The picture: what a meta backend does to a photo behind the backend's back

The contact's photo crosses all eight book legs, and it is the one property where
EDS does something to the data on its own initiative — which is why it needs real
daemons rather than a fixture. Two facts, both measured here against EDS 3.52,
and neither of them anything this repository asks for:

- **A cached photo is a file, not bytes.** `EBookMetaBackend` puts every contact
  it caches through `store_inline_photos`: the picture is written into
  `…/cache/evolution/addressbook/<uid>/PHOTO-<hash>-<n>.png` and the `PHOTO` line
  is rewritten to point at it. So a libebook consumer that writes an inlined
  photo reads back a `file:` URI, and the legs assert the bytes at the end of it
  rather than the shape EDS chose to keep them in. The extension is the only
  place a cached photo still says what it is, so it is asserted too: it comes
  from the media type EDS read off the line.
- **EDS undoes that before a save, and quietly keeps the key.** The backend never
  calls `inline_local_photos`; `EBookMetaBackend` does, before it calls
  `save_contact_sync`, and it rewrites the line's *value* while leaving its
  parameters — the `X-JMAP-KEY` included — in place. That is why an edit to some
  other field does not disturb the picture: the entry is still found by its key.

The consequence is the fourth leg
(`replacing_the_picture_through_eds_patches_the_entry_it_replaces`). When the
*user* picks a new photo, the line is written by `e_contact_set` instead, which
drops the parameters — so the new picture arrives with nothing on it saying which
entry it replaces, and the save has to pair it with the one it replaced
(`rekey_keyless` in `jmap-book-sync`). Remove that pairing and this leg is the
only test in the tree that goes red: the server ends up holding the picture under
a key the reader invented by counting lines, and the one it was filed under
deleted.

### The calendaring addresses: a key that *does* survive being set

The contact's Calendar and Free/Busy fields — `CALURI` and `FBURL`, one
JSContact `calendars` map told apart by a `kind` neither line carries — are the
counter-example to the picture above, and the fifth leg
(`retyping_the_calendar_address_through_eds_patches_the_entry_it_replaces`) is
where that is measured rather than assumed.

Both are plain vCard attributes rather than synthetic fields, so
`e_contact_set` on either rewrites the *value* of the first line of that name in
place and leaves its parameters — the `X-JMAP-KEY` among them — where they were.
A picture the user replaces loses its key and has to be paired; a calendar
address the user retypes keeps its key and is simply patched. Same user action,
opposite requirement, and only real EDS says which is which.

What the leg asserts, on the seeded card with two calendaring resources the
server filed under `calendar-1` and `freebusy-1`:

- EDS reads each URI back out of the field its `kind` chose, which is the only
  check that the two cannot have swapped on the way — a `freeBusy` URI shown to
  the user as their calendar is a plausible failure that no single-field
  assertion would see;
- retyping the Calendar field patches `calendars/calendar-1/uri` and nothing
  else: the entry keeps its key and keeps the `pref` no line can carry, so the
  save patched rather than replaced;
- the free/busy address beside it — a second line, of a different name, that
  nobody touched — is untouched on the server and unchanged in EDS's cache.

The first book leg covers the other direction: a contact written through
`e_book_client_add_contact_sync` with both fields set reaches the server as two
`calendars` entries of kind `calendar` and `freeBusy`. Drop the `X-JMAP-KEY`
from the emitter's two lines and six of the seven seeded legs go red together —
every one that asserts the map survived an edit elsewhere — with the server's
entries deleted and re-added under the `c1`/`c2` the reader invents by counting
lines.

### The spouse: the one property whose *key* is what the user sees

Every other mapped property crosses as a value under a key one side or the other
chose. `X-EVOLUTION-SPOUSE` — vCard 3.0 has no `RELATED` — crosses as the **key**
of a JSContact `relatedTo` entry, because RFC 9553 §2.1.8 keys that map by the
related entity itself and RFC 9555 §2.9.5 is what lets the key be free text
rather than a `uid`. So the name on the line *is* the entry's name, and retyping
the field is not an edit to an entry: it is a marriage withdrawn from one entity
and claimed of another. The sixth leg
(`retyping_the_spouse_through_eds_moves_the_marriage_to_the_name_typed`) is where
that meets real daemons.

What only real EDS can answer here is a *cardinality*, and it is invisible at the
client end: `e_contact_get` hands back the first `X-EVOLUTION-SPOUSE` line's
value, so a set that appended a second line rather than rewriting the first reads
back correctly and reaches the server as a card stating **two** marriages — which
the mapping cannot tell from a card that really does state two, since the lines
are all it has. The assertion that catches it is on the server's side of the
path: exactly two `relatedTo` entries after the save.

What the leg asserts, on the seeded card the server relates to two people — a
spouse and a brother:

- EDS reads the spouse off the line the emitter wrote, which is the key-to-value
  crossing checked against real EDS rather than against a fixture;
- retyping the field leaves the server holding the marriage under the name the
  user typed and nothing under the name they stopped typing;
- the **brother** — related as `sibling`, a type Evolution has no field for, so an
  entry that reaches no line and the user could not have edited — is untouched.
  That is what says the save withdrew one marriage rather than writing back
  whatever the vCard could state.

The first book leg covers the other direction: a contact written through
`e_book_client_add_contact_sync` with the Spouse field set reaches the server as
one `relatedTo` entry, keyed by the name and stating `spouse`. The other five
seeded legs assert the whole `relatedTo` map is untouched by an edit elsewhere,
so dropping the spouse line from the emitter turns all eight legs red at once.

### Clearing that field: the withdrawal with nothing to put in its place

The seventh leg
(`clearing_the_spouse_through_eds_withdraws_the_marriage_and_keeps_the_brother`)
is the other half of the sixth, and it reaches the one branch of the save no
other leg can: an edited card stating **no** relations at all. Every leg above
hands the save a `relatedTo` holding something, so a withdrawal with nothing
claimed in its place had only ever been driven against fixtures.

It also settles a question those fixtures could only assume. What EDS does to
the line when the field is emptied was inferred by analogy with another field;
here the client reports it, and against libebook-contacts 3.52 the answer is
that `e_contact_set (contact, E_CONTACT_SPOUSE, "")` — the empty string, which is
what Evolution's contact editor hands over for an entry the user emptied — leaves
the attribute in place holding no value, while `NULL` removes it outright. The
mapping withdraws the marriage either way, because the reader refuses a line
naming nobody, and the leg passes against both cards; so the observation is
*printed* and the assertion is the version-robust one — whatever became of the
line, the field Evolution shows must read empty.

What the leg asserts, on the same seeded card:

- the marriage the card started from was read by EDS, so the clearing is of
  something that was there;
- the server ends up relating the card to the **brother alone** — the marriage
  withdrawn, and the property still present rather than taken back whole. Drop
  the guard in `diff_related_to` that distinguishes "every entry was a marriage"
  from "one of them was" (`withdrawn.len() == current.len()`) and the save
  answers an emptied field with `relatedTo: null`, deleting a relation of a type
  no field shows: this leg goes red, and so does the fixture test that models
  the same edit (`clearing_the_spouse_field_keeps_a_relation_the_line_never_showed`
  in `jmap-book-sync`). What the leg adds over that fixture is the input — the
  fixture *states* the card a cleared field produces, and only the daemons can
  say EDS produces it.

### The notes: the map the user is shown only part of

Evolution's Notes field is `E_CONTACT_NOTE`, which is the **first** `NOTE` line
and nothing else — while every JSContact `notes` entry writes a line of its own.
A card the server filed two notes on therefore shows the user one note, with the
other sitting behind it and nothing in the UI saying it is there. That is the one
mapped property where the user edits part of a map they cannot see the whole of,
and the eighth leg
(`retyping_the_note_through_eds_patches_the_entry_it_replaces`) is where what
happens to the hidden part is measured rather than assumed.

Measured against libebook-contacts 3.52, `e_contact_set` on a plain vCard
attribute rewrites the **value of the first line of that name in place**: its
parameters stay — the `X-JMAP-KEY` among them, quoted on the way out because the
key holds a hyphen — and every further line of the same name is left standing.
So the calendaring addresses' behaviour, not the picture's: the retyped note is
patched by key, and the note behind it is neither rewritten nor deleted. The two
failures that claim would hide are both invisible at the client end, since
`e_contact_get` reads the first line whatever else is on the card, so the leg
reports a **count** beside the field and the harness holds it to a number.

What the leg asserts, on the seeded card the server filed two notes on under
`note-1` and `note-2`:

- EDS read the first note off the first line the emitter wrote, and kept both
  lines through its cache — `read-note-lines=2`;
- the set left two lines standing — `retyped-note-lines=2`. A `1` is
  `e_contact_set` having replaced every line of the name, so a user editing their
  note in Evolution deletes one they were never shown; a `3` is it having
  appended beside the old line, which reaches the server as a note nobody typed;
- the server ends up with two entries under the **two keys it chose**, the edited
  one holding the new text and still carrying the `created` no `NOTE` line can
  express — so the save patched `notes/note-1/note` rather than replacing the
  entry — and `note-2` untouched.

The other seven legs assert the whole `notes` map is untouched by an edit
elsewhere, so dropping the `X-JMAP-KEY` from the emitter's `NOTE` line turns all
seven seeded legs red at once, with the server's notes deleted and re-added under
the `n1`/`n2` the reader invents by counting lines.

What this leg does **not** add is a mutation only it catches: stop `diff_notes`
patching the text and it goes red, but so does the fixture test that models the
same edit (`editing_a_note_keeps_when_it_was_written_and_by_whom` in
`jmap-book-sync`). What it adds is the *input* — the fixture states the card a
retyped Notes field produces, and only the daemons can say that EDS produces it.

## What the calendar test asserts

`rust/crates/jmap-functional/tests/calendar.rs`, against
`tests/functional/cal-client.c`, is the same test one collection over, and it
was written because the two backends are mirrors of each other — which is
exactly the shape in which one of them carries a bug the other's tests would
have caught. It did: `jmap-backend-cal` had the address book's writable bug,
line for line, and this test is what found it.

- the factory found and loaded `libecalbackendjmap.so` and matched it to
  `BackendName=jmap`;
- **EDS saw the source reach `connected`**, which carries more here than it
  does for the address book: `e_cal_client_connect_sync` succeeds even when the
  backend's `connect_sync` failed — `ECalMetaBackend` opens the calendar and
  schedules the connect — so this is the one observation that tells a calendar
  the backend could not open from one it opened and forgot to claim writable.
  It is therefore asserted *before* the writable check, whose net covers both;
- **the opened calendar is writable**, for the reason above;
- an event added through `e_cal_client_create_object_sync` reaches the server:
  the mock recorded a `CalendarEvent/set`, and the event in its store has the
  summary, the start time and the calendar it was given;
- the same event reads back out of EDS with its summary intact;
- an all-day event — `VALUE=DATE` on both ends — reaches the server as
  JSCalendar's `showWithoutTime`, a day long and with no zone, rather than as a
  midnight appointment;
- and all three things Evolution's "this occurrence / this and future
  occurrences / all occurrences" menu does to a weekly series, because
  `ECalMetaBackend` translates each of them itself and hands the backend
  something different every time: "Edit this occurrence" arrives as a detached
  instance, "Delete this occurrence" as a *save* of the master carrying one more
  `EXDATE` — never as a removal — and "this and future occurrences" as a
  truncated master **plus a second event** under a UID EDS invents, which is the
  only one of the three that reaches the backend as two writes. Each is checked
  at both ends: what EDS kept in its own cache, and what the server was told.
  That series names the day it repeats on — `BYDAY=TH`, what the recurrence page
  writes for anything but "every day" — so both ends also say whether the
  `byDay` of RFC 8984 §4.3.3 survives the trip, including through the split,
  which rewrites the rule;
- and the two events in named zones, which are the only cases here whose
  components are built through the libical *setters* rather than from text. That
  is the point of them: what a setter writes for a builtin zone is libical's own
  identifier — `/freeassociation.sourceforge.net/Europe/Berlin` — which no JMAP
  server resolves, so the zone reaches the server only if the envelope the
  backend builds also carries the `VTIMEZONE` defining it. No test below real EDS
  can say whether it does, because they all supply the identifier by hand. One is
  a plain appointment; the other is a **series in one zone with a single
  occurrence moved into another**, which is the case where two zones have to
  travel in one object and where the `RECURRENCE-ID` stays on the series' clock
  (RFC 5545 §3.8.4.4) while the instance's own `DTSTART` does not.

The read path is left alone for the same reason as the address book's.

## What the mail test asserts

`rust/crates/jmap-functional/tests/mail.rs`, against
`tests/functional/mail-client.c`, and it is not a third mirror of the other
two — the loading story is different, and that difference is most of why it is
worth having.

An address book or calendar backend is dlopened by a factory daemon EDS ships,
found by being a file in a directory that daemon scans. A Camel provider is
dlopened by the **mail client's own process**, and only when something asks for
a protocol that a `.urls` file in Camel's provider directory claims. So the
client program here is not a consumer talking to a daemon that hosts the module
— it *is* the host. Nothing links the provider in; it is a file in a directory,
found the way Camel finds one, which is a mechanism `jmap-mail`'s own tests
cannot exercise because they link it.

- **`protocol=jmap` and the store connected.** Between them these say that the
  keyfile's `BackendName`, the one line in `libcameljmap.urls` and the string
  `camel_provider_module_init` registers all agree. They live in three files,
  and when they do not agree every later step fails with *No provider available
  for protocol* — a message about the connect that is really about a typo. The
  `.urls` file is staged from the source tree rather than written by the test,
  so this is the installed file being checked and not a copy of it;
- **the folder tree is the mock's three mailboxes**, from a single
  `Mailbox/get`;
- **the inbox is the mailbox with the JMAP `inbox` role**, which is what Camel
  asks the store for and what lets Evolution treat it as the account's inbox
  rather than as a folder that happens to be called one;
- **the summaries are the seeded messages** — `Email/query` and `Email/get`,
  through Camel's own summary machinery;
- **every message body downloads**, which is a different request again: a blob
  download is a plain HTTP GET rather than a method call, and a provider that
  lists mail it cannot open is a common enough failure to be worth fetching all
  of them.

Every list the client reports is sorted before it is printed. The order Camel
hands folders or message uids over is the provider's business, and a test that
compared it as given would be asserting an order nobody promised — which it did
once, before this was written down.

Sending is not covered here. It is the next section.

## What the transport test asserts

`rust/crates/jmap-functional/tests/transport.rs`, against
`tests/functional/transport-client.c`: the send half, which is a second
`CamelService` built from a second `ESource` out of the same provider's
`object_types` table.

It is a leg of its own rather than three more assertions on the mail one because
of how the transport is *found*. Camel knows nothing about `ESource`, so nothing
in Camel joins a transport to the account it sends for; what joins them is two
hops of uid indirection through a third source:

```
[Mail Account]     IdentityUid=…    ->  the identity source
[Mail Submission]  TransportUid=…   ->  the transport source
```

Evolution walks that chain out of `libedataserver` accessors, and the client
program is handed only the *account* uid and walks the same one. Every link is a
string in a file, and a broken link is the quietest failure this provider has —
`docs/manual-test-mail-provider.md` says it plainly: the account receives mail
perfectly and fails only when the user presses Send.

- **the chain resolves**: the identity uid off the account, the address off the
  identity, the transport uid off the identity's submission extension, and
  `protocol=jmap` off the *transport's* `BackendName` and not the account's;
- **the transport connected**, which is `object_types[CAMEL_PROVIDER_TRANSPORT]`
  — a different entry of the same registered struct than the store comes out of.
  A provider that left it `G_TYPE_INVALID` loads, receives mail, and fails only
  here;
- **the message went out through the account's identity**: the mock recorded one
  submission, and its `identityId` is the identity seeded for the address the
  *identity source* named, resolved over the wire by `Identity/get`;
- **the envelope is the two `CamelAddress` lists**, not the headers. The
  envelope is what a message is delivered by, and Evolution fills it in from the
  account and the composer's recipient fields;
- **the sent copy is in Sent, and is no longer a draft.** It was staged in
  Drafts and filed across by the server's own `onSuccessUpdateEmail`, so seeing
  it in Sent is evidence the submission was *accepted* and not merely posted;
- **`out_sent_message_saved` is TRUE.** Camel's one out-parameter besides the
  error, and not decoration: Evolution appends a copy of its own when it is told
  FALSE, so a wrong answer is either two of every sent message or none of them;
- **the requests, and their order**: `Identity/get`, `Mailbox/get`,
  `Email/import`, and `EmailSubmission/set` last. The blob upload before the
  import is a plain HTTP PUT rather than a method call, so it is not in that
  list — the import naming a blob is what says it happened.

A second test makes the recipe's mistake on purpose: the same three files with
the transport's `[Authentication]` group deleted. The chain still resolves — this
is a source that was found and that names no server — and the send fails at the
*connect*, with `the account does not name a JMAP server`, having made not one
request. Nothing was imported, so there is no draft left behind for a send that
never happened. That is the difference between a mistake a user fixes by adding
a line to a keyfile and one they fix by also deleting a message they did not
write.

## Debugging a failure

The failure message carries the client's whole stdout and stderr, including
D-Bus activation lines, which is usually enough to tell "the module was never
loaded" from "the module was loaded and misbehaved".

The scratch tree is left behind — `rust/target/tmp/<test name>/` — and
wiped at the *start* of the next run, not the end, so it can be looked at
afterwards. It holds the keyfile the test wrote, the module it staged, and
the meta backend's cache database as EDS left it.

For more from the daemons, add `G_MESSAGES_DEBUG=all` to the session's
environment in `rust/crates/jmap-functional/src/lib.rs` and run the test with
`--nocapture`.

## Related

- `docs/manual-test-book-backend.md`, `docs/manual-test-cal-backend.md` and
  `docs/manual-test-mail-provider.md` — the same paths by hand, in a real
  Evolution, with a real Contacts, Calendar or mail view to look at. These
  tests do not replace them: they check the machinery, those check that a
  person can use it.
