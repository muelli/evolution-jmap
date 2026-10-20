#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script runs a full build.
# It builds the whole cargo workspace and the Evolution example module with
# CMake.
# It then runs the test suite with CTest.
# This script needs the EDS headers and a Rust toolchain.
# Run ci/install-deps.sh before this script, to install the EDS headers.
# The built artifacts land in build/cargo-target/release.

set -euo pipefail
cd "$(dirname "$0")/.."

# CMake gets the value of SOURCE_DATE_EPOCH from git, based on the commit
# timestamp.
# Git refuses to run in a checkout owned by a different user ID, as in CI
# containers, until the user marks the directory as trusted.
git config --global --add safe.directory "$(pwd)" 2>/dev/null || true

# This flag sets CMAKE_INSTALL_PREFIX to /usr, matching debian/rules's
# configure line.
# CMakeLists.txt already defaults the install prefix to /usr itself.
# That default keeps translation catalogues out of /usr/local, avoiding a
# lintian dir-in-usr-local failure.
# This flag only makes the same choice explicit here too.
cmake -S . -B build -G Ninja -DCMAKE_INSTALL_PREFIX=/usr
cmake --build build
ctest --test-dir build --output-on-failure
