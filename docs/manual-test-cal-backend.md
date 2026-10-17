# Manual test: the JMAP calendar backend

The test suite covers every layer of the calendar backend except
`evolution-calendar-factory` loading the module and opening a calendar from a
`.source` file. That step needs a running registry, so it cannot be a
`cargo test`. This recipe covers it.

It uses `jmap-mockd`, not a real server, so you need no account. It mirrors
[the address book recipe](manual-test-book-backend.md). You can run either
one alone.

## What automated tests already cover

`rust/crates/jmap-backend-cal/tests/recipe.rs` loads the keyfile below with
`e_server_side_source_new`. It checks the origin, the anonymous connection,
that `BackendName` matches the factory name, and that the group is
`[Calendar]` (not `[Task List]` or `[Memo List]`). It also checks that the
keyfile in this document is identical to
`docs/examples/jmap-mock-calendar.source`.

## Prerequisites

- Evolution Data Server 3.52 or newer, installed (the
  `evolution-data-server` package). This recipe runs
  `evolution-source-registry` and `evolution-calendar-factory`.
- A D-Bus session bus.

## 1. Start the mock server

```console
$ cargo run --manifest-path rust/Cargo.toml -p evolution-jmap-mock --bin jmap-mockd
jmap-mockd listening on http://127.0.0.1:8080
```

It seeds one account (`A1`) with a calendar named "Personal", which is the
account default. Leave it running in its own terminal. It logs each request
that the backend makes.

Do not pass `--basic` or `--bearer` for the first run. The account below has
no user, so the backend connects anonymously and there is no password prompt.

## 2. Build and install the backend

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component cal-backend
```

This installs `libecalbackendjmap.so` into
`pkg-config --variable=backenddir libedata-cal-2.0` (on Debian and Ubuntu,
`/usr/lib/evolution-data-server/calendar-backends`). That directory is
different from the address book's, so both backends can be named `jmap`.

To avoid a system directory, install into a scratch tree and point the
factory at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component cal-backend
$ export EDS_CALENDAR_MODULES=/tmp/jmap/usr/lib/evolution-data-server/calendar-backends
```

`EDS_CALENDAR_MODULES` replaces the backend directory, so the factory then
has only the JMAP backend. Set it in the environment that starts the factory
(step 4).

## 3. Write the account

Copy `docs/examples/jmap-mock-calendar.source` to
`~/.config/evolution/sources/jmap-mock-calendar.source`. The file name
becomes the source UID, so it must differ from the address book's.

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

Notes on the keys:

- `[Calendar]` sets the kind of source. `ECalBackendFactory` is keyed by
  backend name and component kind, and this module registers only
  `jmap:VEVENT` (`factory::COMPONENT_KIND`). A `[Task List]` or `[Memo List]`
  source parses but no factory opens it.
- `BackendName=jmap` is the other half of that key.
- `Method=none` allows plain HTTP, and only for a loopback host. **The key is
  `Method`, not `Secure`**: EDS ignores `Secure=true`. See the address book
  recipe.
- There is no `User=`. To test password sign-in, start the mock with
  `--basic alice:secret`, add `User=alice` and `Method=plain/password` to
  `[Authentication]`, and let Evolution prompt you.

Without `[Resource] Identity=`, the backend uses the account's default
calendar. To name one, list the mock's calendars (its `apiUrl` is `/jmap`)
and add the id in a `[Resource]` group:

```console
$ curl -s -X POST http://127.0.0.1:8080/jmap \
    -H 'Content-Type: application/json' \
    -d '{"using":["urn:ietf:params:jmap:calendars"],
         "methodCalls":[["Calendar/get",{"accountId":"A1"},"c0"]]}'
```

A source that names an id the server does not have fails to open with "the
account names calendar …, which the server does not have".

The source has no `Parent=`, so it appears on its own.
`docs/manual-test-collection-backend.md` covers the account form.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-calendar-factory
$ pkill -f evolution-source-registry
$ evolution
```

Both daemons start again on demand. In Evolution's Calendar view, "JMAP mock
calendar" is in the calendar list. It is empty on a fresh mock. Create an
event in Evolution and watch the mock log `CalendarEvent/set`.

To watch the backend instead, start the factory by hand before Evolution:

```console
$ G_MESSAGES_DEBUG=all evolution-calendar-factory -r -w
```

## What "it worked" means

- `evolution-calendar-factory` loaded `libecalbackendjmap.so` (see its debug
  output).
- Showing the calendar made the mock serve `/jmap/session` and
  `CalendarEvent` requests.
- An event created in Evolution made the mock serve `CalendarEvent/set`, and
  the event is still there after you close and open the calendar again.
- Editing the event's summary or start time makes another
  `CalendarEvent/set` with an `update`, not a `create`. That shows the UID
  survived the round trip through iCalendar.

Anything less is a bug in this repository. Report it.
