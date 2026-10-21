#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script is a reproducibility check.
# It builds the same crate twice, in two different absolute paths.
# It then asserts that the two binaries are byte-identical.
# This proves that the path-remapping and SOURCE_DATE_EPOCH plumbing keep
# the build independent of where the build happens.
# This script needs only a Rust toolchain, not the EDS headers, because it
# builds only the jmap-mockd binary.
# This script deliberately runs outside the shared build cache, unlike the
# other CI jobs.

set -euo pipefail
cd "$(dirname "$0")/.."

epoch="$(git log -1 --format=%ct)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for d in a b; do
    cp -a rust "$work/$d"
    (
        cd "$work/$d"
        export SOURCE_DATE_EPOCH="$epoch"
        export CARGO_HOME="$work/$d/.cargo-home"
        # Each build tree remaps its own absolute path to /build, and its
        # own CARGO_HOME to /cargo.
        # This remap stops the differing paths from appearing in the
        # output binary.
        export RUSTFLAGS="--remap-path-prefix=$PWD=/build --remap-path-prefix=$CARGO_HOME=/cargo"
        cargo build --release --locked -p evolution-jmap-mock
    )
done

a=$(sha256sum "$work/a/target/release/jmap-mockd" | cut -d' ' -f1)
b=$(sha256sum "$work/b/target/release/jmap-mockd" | cut -d' ' -f1)
echo "build A: $a"
echo "build B: $b"
if [ "$a" != "$b" ]; then
    echo "!! builds differ — not reproducible" >&2
    exit 1
fi
echo "== reproducible: identical =="
