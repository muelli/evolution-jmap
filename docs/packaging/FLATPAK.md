<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Flatpak extension feasibility notes (Batch 5)

Dates: 2026-10-10 and 2026-10-13 (UTC)

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

## Runtime verification (Batch 5 step 2, 2026-10-13 UTC)

Step 2 is now exercised end to end on this VM with user-scoped Flatpak
installation only.

### Tooling and app install

- `flatpak --version`: `Flatpak 1.16.6`
- `flatpak-builder --version`: `flatpak-builder-1.4.8`
- Added Flathub remote for `--user` installation.
- Installed `org.gnome.Evolution` (`app/org.gnome.Evolution/x86_64/stable`).
- Installed `org.gnome.Sdk` branch `50` for local extension builds.

### Local extension manifest and payload

- Added local manifest:
  `packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.yml`
- The manifest builds extension id
  `org.gnome.Evolution.Extension.evolution-jmap` against runtime
  `org.gnome.Evolution` (`stable`) with sdk `org.gnome.Sdk//50`.
- For this verification run, payload files were copied from an existing local
  CPack Debian payload tree under `build/.../_CPack_Packages/.../usr/lib/...`
  into `packaging/flatpak/payload/lib/...`.
- Built and installed with:
  `flatpak-builder --user --install --force-clean --state-dir packaging/flatpak/.flatpak-builder packaging/flatpak/.build packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.yml`

### In-sandbox visibility checks

- `flatpak info --show-metadata org.gnome.Evolution.Extension.evolution-jmap`
  reports:
  - `[ExtensionOf] ref=app/org.gnome.Evolution/x86_64/stable`
  - `[ExtensionOf] runtime=org.gnome.Platform/x86_64/50`
- `flatpak info --show-extensions org.gnome.Evolution` lists the installed
  extension runtime `org.gnome.Evolution.Extension.evolution-jmap`.
- Inside the Evolution sandbox,
  `find /app/evolution/extensions/evolution-jmap -maxdepth 8 -type f` shows:
  - `lib/evolution-data-server/addressbook-backends/libebookbackendjmap.so`
  - `lib/evolution-data-server/calendar-backends/libecalbackendjmap.so`
  - `lib/evolution-data-server/camel-providers/libcameljmap.so`
  - `lib/evolution-data-server/camel-providers/libcameljmap.urls`
  - `lib/evolution-data-server/registry-modules/module-jmap-backend.so`
  - `lib/evolution/modules/module-jmap-configuration.so`

This confirms the local extension is mounted and the expected JMAP module
artifacts are visible to sandboxed Evolution in the extension tree.

### In-sandbox module load checks (Batch 5 step 2 follow-up, 2026-10-13 UTC)

To satisfy the queue requirement to check what sandboxed Evolution actually
loads, factories and the app entrypoint were executed with loader debug output.

- Source registry check:
  - `timeout 120 flatpak run --user --command=sh org.gnome.Evolution -c 'G_MESSAGES_DEBUG=all /app/libexec/evolution-source-registry --disable-migration 2>&1' | rg 'module_load|Failed to load module|camel_provider_list'`
  - Reported `Failed to load module` for
    `registry-modules/module-jmap-backend.so` because
    `libedataserver-1.2.so.27` is missing.
- Address book factory check:
  - `timeout 40 flatpak run --user --command=sh org.gnome.Evolution -c 'G_MESSAGES_DEBUG=all /app/libexec/evolution-addressbook-factory 2>&1' | rg 'module_load|Failed to load module|camel_provider_list'`
  - Reported `Failed to load module` for
    `addressbook-backends/libebookbackendjmap.so` because
    `libedataserver-1.2.so.27` is missing.
- Calendar factory check:
  - `timeout 40 flatpak run --user --command=sh org.gnome.Evolution -c 'G_MESSAGES_DEBUG=all /app/libexec/evolution-calendar-factory 2>&1' | rg 'module_load|Failed to load module|camel_provider_list'`
  - Reported `Failed to load module` for
    `calendar-backends/libecalbackendjmap.so` because `libical-glib.so.3` is
    missing.
- Evolution shell module check:
  - `timeout 40 flatpak run --user --command=sh org.gnome.Evolution -c 'G_MESSAGES_DEBUG=all /app/bin/evolution --quit 2>&1' | rg 'module_load|Failed to load module|camel_provider_list'`
  - Reported `Failed to load module` for
    `evolution/modules/module-jmap-configuration.so` because
    `libevolution-mail.so.0` is missing.

Dependency inspection inside the sandbox confirms an ABI mismatch:

- `ldd` on extension libraries reports unresolved links to older SONAMEs such
  as `libedataserver-1.2.so.27`, `libcamel-1.2.so.64`,
  `libebook-contacts-1.2.so.4`, and `libical-glib.so.3`.
- The Flatpak runtime currently provides newer SONAMEs such as
  `libedataserver-1.2.so.28`, `libcamel-1.2.so.68`,
  `libebook-contacts-1.2.so.5`, and `libical-glib.so.4`.

Conclusion: the local extension payload copied from host package artifacts is
mounted in the right place, but the modules are not loadable in this runtime.
For a shippable Flatpak extension, `evolution-jmap` must be built against the
same Flatpak SDK/runtime ABI instead of reusing host-built package payloads.

### Notes and limits

- `pkg-config` is not present in the app runtime sandbox, so path verification
  used direct in-sandbox file listing rather than `pkg-config --variable`.
- Loading in a real interactive Evolution UI remains unverified in this runner.
