#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# See docs/eds-version-matrix.md for details.
# This script runs the same crate set that cmake/Rust.cmake's rust-test-eds
# CTest target runs, including the layout checks in eds-sys and evo-sys.
# It runs that crate set against whatever EDS pkg-config resolves, inside
# the container the script runs in.
# The 3.52 leg is the existing build job's ctest run, on the pinned Ubuntu
# headers.
# This script covers the other legs.
# ci.yml's eds-version-matrix job invokes this script.

set -euo pipefail
cd "$(dirname "$0")/../rust"

CRATES=(
    -p eds-sys -p evo-sys
    -p jmap-backend-core
    -p jmap-backend-book -p jmap-backend-cal -p jmap-mail
    -p jmap-backend-collection -p jmap-config -p jmap-ui
)

if ! cargo clippy --version >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    rustup component add clippy
fi

cargo clippy --locked "${CRATES[@]}" --all-targets -- -D warnings

cargo test --locked "${CRATES[@]}"
