#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

if ! command -v rpmbuild >/dev/null 2>&1; then
	echo "ci/rpm.sh: missing required tool: rpmbuild" >&2
	exit 1
fi
if ! command -v rpmlint >/dev/null 2>&1; then
	echo "ci/rpm.sh: missing required tool: rpmlint" >&2
	exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
build_dir="${repo_root}/build-rpm"

cmake -S "${repo_root}" -B "${build_dir}" -G Ninja
cmake --build "${build_dir}"

cpack --config "${build_dir}/CPackConfig.cmake" -G RPM --verbose

rpm_count="$(find "${build_dir}" -maxdepth 1 -type f -name '*.rpm' | wc -l)"
if [[ "${rpm_count}" -eq 0 ]]; then
	echo "ci/rpm.sh: no RPM packages produced" >&2
	exit 1
fi

rpmlint "${build_dir}"/*.rpm
