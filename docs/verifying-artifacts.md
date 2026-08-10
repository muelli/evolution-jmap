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
specific commit — not on a maintainer's laptop.

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
