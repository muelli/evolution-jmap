#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script builds the Flatpak extension org.gnome.Evolution.Extension.evolution-jmap.
# It builds the extension from source against Flathub's org.gnome.Evolution.
# It then checks the extension inside the Evolution sandbox.
# The check confirms that every JMAP module is in the extension tree.
# The check also confirms that every library the extension links resolves
# against the app's own EDS and Evolution.
# A module built against the wrong ABI does not load in the sandbox, because
# SONAMEs differ between EDS releases.
# A missing library therefore counts as a check failure.
#
# This script works on a bare machine with flatpak and flatpak-builder
# installed.
# Everything downloads into a private Flatpak installation under
# build-flatpak/.
# The user's own Flatpak installation is never changed.
# The first run downloads the GNOME SDK and two SDK extensions, several GB
# in total.
# The first run also builds the whole Rust workspace.
# Expect the first run to take a long time.
#
# Usage: ci/flatpak.sh            builds, installs into the private
#                                 installation, and checks
#        ci/flatpak.sh --check    only checks what the last build installed

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
work_dir="${repo_root}/build-flatpak"
app_id="org.gnome.Evolution"
ext_id="org.gnome.Evolution.Extension.evolution-jmap"
manifest_rel="packaging/flatpak/${ext_id}.from-source.yml"
flathub_url="https://dl.flathub.org/repo/flathub.flatpakrepo"

# This sets a private user installation, unless the caller already set
# FLATPAK_USER_DIR.
export FLATPAK_USER_DIR="${FLATPAK_USER_DIR:-${work_dir}/installation}"

# Each entry lists a module's path relative to the extension's prefix.
# Keep this list in step with the install commands in the from-source
# manifest.
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
	# The extension's "runtime" is the Evolution app itself.
	# The app's /app directory holds the EDS and Evolution headers and
	# pkg-config files the build needs.
	flatpak install --user --noninteractive --or-update flathub "app/${app_id}/$(uname -m)/stable"

	# This step stages a clean copy of the tree.
	# The manifest's dir source copies this entire staged copy into the
	# sandbox.
	# A developer's checkout can carry several GB of build trees.
	# The staged copy has every tracked file, plus every untracked file git
	# does not ignore.
	local stage="${work_dir}/src"
	rm -rf "${stage}"
	mkdir -p "${stage}"
	# The --ignore-failed-read flag skips a tracked file that was deleted in
	# the work tree.
	(cd "${repo_root}" && git ls-files -z --cached --others --exclude-standard |
		tar --null --ignore-failed-read -T - -cf -) | tar -xf - -C "${stage}"
	[[ -f "${stage}/${manifest_rel}" ]] || fail "staging the source tree failed"

	# The sandbox has no network access.
	# This step hands cargo every crate up front, before the sandbox build
	# runs.
	# The manifest's cargo config points crates.io at this vendor directory.
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

	# This command runs inside the Evolution sandbox, where the extension is
	# mounted.
	# It prints "MISSING <path>" for a missing file.
	# It prints "LDD <path>: <line>" for every unresolved library ldd finds.
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

	# A shell that dies early prints nothing. That case must not count as a
	# pass.
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
