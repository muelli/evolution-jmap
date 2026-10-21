#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script runs the test suites of the FFI crates under LeakSanitizer.
# GLib allocates with its own allocator, so a missing free on the C side is
# invisible to Rust and to every other test. LeakSanitizer intercepts malloc
# for the whole process and finds it.
# A test binary that leaks fails with exit code 23, and so does this script.
# It runs on a schedule, not on every push: it needs a nightly toolchain and
# a second build of each crate.
#
# This script needs rustup, the EDS development headers (ci/install-deps.sh)
# and addr2line (binutils) for symbol names in the reports. It installs the
# toolchain named by LEAK_TOOLCHAIN (the default is `nightly`) if missing.
#
# Usage: ci/leak-check.sh [package...]
#   With no arguments, the script checks the packages in DEFAULT_PACKAGES.
#   With arguments, the script checks only the given packages, for example
#   jmap-mail.
#
# ci/lsan.supp holds the suppressions. Add an entry there only for a one-time
# allocation of GLib or EDS, never for an allocation that this project owns.

set -euo pipefail
cd "$(dirname "$0")/.."

toolchain=${LEAK_TOOLCHAIN:-nightly}

DEFAULT_PACKAGES=(
    eds-sys
    jmap-mail
    evolution-jmap-ical
    jmap-config
    jmap-backend-core
    jmap-backend-book
    jmap-backend-cal
    jmap-backend-collection
)

if ! rustup run "$toolchain" rustc --version >/dev/null 2>&1; then
    rustup toolchain install "$toolchain" --profile minimal
fi

host=$(rustup run "$toolchain" rustc -vV | sed -n 's/^host: //p')

if [ "$#" -gt 0 ]; then
    packages=("$@")
else
    packages=("${DEFAULT_PACKAGES[@]}")
fi

package_args=()
for package in "${packages[@]}"; do
    package_args+=(-p "$package")
done

# The explicit --target keeps RUSTFLAGS away from build scripts and proc
# macros. They run on the host and must not link the sanitizer runtime.
# A separate target directory keeps the sanitizer build away from the
# normal build, which it would otherwise invalidate on each run.
export RUSTFLAGS="-Zsanitizer=leak"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$PWD/build/leak-check-target}
export LSAN_OPTIONS="suppressions=$PWD/ci/lsan.supp:print_suppressions=1:allow_addr2line=1"

echo "== LeakSanitizer: ${packages[*]} =="
cd rust
cargo "+$toolchain" test --locked --target "$host" --no-fail-fast "${package_args[@]}"
echo "== no leaks in ${packages[*]} =="
