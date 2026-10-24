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

## Write a scenario

A scenario is a Python file in `ci/gui-scenario/scenarios/`. It calls
`main(setup, run)` from `ci/gui-scenario/scenario.py`.

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
- Call `screenshot()` at each step that a human must see.
