#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# M10 (docs/eds-version-matrix.md): run the exact crate set
# cmake/Rust.cmake's rust-test-eds CTest target runs — the layout checks in
# eds-sys/evo-sys included — against whatever EDS pkg-config resolves inside
# the container this script is run in. The 3.52 leg is the existing
# `build` job's `ctest` run on the pinned Ubuntu headers; this script is for
# the *other* legs, invoked from ci.yml's eds-version-matrix job.

set -euo pipefail
cd "$(dirname "$0")/../rust"

CRATES=(
    -p eds-sys -p evo-sys
    -p jmap-backend-core
    -p jmap-backend-book -p jmap-backend-cal -p jmap-mail
    -p jmap-backend-collection -p jmap-config
)

if ! cargo clippy --version >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    rustup component add clippy
fi

cargo clippy --locked "${CRATES[@]}" --all-targets -- -D warnings

cargo test --locked "${CRATES[@]}"
