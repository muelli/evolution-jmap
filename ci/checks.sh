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

cd ..

echo "== packaging (.deb ctest, if EDS dev headers/cmake/ninja are present) =="
# Scoped to the package-deb* tests only, not the full ctest suite: those are
# pure packaging checks (build the .deb, run lintian, check reproducibility)
# that need nothing beyond a CMake configure + build, unlike the
# functional/gui-smoke legs, which need a live D-Bus/Xvfb registry this
# script has never assumed. This is what would have caught `ac00396`'s
# lintian-clean-.deb regression (CURRENT PRIORITY item 8) before it sat red
# in CI for days — the packaging job that does catch it runs separately from
# this script, so nobody watching only `ci/checks.sh` saw it break.
if have cmake && have ninja && pkg-config --exists evolution-shell-3.0 evolution-calendar-3.0 evolution-mail-3.0 libecal-2.0 2>/dev/null; then
    cmake -S . -B build -G Ninja >/dev/null
    ninja -C build
    ctest --test-dir build -R 'package-deb' --output-on-failure
else
    echo "-- cmake, ninja, or the EDS dev headers are not available; skipping the .deb packaging check (expected on a bare Rust-only machine) --" >&2
fi

echo "== repository-split boundary lint (warn only) =="
# The infrastructure split (see docs/ROADMAP.md's repository-split item) moves
# NIGHT-LOG.md/AGY-LOG.md/AGY-TASKS.md/BACKLOG.md/MILESTONES.md to a private
# harness repository and rewrites this repository's ROADMAP.md down to a
# thin, human-facing file with no item numbers. Until then, this just counts
# how many mentions of those files, or of a ROADMAP.md item number, remain
# outside the files being replaced — a progress meter for the sweep, not yet
# an enforced boundary: flip to hard-fail only once the count is zero.
boundary_lint_paths=(rust cmake ci debian docs)
boundary_lint_exclude=(
    --exclude=NIGHT-LOG.md --exclude=NIGHT-LOG-archive.md
    --exclude=AGY-LOG.md --exclude=AGY-TASKS.md
    --exclude=BACKLOG.md --exclude=MILESTONES.md
    --exclude=ROADMAP.md --exclude=checks.sh
)
# NOTE 2026-08-30: the first version of this pattern required a literal space
# in "ROADMAP.md item 23", but the form this repository actually uses is
# `docs/ROADMAP.md` item 23, with a closing backtick in between. It therefore
# matched 15 of the 89 real references and the sweep stopped four times
# believing it was finished. Every ROADMAP.md mention now counts: the thin
# roadmap that survives the split carries no item numbers, no Track headings
# and no CURRENT PRIORITY section, so each existing citation needs a decision
# rather than an assumption.
boundary_lint_pattern='NIGHT-LOG\.md|AGY-LOG\.md|AGY-TASKS\.md|BACKLOG\.md|MILESTONES\.md|ROADMAP\.md'
boundary_lint_count=$({ grep -rnE "$boundary_lint_pattern" "${boundary_lint_paths[@]}" "${boundary_lint_exclude[@]}" 2>/dev/null || true; } | wc -l)
echo "-- $boundary_lint_count mention(s) remain; see docs/ROADMAP.md's repository-split item for the sweep plan --"

echo "== all checks passed =="
