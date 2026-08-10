#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Full build: the whole cargo workspace plus the Evolution example module
# via CMake, then the test suite through CTest. Requires the EDS headers
# (run ci/install-deps.sh first) and a Rust toolchain. The built
# artifacts land in build/cargo-target/release.

set -euo pipefail
cd "$(dirname "$0")/.."

# CMake seeds SOURCE_DATE_EPOCH from the commit timestamp via git; when the
# checkout is owned by a different uid (CI containers), git refuses until
# the directory is trusted.
git config --global --add safe.directory "$(pwd)" 2>/dev/null || true

cmake -S . -B build -G Ninja
cmake --build build
ctest --test-dir build --output-on-failure
