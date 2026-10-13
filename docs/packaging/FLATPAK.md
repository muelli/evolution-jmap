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

## Batch 6 step 1 ABI check (2026-10-13 UTC)

Batch 6 starts with an ABI check of the currently installed extension payload,
before replacing copied host artifacts with a source build in the Flatpak SDK.

### Command set

- Enumerated all extension modules and ran `ldd -v` for each inside the app
  sandbox:
  `timeout 180 flatpak run --user --command=sh org.gnome.Evolution -c 'ldd -v <module>'`
- Read bundled EDS package versions from runtime pkg-config files:
  `timeout 120 flatpak run --user --command=sh org.gnome.Evolution -c 'sed -n "s/^Version: //p" /app/lib/pkgconfig/{libedataserver-1.2,libebackend-1.2,libebook-contacts-1.2,libecal-2.0,camel-1.2}.pc'`
- Listed runtime SONAMEs for key EDS and Evolution libs:
  `timeout 120 flatpak run --user --command=sh org.gnome.Evolution -c 'ls -1 /app/lib/libedataserver-1.2.so* /app/lib/libcamel-1.2.so* /app/lib/libebook-contacts-1.2.so* /app/lib/libical-glib.so*'`

### Bundled EDS version

The app runtime reports `3.62.0` for all checked EDS/Camel pkg-config files:

- `libedataserver-1.2.pc`
- `libebackend-1.2.pc`
- `libebook-contacts-1.2.pc`
- `libecal-2.0.pc`
- `camel-1.2.pc`

### Unresolved dependencies by module

- `libebookbackendjmap.so`: `libedataserver-1.2.so.27`,
  `libebook-contacts-1.2.so.4`.
- `libecalbackendjmap.so`: `libical-glib.so.3`,
  `libedataserver-1.2.so.27`.
- `libcameljmap.so`: `libedataserver-1.2.so.27`, `libcamel-1.2.so.64`.
- `module-jmap-backend.so`: `libedataserver-1.2.so.27`.
- `module-jmap-configuration.so`: `libevolution-mail.so.0`,
  `libevolution-mail-composer.so.0`, `libevolution-shell.so.0`,
  `libevolution-util.so.0`, `libedataserver-1.2.so.27`,
  `libcamel-1.2.so.64`.

Runtime-provided SONAMEs in `/app/lib` are newer:

- `libedataserver-1.2.so.28`
- `libcamel-1.2.so.68`
- `libebook-contacts-1.2.so.5`
- `libical-glib.so.4.0`

For Evolution shell libs, `/app/lib` does not contain
`libevolution-{mail,mail-composer,shell,util}.so.0`; the runtime has
unversioned files under `/app/lib/evolution/` (for example
`/app/lib/evolution/libevolution-mail.so`).

### Symbol-version evidence from `ldd -v`

For all five extension modules, the module-level version requirements shown by
`ldd -v` are satisfied for resolved base toolchain symbols:

- `libgcc_s.so.1`: up to `GCC_4.2.0`
- `libc.so.6`: up to `GLIBC_2.34`

The hard failure remains unresolved SONAME dependencies, not a GLIBC symbol
version shortfall in the runtime.

Conclusion for Batch 6 step 1: the currently installed extension payload is not
loadable in `org.gnome.Evolution` runtime branch `50` because it links against
older EDS and Evolution SONAMEs. Batch 6 step 2 must build the modules against
the runtime/SDK ABI instead of reusing host-built package artifacts.

## Batch 6 step 2 source-build probe (2026-10-13 UTC)

This increment adds a source-build manifest draft and runs it in the local
Flatpak builder to verify what still blocks a full step-2 build.

### Added draft manifest

- File:
  `packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.from-source.yml`
- Uses `org.freedesktop.Sdk.Extension.rust-stable` and prepends its bin path
  so CMake can call cargo from the SDK extension.
- Configures CMake with:
  `-DCMAKE_INSTALL_PREFIX=${FLATPAK_DEST} -DFORCE_INSTALL_PREFIX=ON`
  so install destinations derived from pkg-config are rewritten into the
  extension payload tree.
- Installs only the five JMAP components (`book-backend`, `cal-backend`,
  `camel-provider`, `collection-backend`, `config-module`).

### Runtime and SDK evidence

- Header probe in app runtime sandbox:
  `timeout 120 flatpak run --user --command=sh org.gnome.Evolution -c 'find /app/include -maxdepth 2 -type d | head -n 40'`
  showed `evolution-data-server` and `evolution` headers under `/app/include`.
- SDK-only pkg-config probe:
  `timeout 120 flatpak run --user --command=sh org.gnome.Sdk//50 -c 'pkg-config --modversion libedataserver-1.2 ...'`
  failed, confirming EDS and Evolution pkg-config files are supplied by the app
  runtime side, not by bare `org.gnome.Sdk` alone.
- Source-build configure inside `flatpak-builder` succeeded and resolved all
  required modules at `3.62.0` (`evolution-shell-3.0`, `evolution-mail-3.0`,
  `libedata-book-1.2`, `camel-1.2`, `libebackend-1.2`).

### Current blocker for step 2

- Build command:
  `timeout 5400 flatpak-builder --user --install --force-clean --state-dir .flatpak-builder .build-source-test packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.from-source.yml`
- C/CMake phase passes, but Rust build fails in sandbox when cargo tries to hit
  crates.io:
  `Could not resolve host: index.crates.io`.

Conclusion for this probe: the source-build wiring is valid through CMake
configure, and the expected EDS and Evolution headers are visible from the app
runtime. The remaining step-2 blocker is Rust dependency vendoring for offline
Flatpak builds. The next step is to add vendored crate sources plus cargo
offline configuration to this manifest.

## Batch 6 step 2 offline-vendoring completion (2026-10-13 UTC)

This follow-up increment resolves the crates.io DNS blocker by vendoring Rust
dependencies in-tree and wiring cargo to offline mode inside the Flatpak build
sandbox.

### Changes applied

- Added cargo source replacement config:
  `packaging/flatpak/cargo-vendor-config.toml`.
- Added vendor workflow for `packaging/flatpak/vendor/` from:
  `timeout 7200 cargo vendor --locked --versioned-dirs ../packaging/flatpak/vendor`.
- `packaging/flatpak/vendor/` stays untracked in git for now and is generated
  locally before running `flatpak-builder`.
- Updated source-build manifest
  `packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.from-source.yml`:
  - Added `CARGO_NET_OFFLINE=true`.
  - Added SDK extension `org.freedesktop.Sdk.Extension.llvm21` and
    `LIBCLANG_PATH=/usr/lib/sdk/llvm21/lib` for bindgen.
  - Installed cargo config into `${CARGO_HOME}/config.toml` before build.
  - Kept CMake source build and component installs for four backend components.
  - Installed the Evolution config module artifact directly from
    `_flatpak-build/cargo-target/release/libjmap_config_module.so` to
    `${FLATPAK_DEST}/lib/evolution/modules/module-jmap-configuration.so`.

### Probe results

- First offline run no longer tried to resolve `index.crates.io`; cargo reached
  crate compilation from vendored sources.
- Next run failed in bindgen due missing `libclang` in default SDK path;
  resolved by adding LLVM SDK extension and `LIBCLANG_PATH`.
- Next run failed when `cmake --install --component config-module` tried to
  install packaging docs to `/usr/share/doc/evolution-jmap`; worked around in
  manifest by installing only the module `.so` artifact needed by this
  extension payload.
- Final run succeeded end to end:
  `timeout 5400 flatpak-builder --user --install --force-clean --state-dir packaging/flatpak/.flatpak-builder packaging/flatpak/.build-source-vendor packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.from-source.yml`
  built and installed `runtime/org.gnome.Evolution.Extension.evolution-jmap/x86_64/stable`.

## Batch 6 step 3 factory-load probe (2026-10-13 UTC)

This increment exercises the EDS factory processes inside the sandbox and
records their own loader output for the currently installed extension payload.

### Commands run

- `timeout 120 flatpak run --user --command=sh org.gnome.Evolution -c 'G_MESSAGES_DEBUG=all /app/libexec/evolution-source-registry'`
- `timeout 120 flatpak run --user --command=sh org.gnome.Evolution -c 'G_MESSAGES_DEBUG=all /app/libexec/evolution-addressbook-factory -r'`
- `timeout 120 flatpak run --user --command=sh org.gnome.Evolution -c 'G_MESSAGES_DEBUG=all /app/libexec/evolution-calendar-factory -r'`

Each command reaches startup and is then terminated by timeout after collecting
startup logs.

### Loader evidence from the factory logs

- `evolution-source-registry` reports:
  - `Failed to load module: .../module-jmap-backend.so`
  - `module_load: libedataserver-1.2.so.27: cannot open shared object file`
  - `camel_provider_list: Could not load .../libcameljmap.so: libedataserver-1.2.so.27: cannot open shared object file`
- `evolution-addressbook-factory` reports:
  - `Failed to load module: .../libebookbackendjmap.so`
  - `module_load: libedataserver-1.2.so.27: cannot open shared object file`
  - `camel_provider_list: Could not load .../libcameljmap.so: libedataserver-1.2.so.27: cannot open shared object file`
- `evolution-calendar-factory` reports:
  - `Failed to load module: .../libecalbackendjmap.so`
  - `module_load: libical-glib.so.3: cannot open shared object file`
  - `camel_provider_list: Could not load .../libcameljmap.so: libedataserver-1.2.so.27: cannot open shared object file`

No log line indicates a successful JMAP backend registration. The source
registry and both factories continue with built-in providers only.

Conclusion for Batch 6 step 3: on this extension payload, the JMAP backends do
not register in the sandboxed EDS factories because the module dependencies do
not match runtime SONAMEs. Interactive Evolution UI behavior remains unverified
in a running Evolution session, as required by the batch scope.

## Batch 6 step 4 Flathub submission requirements (2026-10-13 UTC)

This section lists what is still required before this extension could be
proposed as a Flathub contribution. It does not submit anything.

### 1. Vendored Rust sources and offline cargo build

- The source-build manifest currently reaches CMake configure but fails in cargo
  with `Could not resolve host: index.crates.io`.
- Flathub packaging therefore needs vendored crates committed in-tree and a
  cargo offline configuration wired into
  `org.gnome.Evolution.Extension.evolution-jmap.from-source.yml`.
- Acceptance check for this item: `flatpak-builder --user --install` completes
  without any network access during cargo dependency resolution.

### 2. Extension point naming and runtime matching

- `org.gnome.Evolution` declares the extension point
  `org.gnome.Evolution.Extension` with target directory
  `evolution/extensions` and `merge-dirs: lib;share`.
- The extension id must stay under that namespace,
  `org.gnome.Evolution.Extension.<suffix>`, and runtime branch must match the
  app branch (`stable` here).
- The current id `org.gnome.Evolution.Extension.evolution-jmap` and branch
  `stable` follow that rule. Keep this invariant in any submitted manifest.

### 3. Licensing metadata and review artifacts

- Keep SPDX headers in every new manifest and script file, consistent with this
  repository's GPL-3.0-or-later policy.
- Keep `LICENSES/GPL-3.0-or-later.txt` available in the extension payload, as
  the existing CPack packaging rules already do.
- Provide a concise review bundle with: exact source commit, manifest file,
  vendoring instructions, and the factory-load proof from Batch 6 step 3 that
  demonstrates successful backend registration after ABI-matched rebuild.

Current status at the end of Batch 6 step 4 plus this step-2 follow-up:
naming and branch constraints are documented, and an ABI-matched offline source
build now installs as a local extension runtime when vendored crates are staged
locally first. Submission remains pending on a committed Flathub-friendly
vendoring path and the factory-load proof from in-sandbox EDS processes after
this rebuild.
