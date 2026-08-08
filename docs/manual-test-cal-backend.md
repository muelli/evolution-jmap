# Manual test: the JMAP calendar backend

The test suite exercises every layer of the calendar backend except the one
that matters to a user: `evolution-calendar-factory` loading the module,
picking the factory out of it, and opening a calendar from a `.source` file.
Constructing an `ECalMetaBackend` needs a running registry, so that last step
cannot be a `cargo test` — it is this recipe.

It talks to `jmap-mockd`, not to a real server, so it can be run on a laptop
with nothing installed and no account anywhere. It is the mirror of
[the address book recipe](manual-test-book-backend.md), and the two are
independent: run either without the other.

## What is already checked without you

`rust/crates/jmap-backend-cal/tests/recipe.rs` loads the keyfile below through
`e_server_side_source_new` — the same call `evolution-source-registry` makes
for every file in its sources directory — and asserts that it means what this
document says it means: the origin, the anonymous connection, a `BackendName`
that matches the name the factory registers, and that the group carrying that
name is `[Calendar]` and not one of the two neighbouring ones. So the keyfile
cannot rot silently; the steps around it are what you are testing.

## Prerequisites

- Evolution Data Server 3.52 or newer, *installed*: the development headers are
  enough to build, but this recipe runs `evolution-source-registry` and
  `evolution-calendar-factory`, which come with the `evolution-data-server`
  package (Debian/Ubuntu) or `evolution-data-server` (Fedora).
- A session bus and a running `gnome-keyring`/`libsecret` provider — the
  anonymous account below never asks for a password, but the registry expects a
  session it can talk on.

## 1. Start the mock server

```console
$ cargo run -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

It seeds one account with a calendar named "Personal", flagged as the account
default. Leave it running in its own terminal; every request it serves is one
the backend made.

Do not pass `--basic` or `--bearer` for the first run. The account below names
no user, so the backend connects anonymously and no password prompt gets in the
way of the thing being tested.

## 2. Build and install the backend

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component cal-backend
```

That installs `libecalbackendjmap.so` into the directory
`pkg-config --variable=backenddir libedata-cal-2.0` reports — on Debian and
Ubuntu, `/usr/lib/evolution-data-server/calendar-backends`. A *different*
directory from the address book's, which is why both backends can be called
`jmap` and both export `e_module_load`.

If you would rather not install into a system directory, install into a scratch
tree and point the factory at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component cal-backend
$ export EDS_CALENDAR_MODULES=/tmp/jmap/usr/lib/evolution-data-server/calendar-backends
```

`EDS_CALENDAR_MODULES` *replaces* the backend directory rather than adding to
it, so a factory started with it set has the JMAP backend and no other — fine
for this test, and the reason the variable is not a way to run Evolution day to
day. It has to be set in the environment the factory is started from, which for
a D-Bus activated one means restarting the daemons (step 4).

## 3. Write the account

Copy `docs/examples/jmap-mock-calendar.source` to
`~/.config/evolution/sources/jmap-mock-calendar.source` — the file name
matters, it becomes the source UID, and it has to differ from the address
book's if you have run that recipe too:

```ini
[Data Source]
DisplayName=JMAP mock calendar
Enabled=true

[Calendar]
BackendName=jmap

[Authentication]
Host=127.0.0.1
Port=8080

[Security]
Method=none
```

Four of those lines are load-bearing:

- `[Calendar]` is the group that decides what kind of thing this source is, and
  it is the calendar-specific half of the recipe. `ECalBackendFactory` keys
  itself by backend name *and* component kind: the factory in this module
  registers `jmap:VEVENT` and nothing else, so writing `[Task List]` or
  `[Memo List]` instead produces a source that parses, appears in the registry,
  and is claimed by no factory at all. There is no JMAP task or note type to
  map those onto yet; see `factory::COMPONENT_KIND` for why registering them
  anyway would be worse than leaving them out.
- `BackendName=jmap` is the other half of that key. It is matched against the
  factory's own name; nothing else in the file selects a backend.
- `Method=none` is what allows plain HTTP. The backend refuses it for any host
  that is not loopback, so this line only ever works for a local mock or a
  development server on the same machine. **The key is `Method`, not
  `Secure`** — `ESourceSecurity:secure` is a boolean *over* the `Method`
  string, so a keyfile saying `Secure=true` sets nothing EDS reads and means
  `none`; the address book recipe has the long version, and
  `jmap-backend-book`'s `tests/recipe.rs` pins it.
- No `User=`: with one, the backend asks EDS for a password and refuses to
  connect until it gets one. To test *that* path instead, start the mock with
  `--basic alice:secret`, add `User=alice@example.com` and
  `Method=plain/password` to `[Authentication]`, and let Evolution prompt you.

There is no `[Resource] Identity=`, so the backend asks the server for the
account's default calendar. Naming one is the other half of the recipe: ask the
mock for its calendars and add the id you want under a `[Resource]` group. The
`apiUrl` is in `/.well-known/jmap`, and on the mock it is `/jmap`; no
credentials, because the mock was started without any.

```console
$ curl -s -X POST http://127.0.0.1:8080/jmap \
    -H 'Content-Type: application/json' \
    -d '{"using":["urn:ietf:params:jmap:calendars"],
         "methodCalls":[["Calendar/get",{"accountId":"A1"},"c0"]]}'
{"methodResponses":[["Calendar/get",{"accountId":"A1","list":[{"id":"CAL1",
"isDefault":true,"isSubscribed":true,"name":"Personal"}],...
```

`Identity` is read out of the same `[Resource]` group for both backends and
means a different thing in each: a calendar id here, an address book id in the
address book recipe. A source that names one the server does not have fails to
open with "the account names calendar …, which the server does not have",
which is the error to expect if you paste the wrong id.

Until M6's collection backend exists there is no account to hang this off, so
the source has no `Parent=` and appears on its own.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-calendar-factory
$ pkill -f evolution-source-registry
$ evolution
```

Both are D-Bus activated and come back on demand. Then, in Evolution's Calendar
view, "JMAP mock calendar" should be in the calendar list and tick on to show
the events the mock has (none on a fresh mock — create one from Evolution and
watch the mock log the `CalendarEvent/set`).

To watch the backend instead of using the UI:

```console
$ G_MESSAGES_DEBUG=all evolution-calendar-factory -r -w
```

started by hand *before* Evolution, with the mock's terminal next to it: the
factory logs each module it loads, and every request the backend makes shows up
in the mock's output.

## What "it worked" means

- `evolution-calendar-factory` loaded `libecalbackendjmap.so` — it appears in
  the factory's debug output, and a `BackendName` nothing claims produces a
  visible "No backend factory for ..." error instead.
- Ticking the calendar on made the mock serve `/jmap/session` and a
  `CalendarEvent/query` + `CalendarEvent/get` pair.
- Creating an event in Evolution made it serve `CalendarEvent/set`, and the
  event survives closing and reopening the calendar — which also means it went
  through the meta backend's cache and came back.
- Editing that event's summary or moving its start time makes a second
  `CalendarEvent/set`, an `update` rather than a `create`; the mock's log tells
  the two apart, and it is what says the round trip through iCalendar kept the
  UID.

Anything short of that is a bug in this repository, not in the recipe;
`docs/NIGHT-LOG.md` is where the ones found this way get written down.
