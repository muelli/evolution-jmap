# Verifying release artifacts

Every release publishes `SHA256SUMS` alongside the artifacts, plus
machine-checkable provenance recorded in the public
[Rekor](https://docs.sigstore.dev/logging/overview/) transparency log.

A release carries three things: `jmap-mockd` (the mock JMAP server the test
suite runs against), a source tarball, and
`evolution-jmap_<version>_<arch>.deb` — the backends themselves. Everything
in the release is attested, because the attestation step and the upload step
are given the same directory rather than two lists that could drift apart.

## 1. Checksums

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

## 2. Build provenance (GitHub releases)

Artifacts are attested with
[`actions/attest-build-provenance`](https://github.com/actions/attest-build-provenance)
(SLSA provenance, Sigstore-signed, Rekor-logged):

```bash
gh attestation verify jmap-mockd --repo muelli/evolution-jmap
```

This proves the file was built by this repository's release workflow at a
specific commit — not on a maintainer's laptop. `gh attestation verify`
refuses to run at all without `gh auth login` or `GH_TOKEN`, even for a
public repository's attestations. Without a token, fetch and verify the
same attestation directly instead:

```bash
digest=$(sha256sum jmap-mockd | cut -d' ' -f1)
curl -s "https://api.github.com/repos/muelli/evolution-jmap/attestations/sha256:$digest" \
  | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['attestations'][0]['bundle']))" \
  > jmap-mockd.bundle.json
cosign verify-blob-attestation --bundle jmap-mockd.bundle.json \
  --certificate-identity-regexp '^https://github.com/muelli/evolution-jmap/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --type slsaprovenance1 \
  jmap-mockd
```

One attestation covers every file in the release (subject list includes
`SHA256SUMS` and all four other artifacts), so the bundle fetched for any
one of them verifies all of them; querying by a different file's digest
returns the identical bundle.

## 3. GitLab tag pipelines

Tag pipelines sign the checksum file keylessly with cosign using the CI
job's OIDC identity:

```bash
cosign verify-blob SHA256SUMS \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity-regexp 'gitlab' \
  --certificate-oidc-issuer https://gitlab.com
```

## 4. The `.deb`

`apt` verifies nothing about a package handed to it as a file, so verify it
before installing rather than after:

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify evolution-jmap_*_amd64.deb --repo muelli/evolution-jmap
sudo apt install ./evolution-jmap_*_amd64.deb
```

Two things worth knowing before installing:

- **It must match the EDS and Evolution it is installed next to.** The six
  files in it are modules that `evolution-data-server`, Camel, the source
  registry and Evolution's shell `dlopen`, and their ABI is not stable across
  releases. The package's `Depends` are derived by `dpkg-shlibdeps` from the
  modules themselves, built against the pinned CI image (Ubuntu 24.04,
  Evolution 3.52) — `apt` will refuse it on a machine whose libraries are too
  old, but a mismatch it cannot see shows up as a backend that never appears in
  the account type list rather than as an error.
- **Inspect it first if you like.** `dpkg-deb --contents` lists exactly six
  files — a book backend, a calendar backend, the Camel provider and its
  `.urls`, the registry module and the configuration module. Nothing else: in
  particular the C example module this repository still builds is not in it.

## 5. Reproduce it yourself

The binaries are bit-for-bit reproducible (a CI job enforces this on every
push). To rebuild and compare:

```bash
git clone https://github.com/muelli/evolution-jmap && cd evolution-jmap
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
cd rust
export RUSTFLAGS="--remap-path-prefix=$PWD=/build --remap-path-prefix=$HOME/.cargo=/cargo"
cargo build --release --locked -p evolution-jmap-mock
sha256sum target/release/jmap-mockd   # compare against SHA256SUMS
```

The toolchain is pinned by `rust/rust-toolchain.toml` and dependencies by
the committed `Cargo.lock`, so any machine with the same rustc produces
the same bytes.

The `.deb` is reproducible in the same sense, and `ctest -R
package-deb-reproducible` is what enforces it — the package is built three
times under three different environments and all three must be byte-identical.
Rebuilding it needs the Evolution and EDS development headers, so do it in the
image the release was built in (the digest is in
`.github/workflows/release.yml`):

```bash
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
cmake -S . -B build -G Ninja
cmake --build build
cmake --build build --target package
sha256sum build/*.deb   # compare against SHA256SUMS
```

Every timestamp inside the package — the files, the directories `cpack`
creates on the way to them, and the `ar` member headers — is
`SOURCE_DATE_EPOCH`, so the package is dated by the commit and not by whoever
packaged it. A different EDS version, or a different image, will produce
different bytes: that is the ABI contract these modules live under, not a
reproducibility failure.

## 6. The SBOM

Every release also carries `evolution-jmap-<version>.cdx.json`, a
[CycloneDX](https://cyclonedx.org/) 1.5 Software Bill of Materials: the Rust
crate graph (from the committed `Cargo.lock`) plus the `.deb`'s system-library
`Depends`, the ones `dpkg-shlibdeps` derives. It is checksummed and attested
like every other artifact — it is one of the files in `dist/*`.

```bash
# Integrity and provenance, same as any other artifact:
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify evolution-jmap-*.cdx.json --repo muelli/evolution-jmap

# Scan it against advisory feeds (either tool; neither is required to install):
grype sbom:evolution-jmap-*.cdx.json
osv-scanner --sbom=evolution-jmap-*.cdx.json
```

The SBOM is reproducible in the same sense as the binaries: its timestamp is
`SOURCE_DATE_EPOCH` and it carries no random serial number, so it is a
deterministic function of `Cargo.lock` and the package's `Depends`. The crate
versions are exact `pkg:cargo` Package-URLs; the system libraries are
`pkg:deb/ubuntu` names carrying the `Depends` version *constraints* rather than
resolved versions, because those are fixed only inside the build image.

## 7. Runtime smoke test on a clean system

Checksums and attestation prove the bytes are genuine; they say nothing about
whether the `.deb` actually works once installed. The `.deb`'s `Depends` name
one target: the pinned CI image (Ubuntu 24.04, Evolution 3.52.3), so that is
the only distro to smoke-test. Do it in a throwaway rootless podman container
so a bad install can't touch the host:

```bash
podman run -d --name jmap-smoke -v /path/to/release/assets:/assets:ro ubuntu:24.04 sleep infinity
podman exec jmap-smoke bash -c "apt-get update -qq"
podman exec jmap-smoke bash -c "DEBIAN_FRONTEND=noninteractive apt-get install -y evolution evolution-data-server libglib2.0-bin curl"
podman exec jmap-smoke bash -c "cd /assets && apt-get install -y ./evolution-jmap_*_amd64.deb"
```

`apt` resolves the package's `Depends` against the distro's own EDS/Evolution
packages with no extra repository needed, confirming the versions in
`Depends` are satisfiable on a genuinely clean install, not just on the build
image.

To prove the backends actually load (not just that the files unpacked),
start `evolution-source-registry` with `G_MESSAGES_DEBUG=all` against an
isolated `XDG_*`/D-Bus session carrying a `BackendName=jmap` probe source
(`[Collection] BackendName=jmap` with `ContactsEnabled=true` and
`CalendarEnabled=true`, as in `docs/examples/jmap-mock-collection.source`),
and look for `Adding probe (...)` in its output — that line only appears if
`module-jmap-backend.so` registered a factory for that backend name. A
`BackendName=notarealbackend` control source started from a clean registry
produces no such line, and
`org.gnome.evolution.dataserver.SourceManager.RefreshBackend` on its UID
fails with `Cannot find source`, where the real probe's `RefreshBackend`
call returns successfully: that contrast is what rules out the line always
appearing regardless of backend name.

For end-to-end proof, point a real collection source's `[Authentication]
Host`/`Port` at a running `jmap-mockd` (release binary, no runtime
dependencies beyond libc) and start `evolution-source-registry` plus
`evolution-addressbook-factory -w` and `evolution-calendar-factory -w`. The
collection backend queries `jmap-mockd`'s JMAP session over HTTP and creates
real child `ESource`s for every resource the mock server reports (visible via
`org.freedesktop.DBus.ObjectManager.GetManagedObjects` on
`/org/gnome/evolution/dataserver/SourceManager`): an `[Address Book]
BackendName=jmap` child with `Resource/Identity=AB1` and a `[Calendar]
BackendName=jmap` child with `Resource/Identity=CAL1`, matching jmap-mockd's
own seeded `AB1`/`CAL1` resources — proof the backend spoke real JMAP to the
mock server, not just that its shared object loaded.

Confirmed 2026-10-16 against the v0.5.0 release assets on a fresh
`ubuntu:24.04` podman container: checksums and attestation verified (section
2), `apt install ./evolution-jmap_0.5.0_amd64.deb` resolved cleanly against
Ubuntu 24.04's own `evolution`/`evolution-data-server` 3.52.3 packages, the
probe source produced `Adding probe (...)` with the `notarealbackend`
control producing nothing, and a probe collection against a release
`jmap-mockd` surfaced its `AB1`/`CAL1` resources as real child sources. No
findings: everything in this release behaves as documented on a clean
install.
