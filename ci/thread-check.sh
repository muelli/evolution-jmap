#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script runs the test suites of the pure-Rust crates under
# ThreadSanitizer. A test binary that reports a data race exits with code 66,
# and so does this script.
# It runs on a schedule, not on every push: it needs a nightly toolchain and
# an instrumented build of the standard library.
#
# The default packages have no C code that shares state between threads.
# Do not add an FFI crate: GLib, Camel and EDS are not instrumented, so
# ThreadSanitizer cannot see their locks and reports every hand-off between
# threads as a race.
#
# This script needs rustup and addr2line (binutils) for symbol names in the
# reports. It installs the toolchain named by THREAD_TOOLCHAIN and its
# rust-src component if missing. The default is the dated nightly of
# ci/leak-check.sh, for the same reasons.
#
# Usage: ci/thread-check.sh [package...]
#   With no arguments, the script checks the packages in DEFAULT_PACKAGES.
#   With arguments, the script checks only the given packages.

set -euo pipefail
cd "$(dirname "$0")/.."

toolchain=${THREAD_TOOLCHAIN:-nightly-2026-10-18}

DEFAULT_PACKAGES=(
    evolution-jmap-client
    evolution-jmap-book-sync
    evolution-jmap-cal-sync
    evolution-jmap-mail-sync
    evolution-jmap-collection-sync
    evolution-jmap-tracing-capture
)

if ! rustup run "$toolchain" rustc --version >/dev/null 2>&1; then
    rustup toolchain install "$toolchain" --profile minimal
fi
rustup component add rust-src --toolchain "$toolchain"

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

# The standard library must be instrumented too (-Zbuild-std). Its locks use
# futexes directly, and ThreadSanitizer only sees the locks it instruments.
# The explicit --target keeps RUSTFLAGS away from build scripts and proc
# macros. A separate target directory keeps this build away from the normal
# build. A compiler wrapper such as sccache breaks the dep-info files of
# -Zbuild-std, so the script turns any configured wrapper off.
export RUSTFLAGS="-Zsanitizer=thread -Cforce-frame-pointers=yes"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$PWD/build/thread-check-target}
export TSAN_OPTIONS="halt_on_error=0 second_deadlock_stack=1"

echo "== ThreadSanitizer: ${packages[*]} =="
cd rust
cargo "+$toolchain" --config 'build.rustc-wrapper=""' test --locked \
    -Zbuild-std --target "$host" --tests --no-fail-fast "${package_args[@]}"
echo "== no data races in ${packages[*]} =="
