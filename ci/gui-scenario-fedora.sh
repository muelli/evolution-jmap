#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script runs one or more ci/gui-scenario/ scenarios against a newer
# Evolution than Ubuntu 24.04's 3.52.
# It runs inside the pinned Fedora image that ci/rpm.sh and ci/gui-smoke-
# fedora.sh use, which ships Evolution and EDS 3.60.
# By default, this script uses rootless podman.
# The --host flag runs the build and the scenario directly, for a job that
# already runs inside that image.
#
# Usage: ci/gui-scenario-fedora.sh [--host] SCENARIO...
# SCENARIO is a file name in ci/gui-scenario/scenarios/, without ".py".
#
# Artifacts go to build-gui-scenario-fedora/gui-scenario. Set
# GUI_SCENARIO_ARTIFACTS to use a different directory.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
build_dir="${repo_root}/build-gui-scenario-fedora"

if command -v podman >/dev/null 2>&1 && [[ "${1-}" != "--host" ]]; then
	image="docker.io/library/fedora@sha256:6c75d5bf57cb0fa5aa4b92c6a83c86c791644496d9ac230de7711f5b8ec3b898"
	echo "ci/gui-scenario-fedora.sh: running rootless in podman image ${image}"
	exec podman run --rm --userns=keep-id \
		-u 0 \
		-v "${repo_root}:${repo_root}:Z" \
		-w "${repo_root}" \
		-e HOST_UID="$(id -u)" \
		-e HOST_GID="$(id -g)" \
		-e GUI_SCENARIO_ARTIFACTS="${GUI_SCENARIO_ARTIFACTS:-${build_dir}/gui-scenario}" \
		"${image}" \
		bash -c '
			set -euo pipefail
			trap '\''chown -R "${HOST_UID}:${HOST_GID}" "${PWD}/build-gui-scenario-fedora"'\'' EXIT
			./ci/install-deps-rpm.sh
			./ci/install-deps-gui-smoke-rpm.sh
			./ci/gui-scenario-fedora.sh --host "$@"
		' bash "$@"
fi

if [ "${1-}" = "--host" ]; then
	shift
fi

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

echo "ci/gui-scenario-fedora.sh: $(rpm -q evolution evolution-data-server | tr "\n" " ")"

cmake -S "${repo_root}" -B "${build_dir}" -G Ninja
cmake --build "${build_dir}"

for component in book-backend cal-backend camel-provider collection-backend config-module; do
	$SUDO cmake --install "${build_dir}" --component "${component}"
done

export GUI_SCENARIO_BUILD="${build_dir}"
export GUI_SCENARIO_ARTIFACTS="${GUI_SCENARIO_ARTIFACTS:-${build_dir}/gui-scenario}"
export JMAP_MOCKD="${build_dir}/cargo-target/release/jmap-mockd"
exec "${repo_root}/ci/gui-scenario/run.sh" --host "$@"
