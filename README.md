# evolution-jmap

JMAP support for [GNOME Evolution](https://gitlab.gnome.org/GNOME/evolution),
written in Rust — mail, contacts, and calendars over a single modern
protocol, structured like
[evolution-ews](https://gitlab.gnome.org/GNOME/evolution-ews).

## Status

**All four backends work end-to-end in real Evolution, tagged `v0.2.0`, and
have been operator-verified against a real JMAP server.**

| Component | Crate(s) | State |
|---|---|---|
| JMAP protocol types (RFC 8620, RFC 8621, RFC 9610, JMAP Calendars draft) | `evolution-jmap-proto` | ✅ |
| Blocking JMAP client (session discovery, SRV autodiscovery, batching, back-references, blobs, OAuth2) | `evolution-jmap-client` | ✅ |
| Stateful in-memory mock JMAP server (`jmap-mockd`) | `evolution-jmap-mock` | ✅ |
| Address book backend (`EBookMetaBackend`) | `jmap-backend-book(-module)`, `jmap-book-sync` | ✅ |
| Calendar backend (`ECalMetaBackend`) | `jmap-backend-cal(-module)`, `jmap-cal-sync` | ✅ |
| Mail provider (Camel store/transport) | `jmap-mail(-sync)` | ✅ |
| Collection backend (one account, all three above) | `jmap-backend-collection(-module)`, `jmap-collection-sync` | ✅ |
| Account-setup UI (`module-jmap-configuration.so`), incl. OAuth2 | `jmap-config(-module)` | ✅ |
| JMAP-only UI features (vacation autoresponder, scheduled send, snooze) | `jmap-ui` | ✅ |
| vCard/iCalendar mapping (JSContact/JSCalendar) | `jmap-vcard`, `jmap-ical` | ✅ |
| Evolution UI module template (Rust port of the wiki example, unused by the above) | `example-module` | ✅ |

Address book, calendar, mail send/receive, and OAuth2 via JMAP SRV
autodiscovery (`_jmap._tcp`) have all round-tripped and persisted against a
real deployment (Stalwart, then Fastmail), confirmed by the operator in real
Evolution — not just against the mock. The mapping crates carry ~39k lines of
round-trip and fuzz tests (`jmap-vcard`/`jmap-ical`); a 2026-08-29 measured
spike into externalising that layer onto the `calcard` crate's own converter
found it a worse fit (15% pass rate against our acceptance suite — see
[docs/CALCARD-SEMANTIC-SPIKE.md](docs/CALCARD-SEMANTIC-SPIKE.md)) and kept
the hand-written mapping. `docs/ROADMAP.md` tracks what's next.

A `jmap-ui` module puts three JMAP-only features into Evolution's own UI,
which has no concept of them: a vacation-autoresponder page in the account
editor, scheduled send in the composer, and snooze in the message-list
context menu. Each is gated on the account's server actually offering the
feature; see [docs/manual-test-ui-features.md](docs/manual-test-ui-features.md).

The test suite covers sending and receiving email (including
`EmailSubmission` with envelope derivation and `onSuccessUpdateEmail`),
contacts CRUD, calendar CRUD, incremental sync via `/changes`, blob
upload/download, authentication (Basic, Bearer, and OAuth2), and protocol
edge cases, plus functional tests that drive the built `.so` modules against
a real EDS registry and GUI-smoke tests against a real Evolution/Xvfb — well
over a thousand tests in total (`ctest --test-dir build` reports the current
count).

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

Packaging this for a distro? [docs/PACKAGING.md](docs/PACKAGING.md) is
addressed to you: what's already lintian-clean and CI-checked, what the
`debian/` skeleton does and doesn't solve, and the one open question
(crate vendoring) an official upload needs answered.

### Poking at the mock server

```bash
cd rust && cargo run -p evolution-jmap-mock --bin jmap-mockd
```

then discover the session the way any JMAP client would:

```bash
curl -s http://127.0.0.1:8080/.well-known/jmap | jq .
```

### Trying backends manually in Evolution

Against that same mock server, with hand-written accounts:
- Address book backend: [docs/manual-test-book-backend.md](docs/manual-test-book-backend.md)
- Calendar backend: [docs/manual-test-cal-backend.md](docs/manual-test-cal-backend.md)
- Mail provider: [docs/manual-test-mail-provider.md](docs/manual-test-mail-provider.md)
- Collection backend (all three together): [docs/manual-test-collection-backend.md](docs/manual-test-collection-backend.md)

## Architecture

```
rust/crates/
├── jmap-proto/                pure serde types; no I/O; fixture round-trip tests
├── jmap-client/                blocking client; HTTP behind a Transport trait with a
│                                cancellation hook (GCancellable seam); ureq default;
│                                SRV autodiscovery via an injectable Resolver seam
├── jmap-mock/                  stateful mock server (tiny_http): auth (incl. OAuth2),
│                                id allocation, per-type state + changes log, outbox
├── jmap-vcard/, jmap-ical/     JSContact<->vCard and JSCalendar<->iCalendar mapping
├── jmap-backend-core/          shared EDS/GObject FFI plumbing (evo-sys, eds-sys)
├── jmap-backend-book(-module)/ EBookMetaBackend + jmap-book-sync
├── jmap-backend-cal(-module)/  ECalMetaBackend + jmap-cal-sync
├── jmap-mail(-sync)/           CamelStore/Transport provider
├── jmap-backend-collection(-module)/  ECollectionBackend fanning one account
│                                       out into the three backends above
├── jmap-config(-module)/       account-setup UI (EMailConfigServiceBackend,
│                                EConfigLookup) incl. OAuth2 discovery/registration
├── jmap-ui/                    vacation, scheduled send and snooze in Evolution's
│                                own UI; rides in jmap-config-module's .so
├── jmap-functional/             functional tests driving the built .so modules
│                                against a real EDS registry (no display needed)
└── example-module/              Evolution UI module template (hand-written FFI,
                                   unrelated to the crates above)
```

Structured like [evolution-ews](https://gitlab.gnome.org/GNOME/evolution-ews)
(design notes in the commit history). FFI via `glib-sys`/`gobject-sys` plus
hand-written and bindgen-generated EDS bindings (`evo-sys`, `eds-sys`);
integration tests against [Stalwart](https://stalw.art/) (mock-first, plus a
`--features live-server` harness against a real deployment) and, manually,
real Fastmail for OAuth2.

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
