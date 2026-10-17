# Verifying release artifacts

Each GitHub release has four artifacts and a `SHA256SUMS` file:

- `jmap-mockd`: the mock JMAP server that the test suite runs against
- `evolution-jmap-<tag>.tar.xz`: the source tarball
- `evolution-jmap_<version>_<arch>.deb`: the backends
- `evolution-jmap-<tag>.cdx.json`: the SBOM

The release workflow (`.github/workflows/release.yml`) attests and uploads
the same directory, `dist/*`. Every published file therefore has provenance in
the public [Rekor](https://docs.sigstore.dev/logging/overview/) transparency
log.

## 1. Checksums

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

## 2. Build provenance

The release workflow attests the artifacts with
[`actions/attest-build-provenance`](https://github.com/actions/attest-build-provenance)
(SLSA provenance, Sigstore-signed, Rekor-logged):

```bash
gh attestation verify jmap-mockd --repo muelli/evolution-jmap
```

This command proves that the release workflow of this repository built the
file at a specific commit. `gh attestation verify` needs `gh auth login` or
`GH_TOKEN`, also for a public repository. If you have no token, fetch the
attestation and verify it with cosign:

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

One attestation covers every file in `dist/`. The bundle that you fetch for
one file therefore verifies all of them.

## 3. The `.deb`

`apt` does not verify a package that you give to it as a file. Verify the
package before you install it:

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify evolution-jmap_*_amd64.deb --repo muelli/evolution-jmap
sudo apt install ./evolution-jmap_*_amd64.deb
```

- **Version match.** The package must match the EDS and Evolution versions on
  the system. `evolution-data-server`, Camel, the source registry and
  Evolution load the modules with `dlopen`. The ABI is not stable across
  releases. `dpkg-shlibdeps` derives the `Depends` from the modules, which the
  pinned CI image builds (Ubuntu 24.04, Evolution 3.52). If the libraries are
  too old, `apt` refuses the package. `apt` cannot see some mismatches. In
  that case, the backend is missing from the account type list, and no error
  occurs.
- **Contents.** `dpkg-deb --contents` shows six modules: the book backend, the
  calendar backend, the Camel provider and its `.urls` file, the registry
  module and the configuration module. It also shows `changelog.gz`,
  `copyright`, `third-party-notices`, a lintian override and the translation
  catalogues. `cmake/Packaging.cmake` lists the exact set. The `package-deb`
  CTest test enforces it.

## 4. Reproduce the build yourself

`jmap-mockd` is bit-for-bit reproducible. The `reproducible` CI job builds it
twice in different directories and compares the hashes. To rebuild it and
compare the result, do this:

```bash
git clone https://github.com/muelli/evolution-jmap && cd evolution-jmap
git checkout <tag>
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
cd rust
export RUSTFLAGS="--remap-path-prefix=$PWD=/build --remap-path-prefix=$HOME/.cargo=/cargo"
cargo build --release --locked -p evolution-jmap-mock
sha256sum target/release/jmap-mockd   # compare against SHA256SUMS
```

`rust/rust-toolchain.toml` pins the toolchain. The committed `Cargo.lock` pins
the dependencies.

The `.deb` is also reproducible. The `package-deb-reproducible` CTest test
builds it three times with different `SOURCE_DATE_EPOCH` environments. The
test requires identical bytes. To rebuild the `.deb`, you need the development
headers of Evolution and EDS. Use the image in which the release was built.
The digest is in `.github/workflows/release.yml`:

```bash
cmake -S . -B build -G Ninja -DCMAKE_INSTALL_PREFIX=/usr
cmake --build build
cmake --build build --target package
sha256sum build/*.deb   # compare against SHA256SUMS
```

Every timestamp in the package (files, directories and `ar` member headers)
is the commit time. It is not the packaging time. A different EDS version or
a different image gives different bytes. This is the ABI contract of these
modules. It is not a reproducibility failure.

## 5. The SBOM

`evolution-jmap-<tag>.cdx.json` is a [CycloneDX](https://cyclonedx.org/) 1.5
Software Bill of Materials. `ci/sbom.sh` makes it. It lists the Rust crate
graph from `Cargo.lock` and the system-library `Depends` of the `.deb`.

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify evolution-jmap-*.cdx.json --repo muelli/evolution-jmap

# Optional: scan it against advisory feeds with either tool.
grype sbom:evolution-jmap-*.cdx.json
osv-scanner --sbom=evolution-jmap-*.cdx.json
```

The SBOM is reproducible: its timestamp is `SOURCE_DATE_EPOCH` and it has no
random serial number. The crates are exact `pkg:cargo` Package-URLs. The
system libraries are `pkg:deb/ubuntu` names with the `Depends` version
constraints. They are not resolved versions.

## 6. Runtime smoke test on a clean system

Checksums and attestations prove that the bytes are genuine. They do not prove
that the `.deb` works. The `Depends` target Ubuntu 24.04. Test the `.deb`
there, in a temporary rootless podman container:

```bash
podman run -d --name jmap-smoke -v /path/to/release/assets:/assets:ro ubuntu:24.04 sleep infinity
podman exec jmap-smoke bash -c "apt-get update -qq"
podman exec jmap-smoke bash -c "DEBIAN_FRONTEND=noninteractive apt-get install -y evolution evolution-data-server libglib2.0-bin curl"
podman exec jmap-smoke bash -c "cd /assets && apt-get install -y ./evolution-jmap_*_amd64.deb"
```

This test shows that the `Depends` resolve against the packages of the
distribution.

To show that the registry module loads, do these steps:

1. Start `evolution-source-registry` with `G_MESSAGES_DEBUG=all`. Use an
   isolated `XDG_*` environment and an isolated D-Bus session.
2. Use a collection source like `docs/examples/jmap-mock-collection.source`.
3. Look for `Adding probe (...)` in the output.

That line appears only if `module-jmap-backend.so` registered a factory for
`BackendName=jmap`. As a control, a source with `BackendName=notarealbackend`
gives no such line.

For an end-to-end check, do these steps:

1. Run `jmap-mockd`. It needs only libc.
2. Point `[Authentication] Host` and `Port` of the collection source at it.
3. Start `evolution-source-registry`, `evolution-addressbook-factory -w` and
   `evolution-calendar-factory -w`.

The collection backend then creates an `[Address Book]` child source and a
`[Calendar]` child source. They are for the address book and the calendar that
the mock server seeds. To see them, call
`org.freedesktop.DBus.ObjectManager.GetManagedObjects` on
`/org/gnome/evolution/dataserver/SourceManager`.
