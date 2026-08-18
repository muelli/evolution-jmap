# Adding an SBOM to releases — proposal

Status: **proposal only.** Nothing here is applied. The unified diff in
[§6](#6-proposed-diff-not-applied) is what an implementation PR would carry.

## Goal and constraints

Publish a Software Bill of Materials (SBOM) with every release so consumers can
see exactly what goes into the shipped artifacts and match them against
vulnerability feeds — **without disturbing** the two guarantees the release
already makes:

- **Reproducible builds** — cacheless, `SOURCE_DATE_EPOCH`, path-remapped;
  enforced by `ci/reproducible.sh`, `ctest -R package-deb-reproducible`, and the
  `reproducible` job in `.github/workflows/ci.yml`.
- **Build provenance** — `actions/attest-build-provenance` signs every file in
  `dist/*` (Sigstore, Rekor-logged), verified with `gh attestation verify`.

The design below rides the *existing* rails rather than adding parallel ones:
the SBOM is written into `dist/`, so the current `sha256sum *`, the
`subject-path: dist/*` attestation, and the `gh release create dist/*` upload
all pick it up **unchanged**. No change to the attest step, the release step, or
the reproducible `package` job — so the repro and provenance guarantees cannot
regress, and `cmake/tests/check-release-workflow.cmake` (which asserts *attested
set == published set*) still passes verbatim.

---

## 1. Format: CycloneDX vs SPDX — recommend **CycloneDX 1.5 (JSON)**

Both are ISO standards (SPDX ISO/IEC 5962; CycloneDX ISO/IEC 5692) and both are
defensible. CycloneDX wins here for concrete reasons:

- **Rust-native tooling.** The reference generator for the Cargo graph
  (`cargo-cyclonedx`, §2) is an OWASP CycloneDX project. It maps `Cargo.lock`
  directly to `pkg:cargo/<name>@<version>` Package-URLs — the identifiers
  `grype`, `osv-scanner`, and OWASP Dependency-Track consume without a
  translation layer.
- **Purpose fit.** This project already has a *license* gate (`cargo-deny`
  against `rust/deny.toml`). The SBOM's job is the other half — supply-chain
  transparency and vulnerability matching — which is CycloneDX's design center
  (component graph, VEX, provenance) rather than SPDX's license-compliance
  origins.
- **No lock-in.** CycloneDX → SPDX is a mechanical `cyclonedx convert` for any
  consumer who needs SPDX, so shipping CycloneDX forecloses nothing. Dual-emit
  is cheap if a distro later demands SPDX, but is not proposed for round one.

**Recommendation:** one CycloneDX 1.5 JSON document per release, named
`evolution-jmap-<version>.cdx.json`.

---

## 2. Tool: **`cargo-cyclonedx`**, pinned in CI — plus the `.deb`'s system deps

### Rust crate graph

- **Tool:** [`cargo-cyclonedx`](https://github.com/CycloneDX/cyclonedx-rust-cargo)
  (OWASP CycloneDX). Reads `cargo metadata` / `Cargo.lock` — **no compile
  required**, so it runs in the plain release job without the Evolution/EDS
  headers the module crates need to *build*.
- **License / viability:** Apache-2.0, actively maintained under the CycloneDX
  umbrella. It is a **build-time** tool only — never linked into or distributed
  with the GPLv3 artifact, so it raises no license question. (Apache-2.0 is on
  the `deny.toml` allowlist regardless.)
- **Pinning:** installed exactly like the existing `cargo-deny` bootstrap in
  `ci/checks.sh`, but with an explicit version so the tool cannot drift:
  `cargo install --locked --version <X.Y.Z> cargo-cyclonedx`. Confirm the
  current release on crates.io at implementation time and pin it exactly; bump
  deliberately, never floating.
- **Not a runtime dependency:** it appears only in the release job, never in the
  `.deb` and never in `Cargo.toml`.

**Alternatives considered.** `cargo-sbom` (MIT/Apache; emits a single aggregate
CycloneDX *or* SPDX doc in one invocation — attractive, but less established than
the OWASP reference tool). `syft` (Apache-2.0; can catalog *both* the Rust tree
and the `.deb` and emit SPDX+CycloneDX — but it is a large external Go binary
that would need its own digest pin and is not Rust-native). `cargo-cyclonedx` is
the minimal, ecosystem-native, license-clean choice; the `.deb` gap it leaves is
closed below without pulling in syft.

### The `.deb`'s system-library dependencies (completeness)

The crate SBOM covers everything compiled *into* the modules, but the `.deb`
also dynamically links system libraries (glib, `libedataserver`, `libcamel`, the
Evolution privlibs) that are **not crates** — they enter through the package's
`Depends`, which `dpkg-shlibdeps` already derives from the modules' ELF files
(see `cmake/Packaging.cmake`, `CPACK_DEBIAN_PACKAGE_SHLIBDEPS ON`). A BoM of only
the crates would be incomplete.

**Approach (recommended, minimal):** read the already-computed field back out of
the built package with `dpkg-deb -f <pkg>.deb Depends` and emit one
`pkg:deb/ubuntu/<name>` component per entry, merged into the same CycloneDX
document. This reuses `dpkg-shlibdeps`' output verbatim — the package's true,
checked dependency surface — and touches neither the reproducible `package` job
nor `Packaging.cmake`. Version *constraints* (`(>= 3.52)`) are recorded but not
resolved to concrete versions, because those are only known inside the pinned
build container.

**Higher-fidelity option (deferred).** To pin concrete installed versions, run
`dpkg-query -W -f='${Package} ${Version}\n'` over the resolved `Depends` closure
*inside the `package` job* (the pinned container) and hand that list to the merge
step. It is strictly more work, edits the reproducible job, and needs care to
stay deterministic — not worth it for round one, but the natural next increment.

---

## 3. Integration: exactly where in `release.yml`

Two structural facts drive the placement:

1. **`check-release-workflow.cmake` asserts attested-set == published-set**, both
   spelled `dist/*`. Anything dropped into `dist/` is therefore checksummed,
   attested, and released *by construction* — and the invariant test keeps
   passing with **zero** changes to the attest/release steps. This is the same
   "same glob on both sides" property the repo already relies on.
2. **The reproducible guarantees live elsewhere.** The `package` job (repro
   `.deb`) and `ci/reproducible.sh` / the `reproducible` job in `ci.yml` are not
   on this path. The SBOM work goes entirely into the **`release` job**, which
   already checks out the tree, has cargo, `jq`, and `dpkg-deb` on the stock
   `ubuntu-24.04` runner, and downloads the `.deb` artifact. So it cannot slow or
   break the repro or provenance work.

**Placement:**

- A new step **"Install the SBOM generator (pinned)"** after *"Download the
  package"* — mirrors the `cargo install --locked` bootstrap already used for
  `cargo-deny`, with an added exact `--version` pin.
- Four lines inside the existing **"Collect artifacts and checksums"** step that
  call a new `ci/sbom.sh` to write `dist/evolution-jmap-<ref>.cdx.json` **before**
  the `sha256sum *` line — so the SBOM is checksummed, and (being in `dist/`)
  attested and published, with no edit to those later steps.

The generator logic goes in **`ci/sbom.sh`**, matching the repo's "logic lives in
`ci/` scripts, YAML just calls them" convention (`reproducible.sh`, `checks.sh`,
`build.sh`). The script is deterministic: timestamp is `SOURCE_DATE_EPOCH`, no
random `serialNumber`, so re-running a tag yields a byte-identical SBOM — the
same reproducibility discipline as the binaries, though not (yet) machine-enforced.

### Attesting the SBOM itself

The SBOM file **already carries build provenance**: it is in `dist/*`, which the
existing `attest-build-provenance` step signs. That is the sensible round-one
answer and needs no extra step.

A stronger *binding* — an SBOM attestation (`actions/attest-sbom`) whose predicate
says "this SBOM describes this `.deb`" — is deliberately **not** in the minimal
diff, because of a sharp edge worth recording: `check-release-workflow.cmake`
collects **every** `subject-path:` value matching `dist/` into its attested set.
A second step with `subject-path: dist/*.deb` would make the attested set
`{dist/*, dist/*.deb}` while the published set stays `{dist/*}`, and the invariant
test would **fail**. Pursuing `attest-sbom` therefore requires first teaching that
check to scope its `subject-path:` scan to the `attest-build-provenance` step
alone. Left as an explicit follow-up, not smuggled into round one.

---

## 4. Verification (consumer-facing, `docs/verifying-artifacts.md`)

A consumer fetches `evolution-jmap-<version>.cdx.json` from the release and:

1. confirms integrity via the existing `sha256sum --check --ignore-missing
   SHA256SUMS` (the SBOM is one of the checksummed files);
2. confirms provenance with `gh attestation verify
   evolution-jmap-<version>.cdx.json --repo muelli/evolution-jmap` (works because
   it rode `dist/*`);
3. optionally validates it as CycloneDX and scans it for advisories, e.g.
   `grype sbom:evolution-jmap-<version>.cdx.json` or
   `osv-scanner --sbom=evolution-jmap-<version>.cdx.json`.

The proposed `## 6. The SBOM` section added to `docs/verifying-artifacts.md` is in
the diff below.

---

## 5. Impact summary

| Property | Effect |
| --- | --- |
| Reproducible `.deb` (`package` job) | **Untouched** — no edits to that job or `Packaging.cmake`. |
| `ci/reproducible.sh` / `reproducible` job | **Untouched** — different workflow. |
| Build-provenance step | **Untouched** — SBOM rides `subject-path: dist/*`. |
| `gh release create` step | **Untouched** — SBOM rides `dist/*`. |
| `SHA256SUMS` | Gains one line (the SBOM) automatically. |
| `check-release-workflow.cmake` | **Still passes** — attested/published stay `dist/*`. |
| Added release-job cost | One `cargo install` (~1 min) + metadata gen (seconds), once per tag, off the repro path. |
| New files | `ci/sbom.sh` (proposed). |

---

## 6. Proposed diff (NOT applied)

```diff
diff --git a/.github/workflows/release.yml b/.github/workflows/release.yml
index 0000000..0000000 100644
--- a/.github/workflows/release.yml
+++ b/.github/workflows/release.yml
@@ -89,6 +89,12 @@ jobs:
       - name: Download the package
         uses: actions/download-artifact@v4
         with:
           name: deb
           path: deb
 
+      # Build-time only, never a runtime/`.deb` dependency. Pinned exactly, like
+      # the cargo-deny bootstrap in ci/checks.sh — confirm the current release on
+      # crates.io and bump this deliberately.
+      - name: Install the SBOM generator (pinned)
+        run: cargo install --locked --version 0.5.7 cargo-cyclonedx
+
       - name: Collect artifacts and checksums
         run: |
           mkdir -p dist
           cp rust/target/release/jmap-mockd dist/
           cp "evolution-jmap-${GITHUB_REF_NAME}.tar.xz" dist/
           cp deb/*.deb dist/
+          # CycloneDX SBOM: the Rust crate graph plus the .deb's dpkg-shlibdeps
+          # Depends, dated by SOURCE_DATE_EPOCH so re-running a tag is
+          # byte-identical. Written into dist/, so the sha256sum below, the
+          # attest step (dist/*) and the release step (dist/*) all cover it
+          # unchanged — attested set stays == published set.
+          export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
+          ci/sbom.sh deb/*.deb "dist/evolution-jmap-${GITHUB_REF_NAME}.cdx.json"
           (cd dist && sha256sum * > SHA256SUMS)
           cat dist/SHA256SUMS
```

```diff
diff --git a/ci/sbom.sh b/ci/sbom.sh
new file mode 100755
index 0000000..0000000
--- /dev/null
+++ b/ci/sbom.sh
@@ -0,0 +1,63 @@
+#!/usr/bin/env bash
+# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
+# SPDX-License-Identifier: GPL-3.0-or-later
+#
+# Emit one CycloneDX SBOM for a release: the Rust crate graph (cargo-cyclonedx,
+# pinned by the workflow) plus the .deb's system-library Depends — which
+# dpkg-shlibdeps already derived (see cmake/Packaging.cmake). Deterministic:
+# the timestamp is SOURCE_DATE_EPOCH and there is no random serialNumber, so
+# re-running a tag yields byte-identical output, the same discipline as the
+# binaries. Needs only cargo metadata (no compile, no EDS headers), jq and
+# dpkg-deb — all present on the stock release runner.
+set -euo pipefail
+cd "$(dirname "$0")/.."
+
+deb="$1"   # path to the built .deb
+out="$2"   # output SBOM path
+: "${SOURCE_DATE_EPOCH:?set SOURCE_DATE_EPOCH}"
+ts="$(date -u -d "@${SOURCE_DATE_EPOCH}" +%Y-%m-%dT%H:%M:%SZ)"
+ver="${GITHUB_REF_NAME:-$(git describe --tags --always)}"
+
+# 1. Rust crates -> CycloneDX 1.5 (one file per workspace member). --locked
+#    ties it to the committed Cargo.lock.
+( cd rust && cargo cyclonedx --locked --format json --spec-version 1.5 )
+
+# Fold every member's component list into one deduplicated array (robust
+# whether the pinned cargo-cyclonedx emits per-crate or aggregate files).
+crate_components="$(find rust -name '*.cdx.json' -print0 \
+  | xargs -0 jq -s '[.[].components[]] | unique_by(.purl // .name)')"
+
+# 2. The .deb's Depends (dpkg-shlibdeps output) -> pkg:deb components. Split on
+#    commas, drop the "(>= x)" version constraints, one component per library.
+deb_components="$(dpkg-deb -f "$deb" Depends \
+  | tr ',' '\n' \
+  | sed -E 's/\(.*\)//; s/^[[:space:]]*//; s/[[:space:]]*$//' \
+  | grep -v '^$' \
+  | jq -R '{type:"library", name:., "bom-ref":("deb:"+.),
+            purl:("pkg:deb/ubuntu/"+.), scope:"required"}' \
+  | jq -s '.')"
+
+# 3. Assemble one deterministic document (no serialNumber; fixed timestamp).
+jq -n \
+  --argjson crates "$crate_components" \
+  --argjson deb "$deb_components" \
+  --arg ts "$ts" \
+  --arg ver "$ver" '
+  {
+    bomFormat: "CycloneDX",
+    specVersion: "1.5",
+    version: 1,
+    metadata: {
+      timestamp: $ts,
+      component: { type: "application", name: "evolution-jmap", version: $ver }
+    },
+    components: ($crates + $deb)
+  }' > "$out"
+
+# Leave only the assembled SBOM behind.
+find rust -name '*.cdx.json' -delete
+echo "wrote $out"
```

```diff
diff --git a/docs/verifying-artifacts.md b/docs/verifying-artifacts.md
index 0000000..0000000 100644
--- a/docs/verifying-artifacts.md
+++ b/docs/verifying-artifacts.md
@@ -50,3 +50,28 @@ before installing rather than after:
   `.urls`, the registry module and the configuration module. Nothing else: in
   particular the C example module this repository still builds is not in it.
+
+## 6. The SBOM
+
+Every release also carries `evolution-jmap_<version>.cdx.json`, a
+[CycloneDX](https://cyclonedx.org/) 1.5 Software Bill of Materials: the Rust
+crate graph (from the committed `Cargo.lock`) plus the `.deb`'s system-library
+`Depends`, the ones `dpkg-shlibdeps` derives. It is checksummed and attested
+like every other artifact — it is one of the files in `dist/*`.
+
+```bash
+# Integrity and provenance, same as any other artifact:
+sha256sum --check --ignore-missing SHA256SUMS
+gh attestation verify evolution-jmap_*.cdx.json --repo muelli/evolution-jmap
+
+# Scan it against advisory feeds (either tool; neither is required to install):
+grype sbom:evolution-jmap_*.cdx.json
+osv-scanner --sbom=evolution-jmap_*.cdx.json
+```
+
+The SBOM is reproducible in the same sense as the binaries: its timestamp is
+`SOURCE_DATE_EPOCH` and it carries no random serial number, so it is a
+deterministic function of `Cargo.lock` and the package's `Depends`. The crate
+versions are exact `pkg:cargo` Package-URLs; the system libraries are
+`pkg:deb/ubuntu` names carrying the `Depends` version *constraints* rather than
+resolved versions, because those are fixed only inside the build image.
```

## 7. Follow-ups (out of round-one scope)

- Resolve concrete system-library versions in the `package` job (§2,
  higher-fidelity option).
- Add `actions/attest-sbom` binding the SBOM to the `.deb` — **requires** first
  scoping `check-release-workflow.cmake`'s `subject-path:` scan to the
  provenance step (§3).
- Optionally machine-enforce SBOM determinism (build twice, diff) the way
  `package-deb-reproducible` does for the `.deb`.
- Mirror the step into `.gitlab-ci.yml`'s tag pipeline if GitLab releases start
  carrying the same artifact set.
