# GUI smoke test: a real Evolution against the mock

M9 layer 1 (`docs/functional-tests.md`) drives EDS through its client API —
close to what a user sees, but never through Evolution itself. This is the
other half the roadmap calls M9 Tier 2: launch the real GUI under `Xvfb`
against `jmap-mockd`, and check the one thing layer 1 cannot — that a JMAP
account actually appears in Evolution's own mail view with mail in it, the
way a person opening the application would see it.

It is one test, deliberately. The roadmap calls it a canary, not coverage:
a full scripted GUI suite (composing, reading, account setup by clicking
through dialogs) is out of scope for this repository — see `docs/ROADMAP.md`
M9 for where that belongs instead.

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
