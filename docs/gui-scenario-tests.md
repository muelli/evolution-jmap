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

Today the save puts the old value of the server field back on the server, so
both scenarios fail at step 4. The edit uses the refreshed cache as the base,
not the version that the editor opened.

Evolution 3.52 has two limits here. **File > Work Offline** does not take the
address books and calendars offline, so a scenario cannot make an offline
edit with it. The contact editor does not enable **Save** after a change of
the nickname only.

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
