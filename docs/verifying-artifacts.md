# Verifying release artifacts

Every release publishes `SHA256SUMS` alongside the artifacts, plus
machine-checkable provenance recorded in the public
[Rekor](https://docs.sigstore.dev/logging/overview/) transparency log.

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

## 4. Reproduce it yourself

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
