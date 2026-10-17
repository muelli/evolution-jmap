# GUI smoke test: a real Evolution against the mock

The functional tests (`docs/functional-tests.md`) drive EDS through its
client API, never through Evolution itself. The GUI smoke test starts the
real Evolution under `Xvfb` against `jmap-mockd` and checks that a JMAP
account appears in the mail view with mail in it.

It is a canary, not coverage. A full scripted GUI suite is out of scope.

## What `ci/gui-smoke.sh` does

1. Starts `jmap-mockd` on a scratch port.
2. Starts a private `Xvfb` and a private D-Bus session bus, and turns on
   AT-SPI (`gsettings set org.gnome.desktop.interface toolkit-accessibility
   true`). Everything runs in a throwaway `HOME` and XDG tree, so it cannot
   touch your own Evolution data.
3. Writes the three mail sources from
   `docs/examples/jmap-mock-standalone-{mail,identity,transport}.source`
   (the files of `docs/manual-test-mail-provider.md`) into that tree. A
   standalone account needs only the Camel provider installed.
4. Starts `evolution -c mail --force-online` and records the display with
   `ffmpeg -f x11grab` into tmpfs (`$GUI_SMOKE_RECORDING_ROOT`, default
   `/dev/shm`).
5. Runs `ci/gui-smoke-assert.py`, which uses AT-SPI to dismiss the one-time
   dialogs (below) and then waits for a folder tree cell `JMAP mock mail`
   with a child `Inbox (N)`, `N > 0`.
6. On failure, saves a screenshot, the recording, an AT-SPI tree dump and
   the Evolution and mock logs in `$GUI_SMOKE_ARTIFACTS`. A passing run
   leaves nothing.
7. Retries once with a fresh scratch tree before it reports failure.

## The one-time dialogs of a fresh profile

A scratch `HOME` has no keyring and no saved credentials. Camel asks the
session to authenticate every account before its first connection, even
though these sources request no credential. So a fresh profile shows
**"Mail authentication request"**. The script clears its "remember password"
check box (with no keyring, a failure to store the password counts as a
failed sign-in) and clicks **OK** with empty fields. The provider sends no
credentials when the account has no user
(`rust/crates/jmap-mail/src/connect.rs`). **Cancel** would mean "do not
authenticate", and the account would fail to open.

On some distributions Evolution also asks **"Do you want to make Evolution
your default email client?"**. The folder-popup and populate-once scripts
dismiss it.

The scripts make no assertions about these dialogs.

## Running it

```console
$ cmake -S . -B build -G Ninja
$ cmake --build build
$ sudo cmake --install build --component camel-provider
$ ci/gui-smoke.sh
```

`ci/install-deps-gui-smoke.sh` installs the extra packages: `evolution`,
`xvfb`, `openbox`, `python3-pyatspi`, `imagemagick` and `ffmpeg`. A D-Bus
daemon is also needed (`ci/install-deps-functional.sh`).

`JMAP_MOCKD` sets the mock binary (default
`build/cargo-target/release/jmap-mockd`). `GUI_SMOKE_PORT`,
`GUI_SMOKE_DISPLAY`, `GUI_SMOKE_WORKDIR`, `GUI_SMOKE_ARTIFACTS` and
`GUI_SMOKE_RECORDING_ROOT` set the rest, so that two runs on one machine do
not collide.

The scripts use plain `pyatspi`, not `dogtail`: one fixed tree shape does
not need dogtail's extra layer.

## CI

The `gui-smoke` job in `.github/workflows/ci.yml` and the three sibling jobs
below run on `workflow_dispatch` or on a pull request with the label
`run-gui-smoke-test`, on a plain `ubuntu-24.04` runner. They are slower than
the rest of the suite, so they do not run on every push. They are not in
`.gitlab-ci.yml`.

## Sibling check: the example-module main menu

`ci/gui-smoke-example-module.sh` and `ci/gui-smoke-example-module-assert.py`
(CI job `gui-smoke-example-module`). Same harness, different assertion: the
C `example-module` (`src/`) merges `"My Message Action..."`
(`src/m-mail-ui.c`) into Evolution's `Message` menu. GTK builds the menu
widgets when the mail shell view is created, so no click is needed.

This needs `example-module` installed where Evolution looks for modules. Its
install rule (`src/CMakeLists.txt`) has no `COMPONENT`, so use
`cmake --install build` without `--component`.

## Sibling check: the example-module folder popup

`ci/gui-smoke-example-module-folder-popup.sh` and
`ci/gui-smoke-example-module-folder-popup-assert.py` (CI job
`gui-smoke-example-module-folder-popup`). Evolution builds the
`mail-folder-popup` menu only when a folder is right-clicked, so the script
right-clicks the mock account's row with AT-SPI's synthetic pointer and then
looks for `"My Maildir Folder Action..."`.

A synthetic click needs two things that the other scripts do not:

- **A window manager.** Without one, Xvfb gives the window no screen
  position and `Component.getExtents` returns `G_MININT32`. The script starts
  `openbox`.
- **`DISPLAY` in the D-Bus activation environment.** `at-spi2-registryd`
  makes the `XTestFakeButtonEvent` call and is D-Bus-activated with its own
  environment, so the script runs `dbus-update-activation-environment
  DISPLAY`.

The item's sensitivity gate in `src/m-mail-ui.c` requires a `maildir`
provider, so it is always insensitive on this JMAP account. With
GtkUIManager (Evolution < 3.55) an insensitive item is still shown, so the
check asserts only that it is present. With EUIManager (Evolution 3.55 and
newer) a popup menu omits disabled actions, so the item is not shown and
this check cannot pass there. That is expected example-module behaviour, not
a bug. The CI job runs on Ubuntu 24.04 (Evolution 3.52).

## Sibling check: live-Evolution populate-once caveat

`ci/gui-smoke-collection-populate-once.sh` and
`ci/gui-smoke-collection-populate-once-assert.py` (CI job
`gui-smoke-collection-populate-once`). It checks that a new collection
account's first populate connects once, not twice. A
`"changed"` write from the first populate must not trigger a second one in
`jmap-backend-collection`'s `on_account_changed`.

The script writes four `.source` files: a collection account
(`[Collection] BackendName=jmap`) with contacts and calendars **off**, and
its three mail sources. With contacts or calendars on, independent EDS
clients open the book and calendar children, and their request counts vary
between runs. Only this Evolution opens the mail store, so `Mailbox/get`
requests in the mock's log count the connections.

A healthy connect makes two `Mailbox/get` calls (the store's
`connect_sync` and Evolution's folder tree refresh). A second populate
doubles that to four. The script waits for the inbox, waits 15 more
seconds, and requires exactly two.
