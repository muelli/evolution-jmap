# Manual test: the JMAP calendar backend

The test suite covers every layer of the calendar backend except one step.
This step is the loading of the module by `evolution-calendar-factory` and the
opening of a calendar from a `.source` file. The step needs a running
registry. It cannot be a `cargo test`. This recipe covers it.

The recipe uses `jmap-mockd`, not a real server. You do not need an account.
It mirrors [the address book recipe](manual-test-book-backend.md). You can run
either recipe alone.

## What automated tests already cover

`rust/crates/jmap-backend-cal/tests/recipe.rs` loads the keyfile below with
`e_server_side_source_new`. The test checks these items:

- the origin
- the anonymous connection
- that `BackendName` matches the factory name
- that the group is `[Calendar]` (not `[Task List]` or `[Memo List]`)
- that the keyfile in this document is identical to
  `docs/examples/jmap-mock-calendar.source`

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

The mock server seeds one account (`A1`) with a calendar named "Personal".
This is the default of the account. Leave the server running in its own
terminal. It logs each request that the backend makes.

Do not use `--basic` or `--bearer` for the first run. The account below has no
user. The backend therefore connects anonymously, and no password prompt
appears.

## 2. Build and install the backend

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component cal-backend
```

This installs `libecalbackendjmap.so` into
`pkg-config --variable=backenddir libedata-cal-2.0` (on Debian and Ubuntu,
`/usr/lib/evolution-data-server/calendar-backends`). This directory is
different from the directory of the address book. Both backends can therefore
have the name `jmap`.

To avoid a system directory, install into a scratch tree. Then point the
factory at it:

```console
$ DESTDIR=/tmp/jmap cmake --install build --component cal-backend
$ export EDS_CALENDAR_MODULES=/tmp/jmap/usr/lib/evolution-data-server/calendar-backends
```

`EDS_CALENDAR_MODULES` replaces the backend directory. The factory then has
only the JMAP backend. Set the variable in the environment that starts the
factory (step 4).

## 3. Write the account

Copy `docs/examples/jmap-mock-calendar.source` to
`~/.config/evolution/sources/jmap-mock-calendar.source`. The file name becomes
the source UID. It must therefore differ from the file name of the address
book.

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

- `[Calendar]` sets the kind of source. `ECalBackendFactory` uses the backend
  name and the component kind as its key. This module registers only
  `jmap:VEVENT` (`factory::COMPONENT_KIND`). A `[Task List]` or `[Memo List]`
  source parses, but no factory opens it.
- `BackendName=jmap` is the other half of that key.
- `Method=none` allows plain HTTP, and only for a loopback address. **The key
  is `Method`, not `Secure`**: EDS ignores `Secure=true`. See the address book
  recipe.
- There is no `User=`. To test password sign-in, do these steps:
  1. Start the mock server with `--basic alice:secret`.
  2. Add `User=alice` and `Method=plain/password` to `[Authentication]`.
  3. Let Evolution prompt you.

If `[Resource] Identity=` is not set, the backend uses the default calendar of
the account. To name a calendar, list the calendars of the mock server (its
`apiUrl` is `/jmap`). Then add the id in a `[Resource]` group:

```console
$ curl -s -X POST http://127.0.0.1:8080/jmap \
    -H 'Content-Type: application/json' \
    -d '{"using":["urn:ietf:params:jmap:calendars"],
         "methodCalls":[["Calendar/get",{"accountId":"A1"},"c0"]]}'
```

If a source names an id that the server does not have, the source fails to
open. The message is "the account names calendar …, which the server does not
have".

The source has no `Parent=`. It therefore appears on its own.
`docs/manual-test-collection-backend.md` covers the account form.

## 4. Restart the daemons and look

```console
$ pkill -f evolution-calendar-factory
$ pkill -f evolution-source-registry
$ evolution
```

The daemons start again on demand. In the Calendar view of Evolution, "JMAP
mock calendar" is in the calendar list. On a fresh mock server, it is empty.
Create an event in Evolution. Watch the mock server log `CalendarEvent/set`.

To watch the backend instead, start the factory by hand before Evolution:

```console
$ G_MESSAGES_DEBUG=all evolution-calendar-factory -r -w
```

## What "it worked" means

- `evolution-calendar-factory` loaded `libecalbackendjmap.so` (see its debug
  output).
- When you showed the calendar, the mock server served `/jmap/session` and
  `CalendarEvent` requests.
- When you created an event in Evolution, the mock server served
  `CalendarEvent/set`. The event is still there after you close and open the
  calendar again.
- If you edit the summary or the start time of the event, the mock server
  serves another `CalendarEvent/set`. It has an `update`, not a `create`. This
  shows that the UID survived the round trip through iCalendar.

Any other result is a bug in this repository. Report it.
