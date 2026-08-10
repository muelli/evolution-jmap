# evolution-jmap

JMAP support for [GNOME Evolution](https://gitlab.gnome.org/GNOME/evolution),
written in Rust — mail, contacts, and calendars over a single modern
protocol, structured like
[evolution-ews](https://gitlab.gnome.org/GNOME/evolution-ews).

## Status

**Round 1 (current): protocol layer, test-driven against a mock server.**

| Component | Crate | State |
|---|---|---|
| JMAP protocol types (RFC 8620, RFC 8621, RFC 9610, JMAP Calendars draft) | `evolution-jmap-proto` | ✅ |
| Blocking JMAP client (session discovery, batching, back-references, blobs) | `evolution-jmap-client` | ✅ |
| Stateful in-memory mock JMAP server (`jmap-mockd`) | `evolution-jmap-mock` | ✅ |
| Evolution UI module template (Rust port of the wiki example) | `example-module` | ✅ |
| EDS backends (address book, calendar), Camel provider, account setup | — | next rounds |

The test suite covers sending and receiving email (including
`EmailSubmission` with envelope derivation and `onSuccessUpdateEmail`),
contacts CRUD, calendar CRUD, incremental sync via `/changes`, blob
upload/download, authentication, and protocol edge cases — 42 tests
against fixtures from the RFC examples and a stateful mock server on
localhost.

## Building and testing

Rust is pinned via [rust/rust-toolchain.toml](rust/rust-toolchain.toml)
(rustup installs it automatically). The JMAP crates need no system
libraries:

```bash
cd rust && cargo test
```

The full build (including the Evolution module template) needs Evolution
3.52 development headers (`evolution-dev`, `libecal2.0-dev`, … — see
[Containerfile.ci](Containerfile.ci) for the complete list):

```bash
cmake -S . -B build -G Ninja
cmake --build build
ctest --test-dir build
```

### Building the package

The five modules also install as one Debian package, built from that same
install tree:

```bash
cpack -G DEB --config build/CPackConfig.cmake -B build/package
sudo apt install ./build/package/evolution-jmap_*.deb
```

Its dependencies are derived from the built objects by `dpkg-shlibdeps`
rather than written down, so the package pins the ABI it was compiled
against — `libevolution (>= 3.52.3), libevolution (<< 3.53)` and the
matching evolution-data-server sonames. That is the contract for modules
Evolution and EDS dlopen: build them against the versions they will be
installed alongside. `ctest -R package-deb` builds the package and checks
its contents and control fields.

### Poking at the mock server

```bash
cd rust && cargo run -p evolution-jmap-mock --bin jmap-mockd
```

then discover the session the way any JMAP client would:

```bash
curl -s http://127.0.0.1:8080/.well-known/jmap | jq .
```

### Trying the address book backend in Evolution

Against that same mock server, with a hand-written account:
[docs/manual-test-book-backend.md](docs/manual-test-book-backend.md).

## Architecture

```
rust/crates/
├── jmap-proto/      pure serde types; no I/O; fixture round-trip tests
├── jmap-client/     blocking client; HTTP behind a Transport trait with a
│                    cancellation hook (future GCancellable seam); ureq default
├── jmap-mock/       stateful mock server (tiny_http): auth, id allocation,
│                    per-type state + changes log, introspectable outbox
└── example-module/  Evolution UI module in Rust (hand-written FFI template)
```

Planned next rounds, mirroring evolution-ews (design notes in the commit
history): `libebookbackendjmap.so` (EBookMetaBackend),
`libecalbackendjmap.so` (ECalMetaBackend), `libcameljmap.so`
(CamelStore/Transport), `module-jmap-backend.so` (ECollectionBackend),
`module-jmap-configuration.so` (account setup UI). FFI via
`glib-sys`/`gobject-sys` plus bindgen-generated EDS bindings; integration
tests against [Stalwart](https://stalw.art/), which implements the full
JMAP suite including contacts and calendars.

## CI, reproducibility, transparency

- One definition of the build, run everywhere: the [`ci/`](ci/) scripts
  (`checks.sh`, `build.sh`, `reproducible.sh`) are the single source of
  truth. GitHub Actions and GitLab CI are thin wrappers that call them,
  the autonomous agent runs `ci/checks.sh` before every push, and you can
  run the same script on your laptop — so "green" means the same thing
  in all four places and the platforms can't drift.
- A dedicated cacheless CI job builds the mock twice in different paths
  and fails on any checksum difference (`--remap-path-prefix`,
  `SOURCE_DATE_EPOCH`, pinned toolchain, committed lockfile).
- Releases ship `SHA256SUMS` plus Sigstore provenance recorded in the
  public Rekor log — see
  [docs/verifying-artifacts.md](docs/verifying-artifacts.md).
- `cargo deny` gates dependency licenses (GPLv3-compatible only);
  [REUSE](https://reuse.software/) tracks per-file licensing.

## License

New code: GPL-3.0-or-later. The example-module files (C sources from the
GNOME wiki and their Rust port) remain LGPL-2.1-or-later (Red Hat
copyright). See [REUSE.toml](REUSE.toml) and [LICENSES/](LICENSES/).
