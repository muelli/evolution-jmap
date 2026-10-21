#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script builds every cargo-fuzz target in the tree.
# It then replays each target's committed corpus once.
# The fuzz crates are separate workspaces: each `fuzz/Cargo.toml` declares its
# own `[workspace]`.
# Because of this, `cargo test` and `cargo clippy` in rust/ never compile the
# fuzz crates.
# An API change in the crate under test can break its fuzz harness unnoticed.
# Nobody notices until the next person tries to fuzz that crate.
# This script exists to give that notice early.
# It runs on a schedule, not on every push.
# It needs a nightly toolchain and builds each harness with AddressSanitizer.
#
# This script needs rustup with the toolchain named by FUZZ_TOOLCHAIN (the
# default is `nightly`).
# It also needs cargo-fuzz.
# The script installs either tool if missing.
#
# Usage: ci/fuzz-build.sh [crate-dir...]
#   With no arguments, the script builds every rust/crates/*/fuzz workspace.
#   With arguments, the script builds only the given crate directories, for
#   example rust/crates/jmap-ical.

set -euo pipefail
cd "$(dirname "$0")/.."

toolchain=${FUZZ_TOOLCHAIN:-nightly}

have() { command -v "$1" >/dev/null 2>&1; }

if ! rustup run "$toolchain" rustc --version >/dev/null 2>&1; then
    rustup toolchain install "$toolchain" --profile minimal
fi
have cargo-fuzz || cargo install --locked cargo-fuzz

host=$(rustup run "$toolchain" rustc -vV | sed -n 's/^host: //p')

if [ "$#" -gt 0 ]; then
    crate_dirs=("$@")
else
    crate_dirs=()
    for manifest in rust/crates/*/fuzz/Cargo.toml; do
        [ -e "$manifest" ] || continue
        crate_dirs+=("$(dirname "$(dirname "$manifest")")")
    done
fi

# A glob that matches nothing is the failure this script exists to catch.
# The script must not report success in that case.
if [ "${#crate_dirs[@]}" -eq 0 ]; then
    echo "FAIL: no rust/crates/*/fuzz/Cargo.toml found; nothing to build." >&2
    exit 1
fi

fail=0
built=0
for crate_dir in "${crate_dirs[@]}"; do
    fuzz_dir="$crate_dir/fuzz"
    if ! grep -qE '^cargo-fuzz[[:space:]]*=[[:space:]]*true' "$fuzz_dir/Cargo.toml" 2>/dev/null; then
        echo "FAIL: $fuzz_dir/Cargo.toml is not a cargo-fuzz workspace." >&2
        fail=1
        continue
    fi

    echo "== $fuzz_dir =="
    targets=$(cd "$crate_dir" && cargo "+$toolchain" fuzz list)
    if [ -z "$targets" ]; then
        echo "FAIL: $fuzz_dir declares no fuzz targets." >&2
        fail=1
        continue
    fi

    if ! (cd "$crate_dir" && cargo "+$toolchain" fuzz build); then
        echo "FAIL: $fuzz_dir does not build." >&2
        fail=1
        continue
    fi

    for target in $targets; do
        binary="$fuzz_dir/target/$host/release/$target"
        if [ ! -x "$binary" ]; then
            echo "FAIL: $target was declared but $binary was not built." >&2
            fail=1
            continue
        fi
        # The -runs=0 flag replays the corpus without changing it.
        # A committed input that crashes today is a regression.
        # Checking this costs only seconds.
        corpus="$fuzz_dir/corpus/$target"
        if [ -d "$corpus" ]; then
            if ! (cd "$crate_dir" && cargo "+$toolchain" fuzz run "$target" -- -runs=0); then
                echo "FAIL: $target crashes on its committed corpus ($corpus)." >&2
                fail=1
                continue
            fi
        else
            echo "-- $target has no committed corpus; built only --"
        fi
        built=$((built + 1))
    done
done

if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo "== $built fuzz target(s) built and replayed =="
