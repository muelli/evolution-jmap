#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
build_dir="${repo_root}/build-rpm"

if command -v podman >/dev/null 2>&1 && [[ "${1-}" != "--host" ]]; then
	image="docker.io/library/fedora@sha256:6c75d5bf57cb0fa5aa4b92c6a83c86c791644496d9ac230de7711f5b8ec3b898"
	host_uid="$(id -u)"
	host_gid="$(id -g)"
	echo "ci/rpm.sh: running rootless in podman image ${image}"
	exec podman run --rm --userns=keep-id \
		-u 0 \
		-v "${repo_root}:${repo_root}:Z" \
		-w "${repo_root}" \
		-e HOST_UID="${host_uid}" \
		-e HOST_GID="${host_gid}" \
		"${image}" \
		bash -lc '
			set -euo pipefail
			trap '\''chown -R "${HOST_UID}:${HOST_GID}" "${PWD}"'\'' EXIT
			./ci/install-deps-rpm.sh
			./ci/rpm.sh --host
		'
fi

if ! command -v rpmbuild >/dev/null 2>&1; then
	echo "ci/rpm.sh: missing required tool: rpmbuild" >&2
	exit 1
fi
if ! command -v rpmlint >/dev/null 2>&1; then
	echo "ci/rpm.sh: missing required tool: rpmlint" >&2
	exit 1
fi


cmake -S "${repo_root}" -B "${build_dir}" -G Ninja
cmake --build "${build_dir}"

cpack --config "${build_dir}/CPackConfig.cmake" -G RPM --verbose -B "${build_dir}"

rpm_count="$(find "${build_dir}" -maxdepth 1 -type f -name '*.rpm' | wc -l)"
if [[ "${rpm_count}" -eq 0 ]]; then
	echo "ci/rpm.sh: no RPM packages produced" >&2
	exit 1
fi

# The --strict flag makes rpmlint fail on a warning too, not only on an
# error.
# This matches the package-rpm-lint check, where any rpmlint tag fails the
# build.
rpmlint --strict "${build_dir}"/*.rpm
