#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Reproducibility check: build the same crate twice in two different
# absolute paths and assert the binaries are byte-identical. Proves the
# path-remapping and SOURCE_DATE_EPOCH plumbing keep the build independent
# of where it happens. Needs only a Rust toolchain (no EDS headers — it
# builds jmap-mockd). Deliberately does its own thing outside any cache.

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
        # Both trees remap their own path to /build, so the differing
        # locations must not survive into the binary.
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
