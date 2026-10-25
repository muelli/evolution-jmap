#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Publication gate for evolution-jmap-mock.
#
# This script verifies three things:
# 1. The crate packages successfully with `cargo package`.
# 2. The unpacked tarball builds and tests outside the workspace.
# 3. Rustdoc for the packaged crate passes with warnings denied.

set -euo pipefail

for tool in cargo tar mktemp find sort tail; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "jmap-mock/ci/package-check.sh: missing required tool: $tool" >&2
        exit 1
    fi
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../../../.." && pwd)"
workspace_dir="${repo_root}/rust"
crate_manifest="${workspace_dir}/crates/jmap-mock/Cargo.toml"

if [ -z "${CARGO_TARGET_DIR:-}" ]; then
    export CARGO_TARGET_DIR="${workspace_dir}/target/jmap-mock-package-check"
fi
if [ -z "${CARGO_PROFILE_TEST_STRIP:-}" ]; then
    export CARGO_PROFILE_TEST_STRIP="debuginfo"
fi

patch_args=(
    --config "patch.crates-io.evolution-jmap-proto.path=\"${workspace_dir}/crates/jmap-proto\""
    --config "patch.crates-io.evolution-jmap-client.path=\"${workspace_dir}/crates/jmap-client\""
    --config "patch.crates-io.evolution-jmap-vcard.path=\"${workspace_dir}/crates/jmap-vcard\""
    --config "patch.crates-io.evolution-jmap-ical.path=\"${workspace_dir}/crates/jmap-ical\""
)

echo "== Packaging evolution-jmap-mock =="
cargo package --manifest-path "${crate_manifest}" --allow-dirty --no-verify "${patch_args[@]}"

target_dir="${CARGO_TARGET_DIR:-${workspace_dir}/target}"
package_dir="${target_dir}/package"
crate_tarball="$(find "${package_dir}" -maxdepth 1 -name 'evolution-jmap-mock-*.crate' | sort -V | tail -n 1)"

if [ -z "${crate_tarball}" ] || [ ! -f "${crate_tarball}" ]; then
    echo "jmap-mock/ci/package-check.sh: package tarball not found in ${package_dir}" >&2
    exit 1
fi

tmp_dir="$(mktemp -d -t jmap-mock-package-check.XXXXXX)"
trap 'rm -rf "${tmp_dir}"' EXIT INT TERM

tar -xzf "${crate_tarball}" -C "${tmp_dir}"
extracted_root="$(find "${tmp_dir}" -mindepth 1 -maxdepth 1 -type d -name 'evolution-jmap-mock-*' | head -n 1)"

if [ -z "${extracted_root}" ] || [ ! -d "${extracted_root}" ]; then
    echo "jmap-mock/ci/package-check.sh: failed to extract package tarball" >&2
    exit 1
fi

extracted_manifest="${extracted_root}/Cargo.toml"

echo "== Building unpacked tarball =="
cargo build --manifest-path "${extracted_manifest}" "${patch_args[@]}"

echo "== Testing unpacked tarball (default features) =="
cargo test --manifest-path "${extracted_manifest}" "${patch_args[@]}"

echo "== Testing unpacked tarball (parse feature) =="
cargo test --manifest-path "${extracted_manifest}" --features parse "${patch_args[@]}"

echo "== Rustdoc with warnings denied =="
RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path "${extracted_manifest}" --no-deps --all-features "${patch_args[@]}"

echo "jmap-mock package check passed"
