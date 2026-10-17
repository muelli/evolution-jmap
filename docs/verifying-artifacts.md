# Verifying release artifacts

Each GitHub release has four artifacts and a `SHA256SUMS` file:

- `jmap-mockd`, the mock JMAP server that the test suite runs against;
- `evolution-jmap-<tag>.tar.xz`, the source tarball;
- `evolution-jmap_<version>_<arch>.deb`, the backends;
- `evolution-jmap-<tag>.cdx.json`, the SBOM.

The release workflow (`.github/workflows/release.yml`) attests and uploads
the same directory, `dist/*`, so every published file has provenance in the
public [Rekor](https://docs.sigstore.dev/logging/overview/) transparency log.

## 1. Checksums

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

## 2. Build provenance

Artifacts are attested with
[`actions/attest-build-provenance`](https://github.com/actions/attest-build-provenance)
(SLSA provenance, Sigstore-signed, Rekor-logged):

```bash
gh attestation verify jmap-mockd --repo muelli/evolution-jmap
```

This proves that this repository's release workflow built the file at a
specific commit. `gh attestation verify` needs `gh auth login` or `GH_TOKEN`,
also for a public repository. Without a token, fetch the attestation and
verify it with cosign:

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

One attestation covers every file in `dist/`, so the bundle that you fetch
for one file verifies all of them.

## 3. The `.deb`

`apt` does not verify a package that you give it as a file. Verify it before
you install it:

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify evolution-jmap_*_amd64.deb --repo muelli/evolution-jmap
sudo apt install ./evolution-jmap_*_amd64.deb
```

- **It must match the EDS and Evolution that it is installed next to.** The
  modules in it are `dlopen`ed by `evolution-data-server`, Camel, the source
  registry and Evolution, and their ABI is not stable across releases.
  `dpkg-shlibdeps` derives the `Depends` from the modules, built in the pinned
  CI image (Ubuntu 24.04, Evolution 3.52). `apt` refuses the package when the
  libraries are too old. A mismatch that `apt` cannot see shows as a backend
  that is missing from the account type list, not as an error.
- **Contents.** `dpkg-deb --contents` shows six modules (book backend,
  calendar backend, Camel provider and its `.urls` file, registry module,
  configuration module), plus `changelog.gz`, `copyright`,
  `third-party-notices`, a lintian override and the translation catalogues.
  `cmake/Packaging.cmake` lists the exact set and the `package-deb` CTest test
  enforces it.

## 4. Reproduce it yourself

`jmap-mockd` is bit-for-bit reproducible. The `reproducible` CI job builds it
twice in different directories and compares the hashes. To rebuild it and
compare:

```bash
git clone https://github.com/muelli/evolution-jmap && cd evolution-jmap
git checkout <tag>
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
cd rust
export RUSTFLAGS="--remap-path-prefix=$PWD=/build --remap-path-prefix=$HOME/.cargo=/cargo"
cargo build --release --locked -p evolution-jmap-mock
sha256sum target/release/jmap-mockd   # compare against SHA256SUMS
```

`rust/rust-toolchain.toml` pins the toolchain and the committed `Cargo.lock`
pins the dependencies.

The `.deb` is also reproducible. The `package-deb-reproducible` CTest test
builds it three times with different `SOURCE_DATE_EPOCH` environments and
requires identical bytes. Rebuilding it needs the Evolution and EDS
development headers, so use the image that the release was built in (the
digest is in `.github/workflows/release.yml`):

```bash
cmake -S . -B build -G Ninja -DCMAKE_INSTALL_PREFIX=/usr
cmake --build build
cmake --build build --target package
sha256sum build/*.deb   # compare against SHA256SUMS
```

Every timestamp in the package (files, directories and `ar` member headers)
is the commit time, not the packaging time. A different EDS version or image
gives different bytes. That is the ABI contract of these modules, not a
reproducibility failure.

## 5. The SBOM

`evolution-jmap-<tag>.cdx.json` is a [CycloneDX](https://cyclonedx.org/) 1.5
Software Bill of Materials, made by `ci/sbom.sh`. It lists the Rust crate
graph from `Cargo.lock` and the `.deb`'s system-library `Depends`.

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify evolution-jmap-*.cdx.json --repo muelli/evolution-jmap

# Optional: scan it against advisory feeds with either tool.
grype sbom:evolution-jmap-*.cdx.json
osv-scanner --sbom=evolution-jmap-*.cdx.json
```

The SBOM is reproducible: its timestamp is `SOURCE_DATE_EPOCH` and it has no
random serial number. Crates are exact `pkg:cargo` Package-URLs. System
libraries are `pkg:deb/ubuntu` names with the `Depends` version constraints,
not resolved versions.

## 6. Runtime smoke test on a clean system

Checksums and attestations prove that the bytes are genuine, not that the
`.deb` works. The `Depends` target Ubuntu 24.04, so test there, in a
throwaway rootless podman container:

```bash
podman run -d --name jmap-smoke -v /path/to/release/assets:/assets:ro ubuntu:24.04 sleep infinity
podman exec jmap-smoke bash -c "apt-get update -qq"
podman exec jmap-smoke bash -c "DEBIAN_FRONTEND=noninteractive apt-get install -y evolution evolution-data-server libglib2.0-bin curl"
podman exec jmap-smoke bash -c "cd /assets && apt-get install -y ./evolution-jmap_*_amd64.deb"
```

This shows that the `Depends` resolve against the distribution's own
packages.

To show that the registry module loads, start `evolution-source-registry`
with `G_MESSAGES_DEBUG=all` in an isolated `XDG_*` and D-Bus session, with a
collection source like `docs/examples/jmap-mock-collection.source`. Look for
`Adding probe (...)` in the output. That line appears only when
`module-jmap-backend.so` registered a factory for `BackendName=jmap`. As a
control, a source with `BackendName=notarealbackend` gives no such line.

For an end-to-end check, run `jmap-mockd` (it needs only libc), point the
collection source's `[Authentication] Host`/`Port` at it, and start
`evolution-source-registry`, `evolution-addressbook-factory -w` and
`evolution-calendar-factory -w`. The collection backend then creates an
`[Address Book]` child source and a `[Calendar]` child source for the address
book and calendar that the mock seeds. You can see them with
`org.freedesktop.DBus.ObjectManager.GetManagedObjects` on
`/org/gnome/evolution/dataserver/SourceManager`.
