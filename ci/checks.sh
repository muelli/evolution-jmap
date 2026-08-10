#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The lint/test gate — the single definition of "green", run identically
# by CI (GitHub, GitLab), the autonomous agent before every push, and you
# on your laptop. Needs only a Rust toolchain and (for the last two
# checks) Python's pipx / cargo; both bootstrap themselves if missing, so
# `ci/checks.sh` works on a bare machine. No Evolution headers required —
# it operates on the workspace's default members.

set -euo pipefail
cd "$(dirname "$0")/.."

have() { command -v "$1" >/dev/null 2>&1; }

echo "== REUSE lint =="
if have reuse; then
    reuse lint
elif have pipx; then
    pipx run reuse lint
elif have uvx; then
    uvx reuse lint
else
    echo "!! neither reuse, pipx, nor uvx found — install one to run the licence check" >&2
    exit 1
fi

cd rust

echo "== rustfmt =="
cargo fmt --check

echo "== clippy (-D warnings) =="
cargo clippy --all-targets --locked -- -D warnings

echo "== tests =="
cargo test --locked

echo "== cargo-deny (licences, advisories, bans) =="
have cargo-deny || cargo install --locked cargo-deny
cargo deny check

echo "== all checks passed =="
