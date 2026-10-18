#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Build the Flatpak extension org.gnome.Evolution.Extension.evolution-jmap
# from source against Flathub's org.gnome.Evolution, then check it inside
# the Evolution sandbox: every JMAP module is in the extension tree and
# every library it links resolves against the app's own EDS and Evolution.
# A module built against the wrong ABI does not load in the sandbox (the
# SONAMEs differ between EDS releases), so a missing library is a failure.
#
# Works on a bare machine with flatpak and flatpak-builder: everything is
# downloaded into a private Flatpak installation under build-flatpak/, so
# the user's own installation is never changed. The first run downloads
# the GNOME SDK and two SDK extensions (several GB) and builds the whole
# Rust workspace, so expect it to take long.
#
# Usage: ci/flatpak.sh            build, install into the private
#                                 installation, check
#        ci/flatpak.sh --check    only check what the last build installed

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
work_dir="${repo_root}/build-flatpak"
app_id="org.gnome.Evolution"
ext_id="org.gnome.Evolution.Extension.evolution-jmap"
manifest_rel="packaging/flatpak/${ext_id}.from-source.yml"
flathub_url="https://dl.flathub.org/repo/flathub.flatpakrepo"

# A private user installation, unless the caller already chose one.
export FLATPAK_USER_DIR="${FLATPAK_USER_DIR:-${work_dir}/installation}"

# Path of each module relative to the extension's prefix. Keep in step with
# the install commands in the from-source manifest.
expected_files=(
	lib/evolution-data-server/addressbook-backends/libebookbackendjmap.so
	lib/evolution-data-server/calendar-backends/libecalbackendjmap.so
	lib/evolution-data-server/camel-providers/libcameljmap.so
	lib/evolution-data-server/camel-providers/libcameljmap.urls
	lib/evolution-data-server/registry-modules/module-jmap-backend.so
	lib/evolution/modules/module-jmap-configuration.so
)

fail() {
	echo "ci/flatpak.sh: $*" >&2
	exit 1
}

for tool in flatpak flatpak-builder; do
	command -v "${tool}" >/dev/null 2>&1 || fail "missing required tool: ${tool}"
done

build() {
	command -v cargo >/dev/null 2>&1 || fail "missing required tool: cargo"
	command -v git >/dev/null 2>&1 || fail "missing required tool: git"

	flatpak remote-add --user --if-not-exists flathub "${flathub_url}"
	# The extension's "runtime" is the Evolution app itself: its /app holds
	# the EDS and Evolution headers and pkg-config files the build needs.
	flatpak install --user --noninteractive --or-update flathub "app/${app_id}/$(uname -m)/stable"

	# Stage a clean copy of the tree: the manifest's dir source copies all
	# of it into the sandbox, and a developer checkout carries several GB of
	# build trees. Tracked files plus untracked ones git does not ignore.
	local stage="${work_dir}/src"
	rm -rf "${stage}"
	mkdir -p "${stage}"
	# --ignore-failed-read: a tracked file deleted in the work tree is skipped.
	(cd "${repo_root}" && git ls-files -z --cached --others --exclude-standard |
		tar --null --ignore-failed-read -T - -cf -) | tar -xf - -C "${stage}"
	[[ -f "${stage}/${manifest_rel}" ]] || fail "staging the source tree failed"

	# The sandbox has no network: hand cargo every crate up front. The
	# manifest's cargo config points crates.io at this directory.
	cargo vendor --quiet --locked --versioned-dirs \
		--manifest-path "${stage}/rust/Cargo.toml" \
		"${stage}/packaging/flatpak/vendor" >/dev/null

	flatpak-builder --user --install --force-clean --disable-rofiles-fuse \
		--install-deps-from=flathub \
		--state-dir "${work_dir}/state" \
		"${work_dir}/build" "${stage}/${manifest_rel}"
}

check() {
	flatpak info --user "${ext_id}" >/dev/null 2>&1 ||
		fail "${ext_id} is not installed in ${FLATPAK_USER_DIR}"

	# Runs inside the Evolution sandbox, where the extension is mounted.
	# Prints MISSING <path> and LDD <path>: <line> for every problem.
	local prefix="/app/evolution/extensions/evolution-jmap"
	local report
	report="$(flatpak run --user --command=sh "${app_id}" -c '
		prefix="$1"
		shift
		for rel in "$@"; do
			path="${prefix}/${rel}"
			if [ ! -f "${path}" ]; then
				echo "MISSING ${rel}"
				continue
			fi
			case "${rel}" in
			*.so)
				ldd "${path}" 2>&1 | grep -F "not found" |
					sed "s|^|LDD ${rel}: |"
				;;
			esac
		done
		echo DONE
	' sh "${prefix}" "${expected_files[@]}")" ||
		fail "could not run a shell inside the ${app_id} sandbox"

	# A shell that died early would print nothing, which must not pass.
	grep -qx DONE <<<"${report}" || fail "the sandbox check did not finish: ${report}"
	if grep -v -x DONE <<<"${report}" | grep -q .; then
		grep -v -x DONE <<<"${report}" >&2
		fail "the extension payload is incomplete or links libraries the sandbox does not have"
	fi
	echo "ci/flatpak.sh: all ${#expected_files[@]} files present, every library resolves in the ${app_id} sandbox"
}

case "${1-}" in
"") build; check ;;
--check) check ;;
*) fail "unknown argument: $1 (expected nothing or --check)" ;;
esac
