#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script runs the test suites of the FFI crates under ThreadSanitizer,
# in the image of ci/Containerfile.tsan. In that image GLib, Camel and EDS are
# instrumented, so ThreadSanitizer sees their locks. The script runs
# ci/thread-check.sh in the image, with the suppressions of ci/tsan.supp.
# A test binary that reports a data race exits with code 66.
#
# The script needs rootless podman. It builds the image if it is missing.
# A build takes about 15 minutes, so keep the image.
#
# Add a package to DEFAULT_PACKAGES only when its tests give no report.
#
# Usage: ci/thread-check-eds.sh [package...]
#   With no arguments, the script checks the packages in DEFAULT_PACKAGES.
#   With arguments, the script checks only the given packages.

set -euo pipefail
cd "$(dirname "$0")/.."

image=${THREAD_EDS_IMAGE:-evolution-jmap-tsan}

DEFAULT_PACKAGES=(
    jmap-mail
)

if [ "$#" -gt 0 ]; then
    packages=("$@")
else
    packages=("${DEFAULT_PACKAGES[@]}")
fi

# The runtime of clang 18 turns address randomization off with personality().
# The default seccomp profile of podman blocks that call.
podman_args=(--security-opt seccomp=unconfined)

if ! podman image exists "$image"; then
    podman build "${podman_args[@]}" -f ci/Containerfile.tsan -t "$image" ci
fi

# The build and the crate downloads stay in build/, so the next run uses them.
mkdir -p build/thread-check-eds-target build/thread-check-eds-registry

podman run --rm "${podman_args[@]}" \
    -v "$PWD:/src" \
    -v "$PWD/build/thread-check-eds-registry:/opt/cargo/registry" \
    -e CARGO_TARGET_DIR=/src/build/thread-check-eds-target \
    -e THREAD_TSAN_OPTIONS="suppressions=/src/ci/tsan.supp" \
    -w /src \
    "$image" ci/thread-check.sh "${packages[@]}"
