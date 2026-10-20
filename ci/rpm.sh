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

# --strict: a warning fails too, as any lintian tag fails the .deb gate.
rpmlint --strict "${build_dir}"/*.rpm

privlibdir="$(pkg-config --variable=privlibdir evolution-shell-3.0)"
if [[ -z "${privlibdir}" ]]; then
	echo "ci/rpm.sh: evolution-shell-3.0 reports no privlibdir" >&2
	exit 1
fi

required_runpath_modules=(
	"${build_dir}/cargo-target/release/libjmap_backend_book_module.so"
	"${build_dir}/cargo-target/release/libjmap_backend_cal_module.so"
	"${build_dir}/cargo-target/release/libjmap_backend_collection_module.so"
	"${build_dir}/cargo-target/release/libjmap_config_module.so"
)

for module_path in "${required_runpath_modules[@]}"; do
	if [[ ! -f "${module_path}" ]]; then
		echo "ci/rpm.sh: missing module for RUNPATH check: ${module_path}" >&2
		exit 1
	fi

	runpath="$(readelf -d "${module_path}" | awk '/RUNPATH/ { gsub(/[][]/, "", $NF); print $NF; exit }')"
	if [[ -z "${runpath}" ]]; then
		echo "ci/rpm.sh: ${module_path} has no RUNPATH" >&2
		exit 1
	fi

	if [[ ":${runpath}:" != *":${privlibdir}:"* ]]; then
		echo "ci/rpm.sh: ${module_path} RUNPATH '${runpath}' misses '${privlibdir}'" >&2
		exit 1
	fi
done

for rpm_file in "${build_dir}"/*.rpm; do
	scripts_output="$(rpm -qp --scripts "${rpm_file}")"
	if [[ -n "${scripts_output}" && "${scripts_output}" != *"contains no scriptlets"* ]]; then
		echo "ci/rpm.sh: ${rpm_file} ships scriptlets, revisit explicit non-ELF Requires" >&2
		exit 1
	fi
done
