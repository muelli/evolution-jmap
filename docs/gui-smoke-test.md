# GUI smoke test: a real Evolution against the mock

M9 layer 1 (`docs/functional-tests.md`) drives EDS through its client API —
close to what a user sees, but never through Evolution itself. This is the
other half the roadmap calls M9 Tier 2: launch the real GUI under `Xvfb`
against `jmap-mockd`, and check the one thing layer 1 cannot — that a JMAP
account actually appears in Evolution's own mail view with mail in it, the
way a person opening the application would see it.

It is one test, deliberately: a canary, not coverage. A full scripted GUI
suite (composing, reading, account setup by clicking through dialogs) is out
of scope for this repository.

## What it does

`ci/gui-smoke.sh`:

1. Starts `jmap-mockd` on a scratch port.
2. Starts a private `Xvfb`, a private D-Bus session bus, and turns on AT-SPI
   (`gsettings set org.gnome.desktop.interface toolkit-accessibility true`) —
   all inside one throwaway `HOME`/XDG tree, so the run cannot see or corrupt
   a developer's own Evolution data, exactly as `jmap-functional`'s `Session`
   does for the headless tests.
3. Writes the three hand-written mail sources
   `docs/examples/jmap-mock-standalone-{mail,identity,transport}.source`
   describe into that tree's `evolution/sources/` — the same files
   `docs/manual-test-mail-provider.md` has a human copy by hand, used here
   unattended. A standalone account rather than one hung off the collection
   backend: it needs no address-book or calendar component installed, only
   the Camel provider, and this test is about Evolution showing mail, not
   about the collection backend's fan-out (M9 layer 1 and
   `jmap-backend-collection`'s own tests already cover that).
4. Launches `evolution -c mail --force-online` in that environment, while
   recording the `Xvfb` display with `ffmpeg -f x11grab` into a tmpfs file
   (`$GUI_SMOKE_RECORDING_ROOT`, default `/dev/shm`) — cheap to write and
   never touched unless the attempt fails.
5. Runs `ci/gui-smoke-assert.py` under the same environment, which drives the
   AT-SPI tree: dismisses the two one-time dialogs a keyring-less profile's
   first connection shows (below), then polls the mail folder tree for a
   cell named `JMAP mock mail` (the account) and one of its children matching
   `Inbox (N)` with `N > 0` — the account appearing and its inbox holding the
   two messages `jmap-mockd` seeds at startup.
6. On failure, saves a screenshot, the recording, a full AT-SPI tree dump,
   and both Evolution's and the mock's logs under `$GUI_SMOKE_ARTIFACTS`
   (default a subdirectory of the run's own temp dir). A passing run leaves
   nothing — the recording is discarded from tmpfs the same as everything
   else.
7. Retries once on failure, with an entirely fresh scratch tree, before
   reporting failure — accepted as "a little flaky" by the roadmap's own
   words for this milestone; the artifacts above are what a flake or a real
   regression leaves behind to tell them apart.

## The two dialogs every fresh profile shows once

Both are artifacts of a scratch `HOME` with no keyring daemon and no
previously-accepted credential, not of anything the account's `.source`
files ask for — `Method=none` and no `User=` mean this backend requests no
credential, but Camel's generic `connect_sync` still starts every account
by asking the session to authenticate it, unconditionally, and EDS's session
answers by prompting once before the first connection has a saved answer
to try:

- **"Mail authentication request"** — the account editor's generic password
  prompt. Clicking **OK** with both fields blank is enough: this backend's
  `open_mail` (`rust/crates/jmap-mail/src/connect.rs`) sends no credentials
  when the account names no user, regardless of what was typed into this
  dialog, so blank-and-OK reaches the mock exactly as the standalone recipe
  promises. Clicking **Cancel** instead does *not* reach the mock — it is
  read as declining to authenticate at all, and the account fails to open.
- **gcr-prompter's "Choose password for new keyring"** — shown only because
  the first dialog's "add this password to your keyring" checkbox is on by
  default and this scratch profile has no keyring service to answer it
  quietly. `ci/gui-smoke-assert.py` clicks **Cancel** on it; declining to
  save a credential nothing needs does not affect the connection already
  under way.

Neither dialog is asserted about — they are dismissed so the account under
test can reach the state that is.

## Why `pyatspi` rather than `dogtail`

The roadmap names AT-SPI/dogtail as the tooling family for this tier.
`dogtail` is a convenience layer over `pyatspi` — predicates, retries,
logging — built for writing many tests against evolving UIs. This is one
script asserting one fixed tree shape, so the plain `pyatspi` Python module
(`python3-pyatspi`) says everything needed without the extra dependency.

## Running it

```console
$ cmake -S . -B build
$ cmake --build build
$ sudo cmake --install build --component camel-provider
$ ninja -C build   # or: cargo build --release -p evolution-jmap-mock
$ ci/gui-smoke.sh
```

Needs, beyond the build: `evolution`, `xvfb`, `python3-pyatspi`, `imagemagick`
and `ffmpeg` (`ci/install-deps-gui-smoke.sh` installs all five) — plus a
private D-Bus session (`dbus-daemon`, already required by
`ci/install-deps-functional.sh`).

`JMAP_MOCKD` overrides where the script looks for the built binary (default
`build/cargo-target/release/jmap-mockd`, `cmake/Rust.cmake`'s
`CARGO_TARGET_DIR`). `GUI_SMOKE_PORT`, `GUI_SMOKE_DISPLAY`,
`GUI_SMOKE_WORKDIR`, `GUI_SMOKE_ARTIFACTS` and `GUI_SMOKE_RECORDING_ROOT`
override the rest, mainly so two runs on one machine do not collide.

## CI

Gated exactly like the M9 layer 1 job, and for the same reason: slower than
the rest of the suite, and worth spending deliberately rather than on every
push. The `gui-smoke` job in `.github/workflows/ci.yml` runs on
`workflow_dispatch` or a pull request labelled `run-gui-smoke-test`, on a
bare `ubuntu-24.04` runner — `ci/install-deps-gui-smoke.sh` installs
Evolution itself the same way `ci/install-deps-functional.sh` installs the
EDS runtime, so this does not touch the shared CI image
(`Containerfile.ci`/`ci-image.yml`) either.

Not wired into `.gitlab-ci.yml`, for the same unverified-elsewhere reason
`docs/functional-tests.md` gives for layer 1.

## Sibling check: the example-module menu caveat

This file deliberately keeps the canary above a single script. A separate
caveat, that the packaged `example-module`'s merged menu items actually
appear in a real Evolution session rather than merely linking cleanly
against both UI eras, gets its own sibling script instead of growing this
one: `ci/gui-smoke-example-module.sh` plus
`ci/gui-smoke-example-module-assert.py`.

Same harness (private `Xvfb`/D-Bus/XDG tree against `jmap-mockd`, retry
once), different assertion: it polls the AT-SPI tree for the mail-message-menu
merge (`"My Message Action..."`, `src/m-mail-ui.c`) under Evolution's
`Message` menu, present as soon as the mail shell view exists since GTK
merges the `EUIManager`/`GtkUIManager` XML into real widgets at shell-view
construction, with no menu click needed. Only that half of the caveat is
checked; the sibling mail-folder-popup item merges into a context menu
built on demand when a folder is right-clicked, checked by its own sibling
script below.

Needs everything `ci/gui-smoke.sh` needs, plus `example-module` installed
where Evolution scans modules: `cmake --install build` with **no**
`--component` filter, since `example-module`'s install rule
(`src/CMakeLists.txt`) carries no `COMPONENT` of its own, so a
camel-provider-only install leaves it uninstalled. Confirmed 2026-10-09
against this project's EDS 3.52 (Ubuntu 24.04, rootless-podman container
since this runner has no root to install Evolution/Xvfb natively): the
assertion passes on a real build, and a negative control (the same run with
the sought menu-item name swapped for one that cannot exist) fails as
expected, ruling out a vacuously-passing tree walk. The EDS 3.60.2 leg
(item 71's podman recipe) is left for a follow-up run.

## Sibling check: the example-module mail-folder-popup caveat

The other half of the same caveat: `ci/gui-smoke-example-module-folder-popup.sh`
plus `ci/gui-smoke-example-module-folder-popup-assert.py`. Evolution builds
the `mail-folder-popup` context menu fresh via `e_ui_manager_create_item`
only when a folder is actually right-clicked, well after the shell view and
its extensions construct, so a static tree walk (the sibling check above)
proves nothing for this half: the item is absent from the tree until the
popup is actually triggered. This script drives a real right-click through
AT-SPI's synthetic pointer at the mock account's own row in the folder tree,
then polls for `"My Maildir Folder Action..."` in the resulting popup. On
GtkUIManager (EDS < 3.55) the item is expected to appear but stay
insensitive (its own sensitivity gate in `src/m-mail-ui.c` requires a
`maildir` Camel provider, which this JMAP account never is, and
`m_utils_enable_actions` only ever toggles sensitivity, never visibility on
that era), so only presence is asserted there, not enabled state. This does
**not** hold on EUIManager (EDS >= 3.55); see the 2026-10-09 EDS 3.60.2
entry below.

A real synthetic right-click needs two things neither `ci/gui-smoke.sh` nor
the sibling script above needs, since both of those only ever drive AT-SPI's
Action interface (a direct, in-process ATK call with no X input involved):

- **A window manager.** Without one, Xvfb never assigns Evolution's toplevel
  an absolute screen position, so `Component.getExtents` comes back as
  GLib's "unknown" sentinel (`G_MININT32`) rather than a usable coordinate.
  The script starts a minimal `openbox` for this (`ci/install-deps-gui-smoke.sh`
  now installs it alongside the rest).
- **`DISPLAY` pushed into the D-Bus activation environment.** `at-spi2-registryd`,
  which owns the actual `XTestFakeButtonEvent` call the synthetic click makes,
  is D-Bus-activated with its own environment, not the caller's, so without
  `dbus-update-activation-environment DISPLAY` the click silently reaches no
  display at all.

Confirmed 2026-10-09 against EDS 3.52 (same rootless-podman Ubuntu 24.04
container as the sibling check): passes twice in a row, and a negative
control (the sought menu-item name swapped for one that cannot exist) fails
both attempts as expected.

## The EDS 3.60.2 leg of the mail-folder-popup check: confirmed, not a bug

Run 2026-10-09 in item 71's pinned-Fedora podman recipe (the same container
the EUIManager main-menu finding above used, with the full `evolution`
package, Xvfb, AT-SPI, openbox and `cmake --install` of the built module —
not just the `-devel` headers `ci/eds-matrix.sh` normally installs). Two
portability bugs in the assert script itself were found and fixed first,
independent of the actual caveat: AT-SPI's application name for Evolution is
`"evolution"` in the minimal rootless-podman Ubuntu container the 3.52 leg
uses, but `"org.gnome.Evolution"` (its GApplication id) in this full-desktop
Fedora container, and the push-button role name itself is `"push button"` on
the former's older at-spi2-core and plain `"button"` on the latter's newer
one — `find_app` now accepts either application name and the dialog-button
lookups (`find_button`) no longer filter on role at all, name alone being
specific enough for both callers. A third, previously-unseen one-time dialog
("Do you want to make Evolution your default email client?") also had to be
dismissed, present only because this container's full `evolution` package
pulls in `desktop-file-utils`/`shared-mime-info`, absent from the minimal
Ubuntu container's apt list.

With all three fixed, the check runs for real and **does not pass, for a
real and already-documented reason, not a bug**: `src/m-mail-ui.c`'s own
sensitivity gate (`REQUIRE_SERVICE_PROTOCOL "maildir"`) never matches this
JMAP account on any node, so `my-mail-ui-folder-action` is unconditionally
insensitive here — true on both UI eras. On GtkUIManager (EDS 3.52) an
insensitive item still renders, greyed out, which is what the EDS 3.52 leg
above observes. On EUIManager (EDS >= 3.55) it does not: real upstream
source settles this precisely (`evolution` at tag `3.60.2`,
`src/e-util/e-ui-manager.c`'s `eum_traverse_menu`, the two identical gates at
lines 2099-2100 and 2138-2139,
`e_ui_action_is_visible (action) && (!is_popup || g_action_get_enabled (...))`):
a menu built with `is-popup='true'` (`mail-folder-popup` is one) omits a
disabled action entirely, where a non-popup menu (`main-menu`) only checks
visibility and leaves a disabled action in place, greyed out. Confirmed
directly: a real right-click on the account's row opens a real popup
(`New Folder…`, `Refresh`, `Manage Subscriptions`, `Disable Account`,
`Properties`, matching `evolution-mail.eui`'s own `mail-folder-popup` menu
definition read off this container's installed copy), correctly missing
every item this account's state leaves disabled — ours among them, not
singled out.

This is not a defect to fix: `REQUIRE_SERVICE_PROTOCOL "maildir"` is
deliberate example-module demo code showing a menu item gated on the
selected account's provider, and this test's own mock account is JMAP by
construction, never maildir, on either UI era. Changing the gate to also
match `jmap` would serve this test alone and misrepresent what the demo
code demonstrates; it is not something a real caveat in item 74 asked for.
What item 74 actually asked — does the module's `my-mail-ui-folder-action`
reach `mail-folder-popup` and register correctly on EUIManager — is already
answered yes, independent of this test: it shares one
`e_ui_manager_add_actions_with_eui_data` call and one action group
(`example-module-mail`) with `my-mail-ui-message-action`, and the EUIManager
main-menu finding above already confirmed that call succeeds and the group
exists on this exact build (`e_ui_manager_has_action_group` true). The
remaining gap is cosmetic, not functional: this AT-SPI check cannot observe
the item in its *enabled* state without a real `maildir` Camel account,
which is out of scope for a JMAP-only harness. Item 74 is fully closed on
both UI eras; do not re-queue chasing a maildir fixture for this one script.

## Sibling check: item 66's live-Evolution populate-once caveat

`ci/gui-smoke-collection-populate-once.sh` plus
`ci/gui-smoke-collection-populate-once-assert.py`, wired into a new
`gui-smoke-collection-populate-once` CI job. Item 66 fixed a real bug
(`jmap-backend-collection`'s `on_account_changed` repopulating an account in
answer to a `"changed"` write its own first populate made), with Rust-level
evidence pinning the fix, but the live-Evolution sequence itself stayed
unverified: there is no session-bus registry in a unit test, so nothing had
driven a real brand-new account through `evolution-source-registry` and
checked the server only saw one login.

Unlike every other check on this page, this one needs a collection account
(`[Collection] BackendName=jmap`), not a plain `[Mail Account]`: the
`on_account_changed`/`wants_repopulate` logic item 66 touches lives entirely
in `jmap-backend-collection`, which a standalone mail source never
constructs. The fixture the script writes (four small `.source` files,
inline rather than copied from `docs/examples/`, since this exact
combination is not one of the manual-test recipes
`docs/manual-test-collection-backend.md` documents for a human to copy)
turns contacts and calendars **off**: a first attempt with both on found the
most obvious oracle, counting `AddressBook/get`/`Calendar/get` requests in
`jmap-mock`'s own log, too noisy to use — `jmap-backend-collection`'s own
`fan_out` auto-creates and exports a book/calendar child the moment either
part is enabled, and each gets opened by its own independently-scheduled EDS
client (evolution-addressbook-factory's autocompletion, the calendar
reminder watcher) with no connection to this account's own populate, so
their request counts did not agree between two otherwise-identical runs.
Mail has no such confound in this headless profile: the one Evolution
instance under test is the only client that ever opens this account's mail
store, so `Mailbox/get` traffic maps directly onto how many times it
connected.

Its healthy baseline is not 1, though, and this script does not assume it
is: a control run of `ci/gui-smoke.sh`'s own plain standalone mail account
(no collection backend, no `on_account_changed` in the picture at all)
empirically shows `Mailbox/get` firing **twice** on every healthy single
connect — once for the store's own `connect_sync`, once more for
Evolution's own folder-tree refresh right after. A bugged second populate
doubles that again, to 4. The script waits for the account's inbox to prove
the healthy first login happened, gives any erroneous second one a 15-second
grace window to show up, then asserts the mock's log shows exactly 2, not 4,
`Mailbox/get` calls.

Confirmed 2026-10-09 in the same rootless-podman Ubuntu 24.04 container
(EDS 3.52) the example-module checks use: passed twice in a row at 2, and a
negative control (the expected count swapped for one that cannot occur)
failed as expected, ruling out a vacuous pass.

**EDS 3.60.2 leg confirmed 2026-10-09, same container shape the EUIManager
and folder-popup 3.60.2 checks used (item 71's pinned-Fedora podman recipe,
full `evolution` package, Xvfb/AT-SPI/dbus installed, module and
`jmap-mockd` built against the container's real 3.60.2 headers).** Two
portability fixes needed in the assert script, both already known from the
folder-popup EDS 3.60.2 leg and applied the same way rather than
rediscovered: `find_app` now accepts either `"evolution"` (the minimal
Ubuntu container's AT-SPI application name) or `"org.gnome.Evolution"` (this
container's GApplication id), and the button lookups no longer filter on
role (`"push button"` there, plain `"button"` here). The same one-time
"Do you want to make Evolution your default email client?" dialog the
folder-popup leg hit also showed up here and needed the same
`"Do not change settings"` dismissal. With those fixed, the check passed
twice in a row at the same healthy count of 2, and a negative control
(expected count swapped to 999) failed both attempts as expected. No
`EDS_CAMEL_PROVIDER_DIR` workaround was needed this time: this container's
`cmake --install --component camel-provider` already lands the module in
the exact directory `libcamel` reports via its own compiled-in default.

Item 66 is now fully confirmed on both EDS legs. Item 93 stays CLAIMABLE
only on item 63 (editing a JMAP account's host must trigger a fresh
authenticate).
