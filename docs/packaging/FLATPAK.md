<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Flatpak extension feasibility notes (Batch 5)

Date: 2026-10-10 (UTC)

This note records Batch 5 evidence from the live Flathub manifest for
`org.gnome.Evolution`, then maps that evidence to this repository's module
layout.

## Scope checked

- Flathub repository: `https://github.com/flathub/org.gnome.Evolution`
- Flathub commit: `daf9916ab0020f6d37ff766319d1a079661e4774`
- Manifest file: `org.gnome.Evolution.json`
- Local install-dir rules: `cmake/Backends.cmake`

## Manifest findings

1. **An extension point is present and explicitly intended for Evolution
   add-ons.**
   - `org.gnome.Evolution.json` line `38` defines `add-extensions`.
   - The extension id is `org.gnome.Evolution.Extension` (line `39`).
   - The target directory is `evolution/extensions` (line `40`).
   - Flatpak merges extension `lib` and `share` trees into the app sandbox
     (`merge-dirs` line `42`) and adds extension `lib` to runtime linker
     lookup (`add-ld-path` line `41`).

2. **The in-sandbox EDS build is wired to the same extension location.**
   - The `evolution-data-server` module is configured with
     `-DEXTENSIONS_DIR=/app/evolution/extensions` (line `239`).
   - The manifest creates `/app/evolution/extensions` in `post-install`
     (line `265`).

3. **Evolution currently bundles `evolution-ews` directly in the app build,
   not as an out-of-tree extension package.**
   - The manifest has an in-tree module named `evolution-ews` (line `427`).

## Mapping to evolution-jmap modules

`cmake/Backends.cmake` installs this project into host-specific module
directories discovered via `pkg-config`:

- Address book backend dir from `libedata-book-1.2 backenddir` (line `12`),
  output `libebookbackendjmap.so` (line `31`).
- Calendar backend dir from `libedata-cal-2.0 backenddir` (line `42`),
  output `libecalbackendjmap.so` (line `55`).
- Camel provider dir from `camel-1.2 camel_providerdir` (line `67`),
  output `libcameljmap.so` plus `libcameljmap.urls` (lines `79`, `83`).
- Source-registry module dir from `libebackend-1.2 moduledir` (line `92`),
  output `module-jmap-backend.so` (line `108`).
- Evolution shell module dir from `evolution-shell-3.0 moduledir` and
  output `module-jmap-configuration.so` (lines `117`, `127`).

Given the Flathub `merge-dirs: lib;share` behavior and these install paths,
an `org.gnome.Evolution.Extension` package can place the five module artifacts
under `lib/...` inside the extension payload so they appear in the sandbox
module search paths.

## Runtime verification status in this session

Batch 5 step 2 requires `flatpak` and `flatpak-builder`. They are not present
in this runner image, and this user cannot install distro packages:

- `command -v flatpak`: not found
- `command -v flatpak-builder`: not found
- `sudo -n true`: denied for user `codex`

So this increment records source-level feasibility evidence, but does not yet
run a local extension build or install.

## Next commands once flatpak tooling is available

1. Create an extension manifest under `packaging/flatpak/` targeting
   `org.gnome.Evolution.Extension`.
2. Build with `flatpak-builder --user`.
3. Install beside `org.gnome.Evolution` and check from inside the sandbox:
   - `pkg-config --variable=backenddir libedata-book-1.2`
   - `pkg-config --variable=backenddir libedata-cal-2.0`
   - `pkg-config --variable=camel_providerdir camel-1.2`
   - `pkg-config --variable=moduledir libebackend-1.2`
   - `pkg-config --variable=moduledir evolution-shell-3.0`
4. Confirm the five JMAP module files are visible at those resolved paths.

Loading in a real interactive Evolution UI remains unverified in this runner.
