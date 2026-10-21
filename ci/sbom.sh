#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script creates one CycloneDX SBOM for a release.
# The SBOM combines the Rust crate graph from cargo-cyclonedx (the workflow
# pins its version) and the .deb package's system-library Depends.
# dpkg-shlibdeps already derived the Depends field (see cmake/Packaging.cmake).
# Each dependency lists the version installed on the host where the package
# was built, as recorded by ci/deb-dependencies.sh.
# The output is deterministic: the timestamp comes from SOURCE_DATE_EPOCH, and
# the script adds no random serialNumber.
# A rerun on the same tag produces an identical SBOM, like the binaries.
# This script needs only cargo metadata (no compile step, no EDS headers), jq,
# and dpkg-deb. All three tools are already present on the standard release
# runner.
set -euo pipefail
cd "$(dirname "$0")/.."

deb="$1"        # path to the built .deb file
resolved="$2"   # output JSON from ci/deb-dependencies.sh for that .deb file
out="$3"        # path for the output SBOM file
: "${SOURCE_DATE_EPOCH:?set SOURCE_DATE_EPOCH}"
ts="$(date -u -d "@${SOURCE_DATE_EPOCH}" +%Y-%m-%dT%H:%M:%SZ)"
ver="${GITHUB_REF_NAME:-$(git describe --tags --always)}"

# Step 1: convert the .deb's Depends field into pkg:deb components.
# Each Depends clause becomes one component, named and versioned as resolved
# on the build host.
# The resolution must come from this exact package, not from a different build.
# A resolution from a different build would list versions this .deb was never
# linked against.
# A package bounded from both sides, for example libevolution's ">=" and "<<",
# is two clauses but one component.
# CycloneDX requires every bom-ref to be unique, so each package name produces
# only one component.
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

# Step 2: convert the Rust crates to CycloneDX 1.5 format, one file per
# workspace member.
# cargo-cyclonedx has no --locked flag of its own: it reads whatever cargo
# metadata resolves.
# The script first runs cargo metadata --locked to verify the committed
# Cargo.lock is current.
# This step ties the SBOM to the lock file without passing a flag
# cargo-cyclonedx rejects.
# The script then runs cargo cyclonedx to generate the CycloneDX files.
( cd rust && cargo metadata --locked --format-version 1 >/dev/null )
# This trap deletes every intermediate *.cdx.json file on exit, success or
# failure, leaving only the assembled SBOM.
trap 'find rust -name "*.cdx.json" -delete' EXIT
( cd rust && cargo cyclonedx --format json --spec-version 1.5 )

# This step folds every workspace member's component list into one
# deduplicated array. It works whether the pinned cargo-cyclonedx version
# emits one file per crate or a single aggregate file.
crate_components="$(find rust -name '*.cdx.json' -print0 \
  | xargs -0 jq -s '[.[].components[]] | unique_by(.purl // .name)')"

# Step 3: assemble one deterministic document. It has no serialNumber field
# and a fixed timestamp.
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
