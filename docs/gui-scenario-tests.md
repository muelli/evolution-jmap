<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# GUI scenario tests: user tasks in the real Evolution

A GUI scenario does a user task in the real Evolution, for example "delete a
message". Then it checks the result on the server and in the log. It does not
compare pixels. It saves screenshots of the key steps for a human review.

The GUI smoke test (`docs/gui-smoke-test.md`) checks only that an account
shows. The scenarios check what the user does with the account.

## Run a scenario

The scenarios run in a rootless podman container with the Evolution 3.52 of
Ubuntu 24.04. They never use your own Evolution profile.

1. Build the tree on Ubuntu 24.04:

   ```console
   $ cmake -S . -B build -G Ninja
   $ cmake --build build
   ```

2. Run one or more scenarios:

   ```console
   $ ci/gui-scenario/run.sh mail-trash
   ```

The first run builds the image `evolution-jmap-gui-scenario` from
`ci/gui-scenario/Containerfile`. This takes about 5 minutes. The image is
about 1 GB. Later runs use the image again.

The script installs the five package components of `build/` into a new
container for each run. Then it starts these items for each scenario:
`jmap-mockd`, `Xvfb`, `openbox`, a private D-Bus session and Evolution with
`EVOLUTION_JMAP_LOG=trace`.

Use `run.sh --host` only in a throwaway machine or container. It needs the
components installed as root.

These variables change the defaults:

| Variable | Default | Meaning |
|---|---|---|
| `GUI_SCENARIO_BUILD` | `build` | The build tree to install |
| `GUI_SCENARIO_ARTIFACTS` | `build/gui-scenario` | The output directory |
| `GUI_SCENARIO_IMAGE` | `localhost/evolution-jmap-gui-scenario` | The image |
| `GUI_SCENARIO_PORT` | `8090` | The port of the mock |
| `GUI_SCENARIO_DISPLAY` | `:96` | The display of `Xvfb` |
| `GUI_SCENARIO_MOCKD` | `build/cargo-target/release/jmap-mockd` | The `jmap-mockd` binary |

### On a newer Evolution

Ubuntu 24.04 has Evolution 3.52. To run a scenario against a newer Evolution,
use this command:

```console
$ ci/gui-scenario-fedora.sh snooze
```

If `podman` is installed, the script runs in the pinned Fedora image that
`ci/rpm.sh` and `ci/gui-smoke-fedora.sh` also use (Evolution and EDS 3.60). It
builds the tree into `build-gui-scenario-fedora/`, installs the five
components and runs `ci/gui-scenario/run.sh --host`. You do not need root.
Use `ci/gui-scenario-fedora.sh --host SCENARIO...` in a job that already runs
in that image. The artifacts go to `build-gui-scenario-fedora/gui-scenario`,
or to `GUI_SCENARIO_ARTIFACTS` if you set it.

## The output

Each scenario writes to `build/gui-scenario/<scenario>/`:

- `NN-<step>.png`: a screenshot of each key step, in sequence.
- `transcript.txt`: each step and each check, with the time.
- `evolution.log`: the output of Evolution, with the trace of the Camel
  provider.
- `mock.log`: each JMAP request to the mock.
- `dbus.log`: the output of the D-Bus services, for example the EDS
  factories.

A failed scenario also writes `failure.png`, `atspi-tree.txt` (the AT-SPI tree
of all applications) and `recording.mp4`.

## The scenarios

### `mail-trash`

The account has a mailbox with the `trash` role. The scenario does these
steps and checks:

1. Delete a message in the inbox. The server moves the message to the trash.
2. Delete the message in the trash, then do **Folder > Expunge**. The server
   destroys the message.
3. Delete a second message in the inbox, then do **File > Empty Trash**. The
   server destroys the message. The trash and the inbox are empty.

The log must show each move and each destroy. The log must show no error of
evolution-jmap.

### `shared-calendar-delete` and `shared-book-delete`

A collection account shows a calendar (or an address book) that a different
account shares with the user. The scenario does these steps and checks:

1. Delete the shared collection. An info bar with **Undo** shows. The text
   tells that the delete is for all users who share the collection. The
   collection disappears from the source selector. The server still has it.
2. Click **Undo**. The collection shows again in the source selector.
3. Delete the collection again. A new info bar shows. The server still has
   the collection after the end of the first undo window, and also just
   before the end of the second window.
4. Wait. At the end of the second window, the server destroys the
   collection. The collections of the user's own account do not change.

Each scenario takes about 2 minutes, because the undo window is 60 seconds.
The registry log in `dbus.log` must show the destroy. No log must show an
error of evolution-jmap.

The mock has no plain shared account. These scenarios start the mock with
`--shared-account-name-copies-user`, so the shared account has the name of
the user, and the collection shows as "alice@example.com: Team calendar".

### `edit-conflict-event` and `edit-conflict-contact`

The account has one event (or one contact). Evolution edits one field, and
the server changes a different field at the same time. The scenario does
these steps and checks:

1. Open the item in its editor.
2. Change a different field on the server: the description of the event, or
   the note of the contact. Evolution gets the change through push and
   refreshes its cache. The open editor still shows the old value.
3. Change the summary of the event (or the full name of the contact) in the
   editor, and save.
4. The server must have the new value of each field.

The backends put a stamp (`X-JMAP-BASE`) in each event and contact that they
give to Evolution, and keep the recent server versions in the cache. A save
uses the version that the stamp names as the base, so both scenarios pass.

Evolution 3.52 has two limits here. **File > Work Offline** does not take the
address books and calendars offline, so a scenario cannot make an offline
edit with it. The contact editor does not enable **Save** after a change of
the nickname only.

### `send-later`

Two standalone mail accounts use the same mock account. The server has an
identity for the address of the first account (`alice@example.com`). It has
no identity for the address of the second account (`mallory@example.com`).
The scenario does these steps and checks:

1. In the first account, write a message and do **File > Send Later > In One
   Hour**. The server must have one `EmailSubmission` for the message, with
   the identity of Alice, the undo status `pending` and a hold of one hour.
   The message must be in Sent and must not be a draft. The composer closes,
   and the Sent folder of the account shows the message.
2. In the second account, write a message and do the same. A dialog tells
   that the account cannot send mail as `mallory@example.com`. The server
   must have no new submission and no copy of the message. The composer
   stays open.

The log must show no error of evolution-jmap.

The scenario starts the mock with `--max-delayed-send 86400`, so the server
offers scheduled send. Use a `jmap-mockd` that has this option. If the
`jmap-mockd` of your build tree does not have it, set `GUI_SCENARIO_MOCKD` to
a `jmap-mockd` that has it.

The mock has a fixed clock. Thus the scenario compares `sendAt` with the
time of the mock, not with the local time.

### `snooze`

One standalone mail account. The mock offers the snooze extension of Cyrus
and has no mailbox with the role `snoozed`. The scenario does these steps and
checks:

1. In the inbox, open the context menu of a message (the Menu key) and do
   **Snooze > For One Hour**. The server must have a new mailbox with the
   role `snoozed`. The message must be only in that mailbox, and its
   `snoozed.until` must be one hour after the click. The Snoozed folder
   appears in the folder tree, the inbox stays selected, and the message
   leaves the message list.
2. Do **Snooze > Until Next Monday Morning** on the other message. Its
   `snoozed.until` must be a Monday at 09:00 local time, in the next seven
   days.

The log must show no error of evolution-jmap.

The scenario starts the mock with `--snooze-extension`. Use a `jmap-mockd`
that has this option, as for `send-later`. The client calculates the times
from the local clock, so the scenario compares them with the local time.

A right-click does not open the context menu of a message: the AT-SPI
extents of an ETable cell point at the column header. The scenario uses the
Menu key instead (`popup_menu()`).

### `contact-photos` and `contact-photo-file`

One collection account with an address book. The server has contacts with a
photo. For each contact, the scenario does these steps and checks:

1. The contact shows in the address book. The scenario reads the vCard of
   the contact from the cache of Evolution (`cache.db`) and checks its
   `PHOTO` line.
2. Select the contact. The preview shows the contact.
3. Change the full name in the contact editor and save. The server must have
   the new name, and the photo on the server must not change.

`contact-photos` has two contacts:

- A photo with an `http:` URI. The cache must have the same URI. The
  scenario serves the image on port 8091 of the local host. The preview must
  not get the image: Evolution 3.52 shows a broken image for it.
- A photo with a `data:` URI. EDS keeps the image in a file of the cache, so
  the cache must have a `file:` URI of a file with the same bytes. The
  preview shows the image.

`contact-photo-file` starts the mock with `--contact-photo-uses-file-uri`, so
the server gives the photo as a `file:` URI. The cache must have no `PHOTO`
line, so Evolution never reads the local file. After the save, the server
must still have its photo entry.

The log must show no error of evolution-jmap.

### `vacation`

One mail account in a collection, as the account assistant makes it. The
server has an autoresponder that is on, with two dates, a subject and a
message. The scenario does these steps and checks:

1. Open the account editor (the Menu key on the account in the folder tree,
   then **Properties**) and go to **Vacation Responder**. The check box
   **Send automatic replies** is on, and the page shows the subject and the
   message of the server.
2. Change the subject and click **OK**. The server must have the new
   subject. The other fields must not change, also the two dates.
3. Open the editor again and click **OK** with no change. The state of the
   `VacationResponse` on the server must not change.
4. Open the editor again, clear the check box and click **OK**. The fields
   become insensitive. The server must have `isEnabled` false, with the
   same subject and message.

Before steps 3 and 4, the scenario opens the inbox again. Evolution
disconnects the store after each save in the account editor. The store must
connect again when it opens a folder.

The log must show no error of evolution-jmap.

## Write a scenario

A scenario is a Python file in `ci/gui-scenario/scenarios/`. It calls
`main(setup, run)` from `ci/gui-scenario/scenario.py`. Two optional arguments
of `main()` change the start: `component` is the Evolution view to open
(default `mail`), and `mock_args` are more arguments for `jmap-mockd`.

- `setup()` runs before Evolution starts. Write the source files and the
  server state here (for example `write_standalone_mail_account()` and
  `Jmap().create_mailbox()`).
- `run()` runs after Evolution starts. Do the user steps and the checks here.

Obey these rules:

- Check the server state with the `Jmap` class. Do not check pixels.
- Use `wait_for()` for each check. Evolution writes to the server some
  seconds after the user action.
- Use `menu_item()` for menu commands. It does the action from a child
  process, because some actions run a modal dialog before they return.
  A path can name submenus, for example `"File/New/Mail Message"`. Give the
  title of a window as the second argument to use the menu bar of that
  window, for example of a composer.
- To write more than one mail account, give each account its own `key` in
  `write_standalone_mail_account()`.
- To open the account editor, use `write_collection_mail_account()`. On
  **OK**, the editor writes the server of a standalone account again from
  the address of the account. This is a known problem.
- In the composer, find the **To** field by its name `"To:"`. Find the
  **Subject** field with `labelled_field()`.
- Use `confirm()` for a confirmation dialog.
- Use `select_folder()` to select a folder. The AT-SPI action "activate"
  opens the folder but does not select the row. Evolution then disables the
  folder commands, for example **Expunge**.
- Use `select_source()` to select a calendar or an address book. It moves
  the cursor of the source selector, as `select_folder()` does.
- The registry and the factories log to `dbus.log`. Give `log="dbus.log"` to
  `expect_log()` and `expect_no_errors()` to check that log.
- Call `screenshot()` at each step that a human must see.
- Use `double_click()` to open an item in its editor, and `window()` to find
  the editor. The fields of the editors have no name: find a field by its
  text with `text_field()`, and type with `set_text()`. `set_text()` uses
  key events, because the contact editor ignores a text that AT-SPI sets.
