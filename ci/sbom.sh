#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Emit one CycloneDX SBOM for a release: the Rust crate graph (cargo-cyclonedx,
# pinned by the workflow) plus the .deb's system-library Depends — which
# dpkg-shlibdeps already derived (see cmake/Packaging.cmake). Deterministic:
# the timestamp is SOURCE_DATE_EPOCH and there is no random serialNumber, so
# re-running a tag yields byte-identical output, the same discipline as the
# binaries. Needs only cargo metadata (no compile, no EDS headers), jq and
# dpkg-deb — all present on the stock release runner.
set -euo pipefail
cd "$(dirname "$0")/.."

deb="$1"   # path to the built .deb
out="$2"   # output SBOM path
: "${SOURCE_DATE_EPOCH:?set SOURCE_DATE_EPOCH}"
ts="$(date -u -d "@${SOURCE_DATE_EPOCH}" +%Y-%m-%dT%H:%M:%SZ)"
ver="${GITHUB_REF_NAME:-$(git describe --tags --always)}"

# 1. Rust crates -> CycloneDX 1.5 (one file per workspace member). cargo-cyclonedx
#    has no --locked flag of its own (it reads whatever `cargo metadata` resolves),
#    so verify the committed Cargo.lock is current first — that keeps the SBOM tied
#    to the lock without passing a flag cargo-cyclonedx rejects — then generate.
( cd rust && cargo metadata --locked --format-version 1 >/dev/null )
( cd rust && cargo cyclonedx --format json --spec-version 1.5 )

# Fold every member's component list into one deduplicated array (robust
# whether the pinned cargo-cyclonedx emits per-crate or aggregate files).
crate_components="$(find rust -name '*.cdx.json' -print0 \
  | xargs -0 jq -s '[.[].components[]] | unique_by(.purl // .name)')"

# 2. The .deb's Depends (dpkg-shlibdeps output) -> pkg:deb components. Split on
#    commas, drop the "(>= x)" version constraints, one component per library.
deb_components="$(dpkg-deb -f "$deb" Depends \
  | tr ',' '\n' \
  | sed -E 's/\(.*\)//; s/^[[:space:]]*//; s/[[:space:]]*$//' \
  | grep -v '^$' \
  | jq -R '{type:"library", name:., "bom-ref":("deb:"+.),
            purl:("pkg:deb/ubuntu/"+.), scope:"required"}' \
  | jq -s '.')"

# 3. Assemble one deterministic document (no serialNumber; fixed timestamp).
jq -n \
  --argjson crates "$crate_components" \
  --argjson deb "$deb_components" \
  --arg ts "$ts" \
  --arg ver "$ver" '
  {
    bomFormat: "CycloneDX",
    specVersion: "1.5",
    version: 1,
    metadata: {
      timestamp: $ts,
      component: { type: "application", name: "evolution-jmap", version: $ver }
    },
    components: ($crates + $deb)
  }' > "$out"

# Leave only the assembled SBOM behind.
find rust -name '*.cdx.json' -delete
echo "wrote $out"
