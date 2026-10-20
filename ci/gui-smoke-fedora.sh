#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script runs ci/gui-smoke.sh against a newer Evolution than Ubuntu
# 24.04's 3.52.
# It runs the same assertion: the JMAP account appears, and its inbox is
# not empty.
# It runs inside the pinned Fedora image that ci/rpm.sh and the EDS matrix
# job use, which ships Evolution and EDS 3.60.
# By default, this script uses rootless podman.
# The --host flag runs the build and the smoke test directly, for a job
# that already runs inside that image.
# Artifacts of a failing run go to build-gui-smoke-fedora/artifacts.
# Set GUI_SMOKE_ARTIFACTS to use a different directory.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
build_dir="${repo_root}/build-gui-smoke-fedora"

if command -v podman >/dev/null 2>&1 && [[ "${1-}" != "--host" ]]; then
	image="docker.io/library/fedora@sha256:6c75d5bf57cb0fa5aa4b92c6a83c86c791644496d9ac230de7711f5b8ec3b898"
	echo "ci/gui-smoke-fedora.sh: running rootless in podman image ${image}"
	exec podman run --rm --userns=keep-id \
		-u 0 \
		-v "${repo_root}:${repo_root}:Z" \
		-w "${repo_root}" \
		-e HOST_UID="$(id -u)" \
		-e HOST_GID="$(id -g)" \
		-e GUI_SMOKE_ARTIFACTS="${GUI_SMOKE_ARTIFACTS:-${build_dir}/artifacts}" \
		"${image}" \
		bash -lc '
			set -euo pipefail
			trap '\''chown -R "${HOST_UID}:${HOST_GID}" "${PWD}/build-gui-smoke-fedora"'\'' EXIT
			./ci/install-deps-rpm.sh
			./ci/install-deps-gui-smoke-rpm.sh
			./ci/gui-smoke-fedora.sh --host
		'
fi

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

echo "ci/gui-smoke-fedora.sh: $(rpm -q evolution evolution-data-server | tr "\n" " ")"

cmake -S "${repo_root}" -B "${build_dir}" -G Ninja
cmake --build "${build_dir}"
$SUDO cmake --install "${build_dir}" --component camel-provider

export GUI_SMOKE_ARTIFACTS="${GUI_SMOKE_ARTIFACTS:-${build_dir}/artifacts}"
JMAP_MOCKD="${build_dir}/cargo-target/release/jmap-mockd" \
	exec "${repo_root}/ci/gui-smoke.sh"
