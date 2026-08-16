#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# M9 layer 1 (docs/functional-tests.md): configure with the EDS runtime
# tests enabled, build, and run just that label. Requires the dev headers
# (ci/install-deps.sh) and the EDS runtime plus a D-Bus daemon
# (ci/install-deps-functional.sh) first.

set -euo pipefail
cd "$(dirname "$0")/.."

# See ci/build.sh: CMake seeds SOURCE_DATE_EPOCH from git, which refuses on a
# checkout owned by a different uid (CI containers) until trusted.
git config --global --add safe.directory "$(pwd)" 2>/dev/null || true

cmake -S . -B build -G Ninja -DENABLE_FUNCTIONAL_TESTS=ON
cmake --build build
ctest --test-dir build -L functional --output-on-failure
