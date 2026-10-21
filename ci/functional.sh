#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# See docs/functional-tests.md for the full procedure.
# This script configures the build with the EDS runtime tests enabled.
# The script then builds the project.
# The script runs only the tests in the functional label.
# Run ci/install-deps.sh before this script, to install the dev headers.
# Run ci/install-deps-functional.sh before this script, to install the EDS
# runtime and a D-Bus daemon.

set -euo pipefail
cd "$(dirname "$0")/.."

# See ci/build.sh for more information.
# CMake gets the value of SOURCE_DATE_EPOCH from git, based on the commit
# timestamp.
# Git refuses to run in a checkout owned by a different user ID, as in CI
# containers, until the user marks the directory as trusted.
git config --global --add safe.directory "$(pwd)" 2>/dev/null || true

cmake -S . -B build -G Ninja -DENABLE_FUNCTIONAL_TESTS=ON
cmake --build build
ctest --test-dir build -L functional --output-on-failure
