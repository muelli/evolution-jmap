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
   or calendar event for a test about what EDS makes of one it did not write;
2. builds a throwaway EDS installation in a directory under the crate's
   target tmpdir: a scratch `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and
   `XDG_CACHE_HOME`, the `.source` keyfiles that describe the account and name
   the mock's port — one of them, or three where a send is involved — and a
   module directory holding the one module under test, named by
   `EDS_ADDRESS_BOOK_MODULES`, `EDS_CALENDAR_MODULES`,
   `EDS_CAMEL_PROVIDER_DIR` or `EDS_REGISTRY_MODULES`;
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

`ci.yml`'s ordinary jobs (`checks`, `build`, `reproducible`) run on a bare
`ubuntu-24.04` runner and install only the EDS **development** headers
(`ci/install-deps.sh`) — neither daemon. A test registered unconditionally
would therefore either fail every run, or — the tempting fix, and the worse
one — be written to skip itself when the runtime is missing, and report
green on a machine where it never ran.

`ENABLE_FUNCTIONAL_TESTS` keeps "did not run" distinguishable from "passed".
With it off, the tests do not exist. With it on, a missing runtime is a
configure error. There is no arrangement in which they quietly pass.

CI does run these, in their own gated `functional` job (`.github/workflows/
ci.yml`): triggered by `workflow_dispatch`, or a pull request labelled
`run-functional-tests`, rather than every push — this layer is slower than
the rest of the suite and, being off by default, is the one place a run is
worth spending deliberately rather than on every commit. The job installs
`evolution-data-server` and `dbus-daemon` itself (`ci/install-deps-
functional.sh`) directly on the runner, the same way `ci/install-deps.sh`
installs the dev headers for the other jobs — it does not touch the shared
CI image (`Containerfile.ci` / `ci-image.yml`), which `release.yml` alone
uses, so adding this job does not grow what every other job pulls. `ci/
functional.sh` is the configure-build-test recipe both the job and a human
running the same thing locally share.

Not wired into `.gitlab-ci.yml`: that runner's default image is Debian's
`rust:1.97.1`, not Ubuntu, and this session did not have a GitLab runner to
verify `apt-get install evolution-data-server dbus-daemon` behaves the same
there. Left for a session that can check it rather than guessed at.

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

The contact's photo crosses all eleven book legs, and it is the one property where
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
from the emitter's two lines and nine of the ten seeded legs go red together —
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
one `relatedTo` entry, keyed by the name and stating `spouse`. The other eight
seeded legs assert the whole `relatedTo` map is untouched by an edit elsewhere,
so dropping the spouse line from the emitter turns all eleven legs red at once.

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

The other legs assert the whole `notes` map is untouched by an edit elsewhere, so
dropping the `X-JMAP-KEY` from the emitter's `NOTE` line turns nine of the ten
seeded legs red at once, with the server's notes deleted and re-added under the
`n1`/`n2` the reader invents by counting lines. The one that does not is the
tenth leg below: it clears the only note there is, so the property goes back
whole and there is no key for the save to have got wrong.

What this leg does **not** add is a mutation only it catches: stop `diff_notes`
patching the text and it goes red, but so does the fixture test that models the
same edit (`editing_a_note_keeps_when_it_was_written_and_by_whom` in
`jmap-book-sync`). What it adds is the *input* — the fixture states the card a
retyped Notes field produces, and only the daemons can say that EDS produces it.

### Clearing the Notes field: the note the user was never shown, revealed

The ninth leg
(`clearing_the_note_through_eds_withdraws_it_and_keeps_the_one_behind_it`) is the
other half of the eighth, and it takes the branch of the save no other leg
reaches: an entry **withdrawn** from a keyed map through a field that cannot
express the map. Every leg above hands the save a `notes` holding something for
each entry it knows about.

What EDS leaves on the card when the field is emptied is the measurement it
exists for, and against libebook-contacts 3.52 the answer matches the spouse
line's: `e_contact_set (contact, E_CONTACT_NOTE, "")` — the empty string, which
is what Evolution's contact editor hands over for a field the user emptied —
leaves the line **standing with no value on it** rather than striking it off, so
`cleared-note-lines` reads 2. The mapping withdraws the note either way, because
the reader refuses a `NOTE` saying nothing (`states_note`), so the leg reports the
count and asserts only what is true of both cards.

Neither the field nor that count can say *which* line is which — a card whose
first note was emptied and a card whose second note was deleted read alike
through `e_contact_get`, and both leave one line saying something — so the client
also reports every `NOTE` value on the card, joined on `|`, and the harness
compares the ones that say something.

What the leg asserts, on the same seeded card:

- the note the field showed was read by EDS, off the first of two lines, so the
  clearing is of something that was there;
- the card handed to the save shows an empty field and carries exactly one
  `NOTE` value: the note **behind** the cleared one;
- the server ends up holding `note-2` alone, under the key it chose and with its
  text untouched — the entry withdrawn rather than the property taken back whole,
  and rather than the withdrawal never made. Drop the removal pass in
  `diff_entries` and this leg goes red on its own; so does making `states_note`
  accept a note that says nothing, which would send the emptied line back to the
  server as a note spelled as nothing;
- and, after the save, EDS's cache hands the Notes field back holding
  `do not call before 10:00, ever` — one line, `read-back-note-lines=1`. Clearing
  the field therefore **reveals** the note behind it rather than emptying the
  field, which is surprising and correct: the field is the first `NOTE` line, and
  after the withdrawal that is the second note. It is asserted because both other
  readings are failures — the old text is a withdrawal that never reached the
  server, and an empty field is the second note having gone with the first.

The fixture beside it is `emptying_one_note_line_of_two_withdraws_that_note_alone`
in `jmap-book-sync`, which states the card an emptied field produces; this leg is
what says EDS produces it.

### Clearing the only note: the property that goes back whole

The tenth leg
(`clearing_the_only_note_through_eds_withdraws_the_whole_property`) is the same
edit as the ninth on a card the server filed **one** note on, and that changes
what the save must say. The mapping counts what the card hides before it decides:
with a note behind the cleared one, the entry alone is withdrawn
(`notes/note-1: null`, the ninth leg); with nothing behind it, the property itself
is gone (`notes: null`). Two branches of `diff_entries`, and until this leg the
one below the fold had no test through the daemons at all — drop it and this leg
is the only one of the eleven that goes red.

Measured against libebook-contacts 3.52 on this card, the emptied line behaves as
it does on the other: `e_contact_set (contact, E_CONTACT_NOTE, "")` leaves one
`NOTE` line standing with no value on it — `cleared-note-lines=1`, and every
value the card reports is empty. So the card handed to the save states a note
that says nothing, and the withdrawal is the reader's (`states_note` refuses it).
The leg asserts the values rather than the count, for the reason the ninth does:
which shape EDS leaves behind is libebook-contacts' business, and the note is
withdrawn either way.

What it asserts beyond that, on the one-note card:

- EDS read the note and held exactly one line of it — `read-note-lines=1`. A `2`
  here is the ninth leg's card in front of the daemons instead, which would make
  every assertion below about the wrong branch;
- the server ends up with **no `notes` at all**. An empty map would be the
  property still there saying nothing, which is what the per-entry withdrawal
  produces when there is nothing left to keep;
- and the field really is empty afterwards — `read-back-note-lines=0`. This is
  the counterpart of the ninth leg's surprise: there, clearing the field revealed
  the note behind it, and only a card with nothing behind it can say that the
  reveal was the second note rather than a save that refused the withdrawal.

The fixture beside it is `emptying_the_only_note_line_withdraws_the_property` in
`jmap-book-sync`. It states the same card, and it is a different test from
`removing_the_note_line_removes_the_note` next to it in exactly one way — the
input: that one strikes the `NOTE` line off the card, which is not what EDS does
to a field the user emptied, and this one empties it, which is.

### The URI-only handle: a value the card cannot carry as it stands

The eleventh leg
(`retyping_a_uri_only_handle_through_eds_writes_the_uri_back`) is about the one
mapped property whose *value* the server can state in a form no vCard line can.
RFC 9553 §2.3.2 lets an `onlineServices` entry name the contact with a `user`, a
`uri`, or both; Evolution's instant-messaging fields hold a handle and nothing
else. So the seeded card carries an entry stating **only**
`uri: xmpp:jp@jabber.example`, and it reaches the user at all because `xmpp:` is
RFC 5122's — its path is the JID and nothing besides — which lets the reader draw
the handle out of it. What the save then must do is write the edit back onto the
member it came from: rebuilding the URI, rather than answering a card shaped one
way with a card shaped another.

Three things only real EDS can answer:

- **the drawn handle reaches the field the user looks at.** The reader picks the
  slot — `TYPE=HOME` — and `E_CONTACT_IM_JABBER_HOME_1` is the field that slot
  feeds. Whether the parameter our emitter writes is the one libebook-contacts
  reads a handle back out of is a claim about EDS, and a handle in the wrong
  slot, or on a line with no `TYPE`, is one nobody sees. Measured:
  `read-im-handle=jp@jabber.example`. Point `DEFAULT_SLOT` at `WORK` and this leg
  and the first go red together;
- **what a set on that field leaves on the card.** Measured against
  libebook-contacts 3.52, `e_contact_set` rewrites the value of the first
  `X-JABBER` line in place and keeps its parameters:
  `retyped-im-handle-lines=1`, `retyped-im-handle-key=handle-1`. So the save can
  patch the entry the URI came from by key, rather than having to pair a keyless
  line with it as it does for a replaced photo. Both observations are printed
  rather than asserted — the save reaches the same entry either way — and what is
  asserted is the consequence on the server;
- **the entry survives an edit elsewhere unchanged.** The other nine seeded legs
  assert that (`assert_the_seeded_service_survived`), and the reason it is worth
  asserting is that the line hands back a `user` where the server holds a `uri`:
  an entry rewritten into the shape the line states would be a card answered with
  a card it never was.

What the leg asserts beyond the read: the server ends up with the entry under
**the key it chose**, at the same service, holding `uri: xmpp:jp@xmpp.example`
and no `user` at all. Two mutations redden this leg and nothing else in the
file — writing the retyped handle onto a `user` instead of rebuilding the URI,
and dropping `xmpp` from the reader's table of schemes — but `jmap-book-sync`'s
fixtures catch both, so what the leg adds is the *input*.

What it does **not** catch, measured rather than assumed: a save that compares an
entry's members rather than the handle they both spell. That writes the URI back
with the text it already had, so every assertion here still holds and only the
card's state on the server moves. Catching it takes a before-and-after on that
state, which is `an_edit_that_left_a_uri_only_handle_alone_writes_nothing` in the
fixtures.

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

### The second calendar leg: an event that came from the server

`retyping_a_place_through_eds_patches_the_entry_the_server_chose`, against
`tests/functional/cal-edit-client.c` — a second client program, because the two
ask opposite questions and share no code path worth sharing. Every event in the
leg above is created through EDS, so its `locations` and `virtualLocations` hold
exactly what an iCalendar line can state and a round trip has nothing to lose.
This one starts from an event the *mock* was seeded with before EDS ever
connected, holding entries a line can only draw part of:

- a `locations` entry with a `name` and a `description`,
- a `virtualLocations` entry with a `uri`, a `name` and a `description`, and
- **two** `links` entries, each with an `href`, a `contentType`, a `size` and a
  `title`, and
- a **third** `links` entry carrying the `icon` `rel` and a `display`, which is
  the same map's other half: RFC 8984 §4.2.7 keeps in one `links` map what
  iCalendar splits between RFC 5545 §3.8.1.1's `ATTACH` and RFC 7986 §5.10's
  `IMAGE`, and the `rel` is what tells them apart,

each under a key only a server would choose. The event also **starts in a named
zone** rather than in UTC, which is the one thing here that is not about a map:
`jmap-ical` draws a UTC start as a `DTSTART` ending in `Z`, naming no identifier,
but a named zone goes out as `DTSTART;TZID=Europe/Berlin` with **no `VTIMEZONE`**
defining it — RFC 5545 §3.2.19 has the document define what a `TZID` refers to,
and the mapping leans on the consumer's zone database instead, having none of its
own to build one from. The client waits for the event to become gettable, reports
what EDS gave it, retypes the `LOCATION` (the field Evolution's appointment editor
writes) and the `CONFERENCE` value, re-addresses one of the two `ATTACH` lines and
the `IMAGE` — each named by the address it already carries, which is how a user
picks the resource they meant — and saves.

What it asserts, and why each observation is separate:

- **the drawing arrived** — the place on the `LOCATION` line, the address on the
  `CONFERENCE` line, one of each and no more, the conference's `LABEL`, both
  `ATTACH` lines with the `FMTTYPE` and `SIZE` standing on each, and the one
  `IMAGE` line with its `FMTTYPE` and its `DISPLAY`. The counts carry the split:
  two `ATTACH` and one `IMAGE` says the icon link left on the property it belongs
  on, where a mapping that ignored the `rel` would show three and none;
- **the `X-JMAP-KEY` came back on every line.** For the conference this is
  load-bearing: RFC 7986 §5.11 admits several `CONFERENCE` lines, so the mapping
  finds the server's entry by the key the line carries and by nothing else. For
  the `LOCATION` it is not — RFC 5545 §3.6.1 allows one, so the save finds the
  single entry in the server's own map whatever the line says — and it is
  asserted anyway, so that the day the mapping draws a second place a change in
  what EDS carries fails here rather than corrupting. For the two attachments it
  is load-bearing *and* the loss corrupts rather than fails, which is why there
  are two of them: with one resource a save that lost the key still finds the
  server's only entry, and with two it re-addresses whichever the mapping
  guessed. `ATTACH` also asks the question in the hardest form, since RFC 5545
  §3.8.1.1 gives it a value type of its own — libical parses the line into an
  `icalattach`, so the parameters stand beside a value the library re-made, and
  the client therefore reads the address back through `i_cal_property_get_attach`
  rather than as text. The `IMAGE` asks it in a *third* shape again: §5.10's
  grammar makes `VALUE=URI` REQUIRED on the URI alternative, so the mapping writes
  the parameter, and with it present libical parses the value as a URI rather than
  as the `icalattach` an `ATTACH` gets — `i_cal_property_get_attach` on such a
  property reaches into the union as though it were one and **crashes** (measured
  on libical 3.0.17), which is why the client reads that address as text and
  re-addresses it through `i_cal_property_set_value_from_string`;
- **every edit reached the server as a patch of the entry it was drawn from**:
  `locations/<key>/name`, `virtualLocations/<key>/uri` and `links/<key>/href`,
  with the `description` and the `title` no line had room for still where the
  server put them, and the attachment the user never touched still at the address
  it went out with. A save that named any of the three properties whole passes
  every observation above and fails this one, and what it costs the user is a
  note they never saw; a save that reached the wrong `links` entry moves a
  document nobody asked to move and loses the edit that was made. The
  `contentType` and the `size` are shown on the line and deliberately never
  written back — they are the server's description of what it holds, not a field
  the user was offered. The picture makes the same argument one member further:
  §5.10 admits no `SIZE` on an `IMAGE`, so its `size` was never shown at *all* —
  a save that wrote back every member it could name would keep the `ATTACH`
  entries' sizes and delete this one's — and its `rel` and `display` are the
  server's own too, since the property name is the whole of what the line says
  about them;
- **the entry keys did not move** and the event was not renamed, so the save
  patched the event rather than replacing it with what one component can state;
- **the start means the instant the server means.** Three observations, before the
  save and after it: the wall clock on the `DTSTART` line, the `TZID` verbatim, and
  what a libecal consumer resolves the two to, converted to UTC. The third is the
  one no fixture can make — `jmap-ical`'s and `jmap-cal-sync`'s own tests compare
  text against text, where a zone nobody could resolve reads back exactly like one
  anybody could, so only a real consumer says whether shipping no `VTIMEZONE` costs
  the user anything. It does not: EDS keeps the identifier as the mapping spelled
  it, and libical resolves `Europe/Berlin` out of its builtin table, landing on
  08:00 UTC for a 10:00 CEST start. A start that quietly floated would report
  10:00 UTC — two hours off, and identically on every machine, since libical does
  not adjust a floating time it converts. The pair from the server's end says the
  save did not *restate* the start: `start` and `timeZone` are still what the mock
  was seeded with, so an edit the user made to a picture did not also move the
  appointment for every other client of the account.

Ten mutations have been run against it, each reddening a different assertion:
dropping the `X-JMAP-KEY` from the `CONFERENCE` (the key observation); ignoring
the key when reading the line back, which leaves the drawing intact and the save
unable to name the entry (the read-back address, because EDS's cache hands the
old one over); making the rename replace `locations` rather than patch into it
(the server-side entry, which comes back without its `description`); patching
`contentType` alongside `href` (the server-side `links` entry, with the media
type gone); re-addressing the *first* `ATTACH` instead of the one the user
picked (the read-back lookup, which finds the untouched document moved and the
edited one where it was); replacing `links` whole rather than patching one
`href` (the server-side pair, which comes back with both `title`s gone while
every client observation still passes); dropping the `X-JMAP-KEY` from the
`IMAGE` alone (the read-side picture lookup, which finds no line answering to the
key); ignoring the `icon` `rel` when drawing, so the picture goes out as a third
`ATTACH` — with a `SIZE` §5.10 forbids — and the client, finding no `IMAGE` to
re-address, fails outright; dropping the `DISPLAY` parameter (the picture's
`display`, and nothing else); and patching the whole `links/<key>` entry rather
than its `href` (the server-side map, where both edited entries lose the `title`
and the picture loses its `size` too, while every client observation passes); and
two against the zone — drawing the start with no `TZID` at all, which floats it and
moves the event two hours for the consumer, and drawing the `TZID` in the
solidus-prefixed form libical uses for a builtin zone
(`/freeassociation.sourceforge.net/Europe/Berlin`), which reddens the identifier
observation while the instant stays right, since libical resolves that form too.

The server-side start assertion is the one guard here that no mutation of this
repository can redden, and deliberately so: `jmap_cal_sync::patch` diffs against
the server's own event put through the same round trip, so a reader that misreads a
zone misreads it on both sides and patches nothing. What it guards against is the
platform changing under us — an EDS whose cache normalised a zoned `DTSTART` to
UTC would send a `start` and a `timeZone` nobody edited — and it is read from the
server rather than from the client because EDS answers a get out of its own cache,
where such a patch would be invisible.

Retyping the conference is the one thing here a user of Evolution 3.52 cannot
do: it has no control for the property, and libical-glib 3.0 does not even name
it — the generated `ICalPropertyKind` has no `I_CAL_CONFERENCE_PROPERTY`, so the
client casts the libical C enumerator. That edit is what another client on the
same account does; the mapping has a path for it, and this is what says the path
works through real EDS.

### The third calendar leg: the zone only the server can name

`the_zone_only_the_server_can_name_means_an_instant_a_save_does_not_move`,
against `tests/functional/cal-zone-client.c` — a third client program. The two
legs above ask what reaches the server; this one starts from the other end, with a
question about what reaches the *user*: is the appointment shown at the hour the
server put it at? And then, once that is answered, it saves the appointment twice
and asks it again after each — because a zone that is resolvable until the first
ordinary edit is not resolvable. The two saves are the two kinds of edit there are:
one that has nothing to do with the clock, and one that is about nothing else.

RFC 8984 §1.4.9 lets an event's `timeZone` be either an IANA name or a custom
identifier beginning with a solidus **that the event's own `timeZones` (§4.7.2)
defines** — the second is what a server invents for a zone no database names, such
as the one an Outlook invitation carries its own `VTIMEZONE` for, so it arrives on
any account holding a meeting somebody organised there. The leg seeds two events at
the same wall clock, 10:00 on 2026-04-09, and gives them different zones:

- one in `/example.com/Europe-Berlin`, with a full §4.7.2 definition — both
  observances, the offsets on either side of each, the yearly rule that repeats
  them and a `TZNAME`; and
- one in `/example.com/Somewhere-Else`, with **no** definition at all, which is
  what a server sends when it has a zone it cannot describe.

The tail of the first identifier is `Europe-Berlin` and not `Europe/Berlin` on
purpose: libical looks a location up in its own table, so an identifier whose tail
*was* an IANA location could resolve out of the table and the leg would pass
without the definition being read. Both are seeded straight into the mock rather
than written through EDS, and here that is not a convenience — a create carries a
`timeZones` entry only for a zone somebody handed the calendar (the fourth leg,
below), and what this leg is about is the zone nobody here ever had. A zone only a
server can name reaches this backend only from a server.

For each event the client reports the wall clock on the `DTSTART`, the `TZID`
verbatim, how many `VTIMEZONE`s stand beside the event, and the instant the start
means — asked **twice**, which is what this leg discovered and the reason it is a
program of its own:

- `i_cal_component_get_dtstart`, which resolves a `TZID` the way libical does: the
  enclosing component's own definitions first, then the builtin table. For a custom
  identifier that finds nothing, and libical does not adjust a floating time it
  converts, so the start reports the wall clock with a `Z` stamped on it — **10:00
  UTC, two hours from where the server put it**, identically on every machine.
- `e_cal_client_get_timezone_sync`, which asks the *calendar* for the zone. This is
  where the definition is: `ECalMetaBackend` gathers the `VTIMEZONE`s out of what
  the backend gave it into the calendar's own timezone store, and
  `e_cal_client_get_object_sync` then answers with the component **alone** — the
  observation count is `0`, for both events and for the builtin-zone event of the
  leg above. So a consumer written the obvious way is two hours out, and one that
  asks the calendar lands on **08:00 UTC**, the instant the server means. That is
  why Evolution's recurrence and alarm machinery takes a zone-lookup callback, and
  it is not something this repository chooses.

What it asserts: for the defined zone, that the wall clock and the identifier
arrived unchanged, that the calendar knows the zone and resolves the start to
08:00Z — one assertion covering the whole path, since nothing but a reader of the
definition the event carried can arrive at that number — and, separately, that
libical alone still floats it and that EDS still hands back no `VTIMEZONE`, so a
change in either is visible rather than silent. For the undefined zone, that the
`TZID` is still stated (dropping it would say "no zone at all" and inventing UTC
would say the wrong instant, so the mapping states what it was given), that neither
route can resolve it, and that the start therefore floats to 10:00Z. That last
number is nobody's idea of correct; it is `jmap-ical`'s documented fallback, and
the assertion is what will fail on the day a server sends better or this repository
does better with it.

**And then the user renames the appointment.** Retyping the `SUMMARY` of the
defined-zone event and saving it is the edit that touches nothing the zone cares
about — Evolution offers no way to *redefine* a zone, and this mapping would refuse
to send a redefinition — and that is the point of doing it first: it has nothing to
do with the zone, so anything that happens to the zone is something the save did on
its own. A start the user restated could not tell "the zone survived" from "the
clock was re-sent in a way that happened to agree" — which is why the clock is moved
in a save of its own, below.

What the save asserts, from both ends:

- **What the user sees.** The new title, on an appointment that has not moved: the
  wall clock and the `TZID` unchanged, and the calendar still resolving that
  identifier to 08:00Z.
- **What the server holds.** The title the user typed, and — untouched — the
  `timeZone` and the whole of the `timeZones` definition. `jmap_cal_sync::patch`
  cannot express this zone in JSCalendar's terms, so it must leave `timeZone` out of
  the patch rather than send the iCalendar identifier it read or clear the property;
  and `timeZones` is stronger still, since no component EDS handed anyone even
  mentions it, so the only way it could change is a save overwriting what it never
  saw.

Two mutations were run against the save, and between them they say which end each
assertion belongs to. Adding `"timeZones": null` to the patch — the shape of a save
that writes back everything the mapping knows about, which is not everything the
server holds — reddens the server-side assertion **only**: within one session the
consumer still resolves the zone, because `ECalMetaBackend` gathered it into the
calendar's timezone store during the first read and answers out of that store, so
the loss is invisible to the client until a cache is filled fresh. And clearing the
zone the mapping cannot name — `timeZone: null`, the plausible reading of "we cannot
express it, so it is nothing" — reddens the consumer's pair immediately: the
`TZID` is gone from the `DTSTART`, the calendar can no longer name a zone, and the
appointment the user only renamed floats to 10:00Z, two hours from where it was.
That is the bug this guard exists for, seen from the user's side.

**And then the user drags the appointment.** The second save is the other kind of
edit there is, and it asks the opposite question of the same save path: the rename
says an edit with nothing to do with the clock leaves the clock alone, this one says
an edit about nothing *but* the clock leaves the *zone* alone. It is the likelier
bug of the two, because `patch::diff` reads a zone it cannot name off the component
on both sides — so the guard that keeps `timeZone` out of the patch is only
exercised where something else in the patch is a date-time.

The client retypes the value of the `DTSTART` already there, leaving every parameter
on it alone, and states the new length as a `DTEND` carrying the `TZID` it read off
that same start — with the `DURATION` the mapping drew removed beside it, since RFC
5545 §3.6.1 makes the two mutually exclusive. That is what Evolution's appointment
editor produces; and the program must not name the zone itself, or it would be
supplying the answer the leg is asking for.

The new clock is **14:30 on 2026-11-05**, deliberately on the far side of the
definition's `STANDARD` transition. A move inside CEST would resolve through the
same observance the read already proved; landing in CET says the yearly rules of
*both* halves survived into EDS's timezone store, which is what makes the zone a
zone rather than a fixed offset that happened to be right in April. What it asserts:
the title the rename gave it, the moved wall clock, the same identifier, and the
calendar resolving it to **13:30Z** — one hour, not two. On the server: `start` moved
to `2026-11-05T14:30:00` while `timeZone` and `timeZones` did not move at all, and a
`duration` of `PT1H30M`. The length travels by a different route from the start —
the start is a value copied across, the duration is *computed* from a pair of wall
clocks by `read_duration` — and a duration that came out of the subtraction wrong is
an appointment of the wrong length rather than at the wrong time, which no assertion
about `start` would catch. This is also the one place `read_duration`'s `DTEND`
branch is driven through real EDS, in a zone nothing outside the server can resolve.

Two mutations stand behind this half, and both were invisible before it existed.
Drawing only the `DAYLIGHT` observance of the definition puts the moved appointment
at **12:30Z** — April's summer offset applied in November, confidently an hour out —
while leaving *both* April lookups green at 08:00Z: a half-drawn zone is precisely
the failure a single-date measurement cannot see. And dropping `read_duration`'s
`DTEND` branch loses the length entirely: EDS reports no `DURATION` and no `DTEND`,
and the server's `duration` goes to null, so an appointment the user gave a new end
comes back with no end at all.

Three earlier mutations stand behind the read half: never drawing the `VTIMEZONE`,
which leaves the calendar unable to name the zone and both routes floating; drawing
only the `STANDARD` observance, which is the "half a definition is worse than none"
case made concrete — libical builds a zone from it happily and resolves the start
to **09:00Z**, confidently one hour out, which is exactly why the mapping draws a
definition whole or not at all; and swapping the two events on the command line,
which reddens the summary check that keeps the floating assertions off the defined
event.

`tests/functional/event-start.c` holds the function both this program and
`cal-edit-client.c` report the instant through. Shared rather than copied because
the two legs exist to be compared: a difference between their answers should be a
difference in the event, not in how it was measured.

### The fourth calendar leg: the zone only the client can name

`a_zone_only_the_client_can_name_reaches_the_server_with_its_definition`, against
the same program run as `functional-cal-zone-client create …`. It is the third leg
turned around: there a zone only the server could name had to reach a consumer,
here one only the *client* can name has to reach the server.

Which is not a hypothetical. It is what Evolution has in hand the moment the user
accepts an invitation whose `VTIMEZONE` names a zone no database holds — an
Exchange organiser's own — and the route it takes is the one EDS gives it:
`e_cal_client_add_timezone`, then a create whose `DTSTART` merely *names* the zone.
EDS does not carry the definition inside the component; it files it in the
calendar's own timezone store, which is the same platform fact the third leg pinned
down from the read side. So a backend that looked only at what it was handed would
see a `TZID` resolving nowhere, `jmap-ical`'s `maps_time_zone` would refuse it, and
the appointment would be filed **floating** — an hour or two out for everybody but
the user who typed it. `jmap_backend_cal`'s `resolve_time_zone` therefore asks the
calendar as its third and last place to look, and `ECalBackend` implementing
`ETimezoneCache` is what makes the store reachable at the vfunc.

That much had unit tests on both sides. What had none was the **join**: that the
zone a client sent is in the backend's own cache, under the identifier the
component still names, by the time EDS calls `save_component_sync`. That is a fact
about EDS, and this leg is where it stops being assumed.

The client is handed a `VTIMEZONE` in a file — written by the test, out of the same
description the expected `timeZones` map is written from, so the two ends cannot
agree by accident — and reads the identifier back off the zone libical made of it
rather than being told it, which keeps the program from being able to name a zone
the file did not. The zone is `/example.com/Somewhere-New`, a third `example.com`
identifier so that a definition arriving at the mock can only have come from here,
and it carries no `X-LIC-LOCATION`: one naming an IANA zone would make it a
*spelling* of a zone that already has a name, which `jmap-ical` translates and
libical's builtin table would then answer for, and the leg would pass without the
client's definition being read at all.

What it asserts, from both ends:

- **What EDS hands back**, which is not the component the client wrote: a create is
  answered out of what the *server* stored, so the `TZID` on the read-back is the
  zone as it came home. A backend that could not name the zone leaves it empty. The
  instant beside it — 08:00Z for a 10:00 start in April — says the round trip did
  not shift the clock; that the calendar still *knows* the zone is the weaker half
  of that pair, since the client put it in the store itself.
- **What the server holds**, which is the claim no client can fake: `timeZone` is
  the custom identifier, and `timeZones` is the whole definition. Asserting the
  identifier alone would pass on a backend that sent a dangling reference — RFC 8984
  §1.4.9 admits a custom `TimeZoneId` only beside the entry that says what it is,
  and a server is entitled to reject one without.

One mutation stands behind it: dropping the calendar from `resolve_time_zone`'s
list of places to look — the last of its three, the one added for exactly this —
reddens this leg and nothing else in the calendar suite. The appointment comes back
with no `TZID` at all and floats to 10:00Z, and the server holds it with no zone.

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

## What the config lookup test asserts

`rust/crates/jmap-functional/tests/config-lookup.rs`, against
`tests/functional/config-lookup-client.c`, is the odd one out among the other
five: it is not a daemon opening a book, calendar or mail store from a
`.source` keyfile, because a config lookup happens *before* any account
exists. It exercises `JmapConfigLookup` (`jmap-config/src/config_lookup.rs`),
the `EConfigLookupWorker` behind the account assistant's "Look Up Account
Details" step — M7's OAuth 2.0 autodiscovery path.

The client loads `module-jmap-configuration.so` itself, via
`e_module_load_all_in_directory`, rather than pointing a factory's module
directory at it: nothing scans for this module the way the address book and
calendar factories scan theirs (see `jmap_config::module`'s own doc comment
for why — briefly, `JmapConfigLookup` registers itself the moment a real
`EConfigLookup` is *constructed*, which only a client does). It then builds a
real `ESourceRegistry` and `EConfigLookup`, runs the lookup against the
mock's RFC 8414/7591 OAuth 2.0 endpoints (`MockServer::builder()
.oauth_authorization_server(...).oauth_client_registration(...)`), and
applies the one result it gets onto a scratch `ESource` via
`e_config_lookup_result_configure_source` — the same call the account
assistant makes when the user picks a result, and the only way to read a
"simple" result's added values back at all (`e-config-lookup-result-
simple.c` keeps them private).

Asserted:

- exactly one, complete, `jmap`-protocol result came back;
- the scratch source's `[Collection]` extension names the `jmap` backend and
  the email address as its identity;
- `[Authentication]` names the mock's own host and port (proving the 307th
  session's `parse_target` fix — a bare `scheme://host:port` `servers`
  value, the only way to name a plaintext, non-default-port deployment at
  all — actually reaches a real `EConfigLookup`, not just `config_lookup.rs`'s
  own unit tests) and this crate's own `EOAuth2Service` (`JMAP`) as the
  authentication method;
- `[Security] method` is `none`, matching the mock's plaintext origin.

What this does not, and cannot, cover: the actual browser consent exchange.
That is `ECredentialsPrompterImplOAuth2`'s job, not this worker's — `run()`
only has to leave behind a `client_id` and the discovered endpoints before a
prompter ever opens, and this test's assertions stop at exactly that
boundary. `docs/NIGHT-LOG.md`'s 307th session hand-drove this same dispatch
once with a throwaway client before this test existed; this is that spike
made permanent and repeatable.

## What the collection test asserts

`rust/crates/jmap-functional/tests/collection.rs`, against `tests/functional/
collection-client.c`, is the odd one out among these seven in a different way
again: it is not a factory opening a leaf backend from a `.source` file at
all, but `evolution-source-registry` itself loading `module-jmap-backend.so`
(`EDS_REGISTRY_MODULES`) and turning one collection account into the
children `docs/manual-test-collection-backend.md`'s hand-run recipe
describes — `jmap-backend-collection`'s populate/fan-out, checked through a
real registry instead of the crate's own in-process `EServerSideSource`
fixtures.

The client connects to the registry, confirms the account keyfile itself
was picked up (`account-found`), then polls `e_source_registry_list_sources`
until it sees two sources naming the account as `Parent=` or a 30-second
deadline passes — the same polling shape `connection-status.c` uses for a
different property of the same kind of source, since the children are
written to disk by the backend and picked up by the registry's own file
monitor, an asynchronous step with no simpler signal to wait on.

Asserted:

- the account was found at all — a registry that never loaded the module,
  or loaded it and matched no `BackendName=jmap` collection factory, fails
  here first;
- exactly one address-book child and one calendar child appeared, matching
  `ContactsEnabled=true`/`CalendarEnabled=true` in the keyfile;
- each child names `jmap` as its own `BackendName`, the account as its
  `Parent=`, and is enabled — the properties `child_added` is actually
  responsible for, not merely that some source of the right kind exists;
- the mock recorded an `AddressBook/get` and a `Calendar/get` (both fan-out
  asks with `ids: null`) and no `Mailbox/get`, matching `MailEnabled=false`.

What this does not cover, left for a future increment now that this harness
exists: D2's colour push (`source_changed`, which needs a running
calendar-factory backend instance rather than just the registry). The
`create_resource_sync`/`delete_resource_sync` half — for both an address
book and a calendar — is covered below.

## What the collection-create test asserts

`rust/crates/jmap-functional/tests/collection-create.rs`, against
`tests/functional/collection-create-client.c`, is the write half of the
surface the test above only reads: Evolution's own "New Address Book"/
"Delete" calls — `e_source_remote_create_sync()` on the account and
`e_source_remote_delete_sync()` on the child it returns — against the same
real registry, proving `ECollectionBackendClass::create_resource_sync`/
`delete_resource_sync` rather than populate/fan-out.
`jmap-backend-collection`'s own `tests/{create_resource,delete_resource}.rs`
drive those same vfuncs directly against an in-process `EServerSideSource`
they build themselves; this is the one test in the tree that reaches them
through a real registry's own D-Bus round trip instead.

**Not `e_source_registry_create_sources_sync`/`e_source_remove_sync`, which
this test used at first.** Those write a source's keyfile straight to the
registry's own directory with no backend involved — the right pair for a
standalone account, not a collection's child — and a first run against them
"passed" a client that never called `create_resource_sync` at all: the
child appeared with an empty `BackendName`, no `[Resource]`/`[Authentication]`/
`[Security]` groups, and no request reached the mock, then refused to be
removed with "is not removable" (a backend child's `removable` is always
`FALSE`; only `remote-deletable` lets it go). `e_source.h`'s own comments on
`remote_create_sync`/`remote_delete_sync` are what named the actual pair:
`remote_create_sync` is called *on the account*, passing a scratch source as
an argument, and requires `ESource:remote-creatable`; `remote_delete_sync`
is called *on the child itself* and requires `ESource:remote-deletable`.

The client waits for the account to become `remote-creatable` (set by the
first populate, per `jmap-backend-collection::populate::Populating::
offer_creation`) — the flag `e_source_remote_create_sync` refuses outright
without — then builds a scratch `ESource` the same way "New Address Book"
does: no `GDBusObject` yet, the `[Address Book]` extension naming the kind,
and a display name (the mock's `AddressBook/set` handler refuses an empty
one). No uid and no `Parent=` on the scratch source — the registry service
mints its own uid for the child (`e_server_side_source_new_user_file`, per
`create_resource.rs`'s own module comment) and `adopt_created` sets
`Parent=` itself — so after `e_source_remote_create_sync` succeeds the
client finds the new child the way `collection-client.c` finds discovered
ones: by listing which address book names the account as `Parent=`. It then
calls `e_source_remote_delete_sync` on it and polls for it to disappear the
same way.

Asserted:

- the account was found and became `remote-creatable`;
- the created child appeared, naming `jmap` as its `BackendName`, the
  account as its `Parent=`, enabled and writable — the same properties
  `create_resource.rs`'s own module doc says `adopt_created` is responsible
  for;
- the child disappeared after `e_source_remote_delete_sync`;
- the mock recorded exactly two `AddressBook/set` calls — one create, one
  destroy — not merely a source appearing and disappearing locally with no
  server round trip.

## What the collection-create-calendar test asserts

`rust/crates/jmap-functional/tests/collection-create-calendar.rs`, against
`tests/functional/collection-create-calendar-client.c`, is the same proof as
the test above, run for a calendar instead of an address book —
`E_SOURCE_EXTENSION_CALENDAR` and `Calendar/set` in place of the address-book
extension and `AddressBook/set`. The client is a line-for-line mirror of
`collection-create-client.c` with the extension swapped; nothing about the
create/delete sequence itself differs between the two kinds of child.

The reason this is its own test rather than a second case inside the
existing one: which `/set` call a create or delete reaches is exactly the
kind of thing a resource-kind mixup gets wrong silently — an address book
and a calendar may share one resource id (RFC 8620 §1.2), so a backend that
guessed the kind from the id alone could destroy the wrong object and still
report success. `jmap-backend-collection/tests/delete.rs` already guards
against that on the read/delete-decision side with an in-process fixture;
this is the same risk, checked from the write side, through a real registry.

Asserted, mirroring the address-book test property for property: the
account was found and became `remote-creatable`; the created child
appeared, naming `jmap` as its `BackendName`, the account as its `Parent=`,
enabled and writable; the child disappeared after
`e_source_remote_delete_sync`; the mock recorded exactly two `Calendar/set`
calls (one create, one destroy) and **no** `AddressBook/set` call at all —
the check that a calendar create/delete does not silently reach the other
kind's server call.

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
