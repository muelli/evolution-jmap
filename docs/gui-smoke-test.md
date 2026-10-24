# GUI smoke test: a real Evolution against the mock

The functional tests (`docs/functional-tests.md`) drive EDS through its client
API. They never use Evolution itself. The GUI smoke test starts the real
Evolution under `Xvfb` against `jmap-mockd`. It checks that a JMAP account
appears in the mail view with mail in it.

The test is a canary. It is not coverage. For scripted user tasks, see
`docs/gui-scenario-tests.md`.

## What `ci/gui-smoke.sh` does

1. It starts `jmap-mockd` on a scratch port.
2. It starts a private `Xvfb` and a private D-Bus session bus. It turns on
   AT-SPI (`gsettings set org.gnome.desktop.interface toolkit-accessibility
   true`). Everything runs in a temporary `HOME` and XDG tree. The script
   therefore cannot touch your own Evolution data.
3. It writes the three mail sources into that tree. They come from
   `docs/examples/jmap-mock-standalone-{mail,identity,transport}.source` (the
   files of `docs/manual-test-mail-provider.md`). A standalone account needs
   only the installed Camel provider.
4. It starts `evolution -c mail --force-online`. It records the display with
   `ffmpeg -f x11grab` into tmpfs (`$GUI_SMOKE_RECORDING_ROOT`, default
   `/dev/shm`).
5. It runs `ci/gui-smoke-assert.py`. This script uses AT-SPI to dismiss the
   one-time dialogs (below). Then it waits for a folder tree cell
   `JMAP mock mail` with a child `Inbox (N)`, where `N > 0`.
6. If the run fails, it saves these items in `$GUI_SMOKE_ARTIFACTS`: a
   screenshot, the recording, an AT-SPI tree dump and the Evolution and mock
   logs. A passing run leaves nothing.
7. It retries once with a fresh scratch tree before it reports a failure.

## The one-time dialogs of a fresh profile

A scratch `HOME` has no keyring and no saved credentials. Camel asks the
session to authenticate every account before its first connection, although
these sources request no credential. A fresh profile therefore shows
**"Mail authentication request"**. The script does these steps:

1. It clears the "remember password" check box. With no keyring, a failure to
   store the password counts as a failed sign-in.
2. It clicks **OK** with empty fields.

The provider sends no credentials if the account has no user
(`rust/crates/jmap-mail/src/connect.rs`). **Cancel** means "do not
authenticate". The account would then fail to open.

On some distributions, Evolution also asks **"Do you want to make Evolution
your default email client?"**. The folder-popup and populate-once scripts
dismiss this dialog.

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
daemon is also necessary (`ci/install-deps-functional.sh`).

`JMAP_MOCKD` sets the mock binary (default
`build/cargo-target/release/jmap-mockd`). `GUI_SMOKE_PORT`,
`GUI_SMOKE_DISPLAY`, `GUI_SMOKE_WORKDIR`, `GUI_SMOKE_ARTIFACTS` and
`GUI_SMOKE_RECORDING_ROOT` set the other values. With them, two runs on one
machine do not collide.

The scripts use plain `pyatspi`, not `dogtail`. The scripts use one fixed tree
shape. They do not need the extra layer of dogtail.

### On Evolution 3.60

Ubuntu 24.04 has Evolution 3.52. To run the same test against Evolution and
EDS 3.60, use this command:

```console
$ ci/gui-smoke-fedora.sh
```

If `podman` is installed, the script runs in the pinned Fedora image that
`ci/rpm.sh` also uses. It builds the tree into `build-gui-smoke-fedora/`,
installs the Camel provider and runs `ci/gui-smoke.sh`. You do not need root.
`ci/install-deps-gui-smoke-rpm.sh` installs the packages in the image. Use
`ci/gui-smoke-fedora.sh --host` in a job that already runs in that image. The
artifacts of a failed run go to `build-gui-smoke-fedora/artifacts`, or to
`GUI_SMOKE_ARTIFACTS` if you set it.

## CI

The `gui-smoke` job in `.github/workflows/ci.yml`, the `gui-smoke-fedora` job
and the three sibling jobs below run on `workflow_dispatch` or on a pull request with the label
`run-gui-smoke-test`. They run on a plain `ubuntu-24.04` runner. They are
slower than the rest of the suite. They therefore do not run on every push.
They are not in `.gitlab-ci.yml`.

## Sibling check: the example-module main menu

The files are `ci/gui-smoke-example-module.sh` and
`ci/gui-smoke-example-module-assert.py` (CI job `gui-smoke-example-module`).
This check uses the same harness with a different assertion. The C
`example-module` (`src/`) merges `"My Message Action..."`
(`src/m-mail-ui.c`) into the `Message` menu of Evolution. GTK builds the menu
widgets when the mail shell view is created. No click is necessary.

This check needs `example-module` installed where Evolution looks for
modules. Its install rule (`src/CMakeLists.txt`) has no `COMPONENT`. Use
`cmake --install build` without `--component`.

## Sibling check: the example-module folder popup

The files are `ci/gui-smoke-example-module-folder-popup.sh` and
`ci/gui-smoke-example-module-folder-popup-assert.py` (CI job
`gui-smoke-example-module-folder-popup`). Evolution builds the
`mail-folder-popup` menu only when the user right-clicks a folder. The script
therefore right-clicks the row of the mock account with the synthetic pointer
of AT-SPI. Then it looks for `"My Maildir Folder Action..."`.

A synthetic click needs two things that the other scripts do not need:

- **A window manager.** Without a window manager, Xvfb gives the window no
  screen position, and `Component.getExtents` returns `G_MININT32`. The script
  starts `openbox`.
- **`DISPLAY` in the D-Bus activation environment.** `at-spi2-registryd` makes
  the `XTestFakeButtonEvent` call. D-Bus activates it with its own
  environment. The script therefore runs `dbus-update-activation-environment
  DISPLAY`.

The sensitivity gate of the item in `src/m-mail-ui.c` requires a `maildir`
provider. The item is therefore always insensitive on this JMAP account. The
two UI managers behave differently:

- With GtkUIManager (Evolution < 3.55), an insensitive item is still shown.
  The check asserts only that the item is present.
- With EUIManager (Evolution 3.55 and newer), a popup menu omits disabled
  actions. The item is not shown, and this check cannot pass there.

This is the expected behavior of example-module. It is not a bug. The CI job
runs on Ubuntu 24.04 (Evolution 3.52).

## Sibling check: live-Evolution populate-once caveat

The files are `ci/gui-smoke-collection-populate-once.sh` and
`ci/gui-smoke-collection-populate-once-assert.py` (CI job
`gui-smoke-collection-populate-once`). The check shows that the first populate
of a new collection account connects once, not twice. A `"changed"` write from
the first populate must not start a second populate in `on_account_changed` of
`jmap-backend-collection`.

The script writes four `.source` files: a collection account
(`[Collection] BackendName=jmap`) with contacts and calendars **off**, and its
three mail sources. If contacts or calendars are on, independent EDS clients
open the book and calendar children. Their request counts vary between runs.
Only this Evolution opens the mail store. The `Mailbox/get` requests in the
log of the mock server therefore count the connections.

A healthy connect makes two `Mailbox/get` calls: the `connect_sync` of the
store and the folder tree refresh of Evolution. A second populate doubles this
to four. The script waits for the inbox, then waits 15 more seconds. It
requires exactly two calls.
