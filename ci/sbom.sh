#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Emit one CycloneDX SBOM for a release: the Rust crate graph (cargo-cyclonedx,
# pinned by the workflow) plus the .deb's system-library Depends — which
# dpkg-shlibdeps already derived (see cmake/Packaging.cmake), each with the
# version installed where the package was built, as ci/deb-dependencies.sh
# recorded it there. Deterministic:
# the timestamp is SOURCE_DATE_EPOCH and there is no random serialNumber, so
# re-running a tag yields byte-identical output, the same discipline as the
# binaries. Needs only cargo metadata (no compile, no EDS headers), jq and
# dpkg-deb, all present on the stock release runner.
set -euo pipefail
cd "$(dirname "$0")/.."

deb="$1"        # path to the built .deb
resolved="$2"   # ci/deb-dependencies.sh's output for that .deb
out="$3"        # output SBOM path
: "${SOURCE_DATE_EPOCH:?set SOURCE_DATE_EPOCH}"
ts="$(date -u -d "@${SOURCE_DATE_EPOCH}" +%Y-%m-%dT%H:%M:%SZ)"
ver="${GITHUB_REF_NAME:-$(git describe --tags --always)}"

# 1. The .deb's Depends -> pkg:deb components, one per Depends clause, named
#    and versioned as resolved on the build host. The resolution must have been
#    made for this very package: one recorded for another build would put
#    versions in the SBOM that this .deb was never linked against. A package
#    bounded from both sides (libevolution's ">=" and "<<") is two clauses but
#    one component: CycloneDX requires every bom-ref to be unique.
if [ "$(jq -r .depends "$resolved")" != "$(dpkg-deb -f "$deb" Depends)" ]; then
  echo "sbom: $resolved was not resolved from $deb's Depends" >&2
  exit 1
fi
deb_components="$(jq '
  .distro as $d
  | reduce .packages[] as $p ([];
      if any(.[]; .name == $p.name) then . else . + [$p] end)
  | [.[]
     | {type: "library", name: .name, version: .version,
        "bom-ref": ("deb:" + .name),
        purl: ("pkg:deb/" + $d.id + "/" + .name + "@" + (.version | @uri)
               + "?arch=" + .arch
               + (if $d.codename != "" then "&distro=" + $d.codename else "" end)),
        scope: "required"}]' "$resolved")"

# 2. Rust crates -> CycloneDX 1.5 (one file per workspace member). cargo-cyclonedx
#    has no --locked flag of its own (it reads whatever `cargo metadata` resolves),
#    so verify the committed Cargo.lock is current first — that keeps the SBOM tied
#    to the lock without passing a flag cargo-cyclonedx rejects — then generate.
( cd rust && cargo metadata --locked --format-version 1 >/dev/null )
# Leave only the assembled SBOM behind, whichever way this exits.
trap 'find rust -name "*.cdx.json" -delete' EXIT
( cd rust && cargo cyclonedx --format json --spec-version 1.5 )

# Fold every member's component list into one deduplicated array (robust
# whether the pinned cargo-cyclonedx emits per-crate or aggregate files).
crate_components="$(find rust -name '*.cdx.json' -print0 \
  | xargs -0 jq -s '[.[].components[]] | unique_by(.purl // .name)')"

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

echo "wrote $out"
