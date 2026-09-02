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

echo "== unsafe-count meter =="
# rustc/clippy already gate *known* unsafe idioms (forbid(unsafe_code) on the
# pure crates, deny(unsafe_op_in_unsafe_fn) everywhere); this catches the
# thing they can't: a crate's unsafe surface growing silently. Growth stays
# possible, it just needs unsafe-baseline.txt updated in the same commit.
unsafe_meter_fail=0
unsafe_meter_pattern='unsafe (fn|impl|\{|extern)'
while read -r crate baseline_count; do
    case "$crate" in
        ""|"#"*) continue ;;
    esac
    crate_src="crates/$crate/src"
    if [ ! -d "$crate_src" ]; then
        echo "FAIL: unsafe-baseline.txt names '$crate', which has no crates/$crate/src." >&2
        unsafe_meter_fail=1
        continue
    fi
    # `|| true` inside the substitution: grep exits 1 when a crate has zero
    # unsafe sites (the pure crates: evo-sys, jmap-*-sync), and under
    # `set -o pipefail` that would abort this whole script mid-loop before the
    # meter could report anything.
    actual_count=$({ grep -rEo "$unsafe_meter_pattern" "$crate_src" 2>/dev/null || true; } | wc -l)
    if [ "$actual_count" -gt "$baseline_count" ]; then
        echo "FAIL: $crate grew from $baseline_count to $actual_count unsafe sites." >&2
        echo "If intentional, update its line in unsafe-baseline.txt in this commit." >&2
        unsafe_meter_fail=1
    fi
done < unsafe-baseline.txt
for crate_dir in crates/*/; do
    crate=$(basename "$crate_dir")
    if ! grep -qE "^$crate " unsafe-baseline.txt; then
        echo "FAIL: crates/$crate has no line in unsafe-baseline.txt; add one." >&2
        unsafe_meter_fail=1
    fi
done
if [ "$unsafe_meter_fail" -ne 0 ]; then
    exit 1
fi
echo "-- unsafe-count meter: no crate exceeds its baseline --"

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

echo "== repository-split boundary lint =="
# The infrastructure split moved NIGHT-LOG.md/AGY-LOG.md/AGY-TASKS.md/
# BACKLOG.md/MILESTONES.md to a private harness repository and rewrote this
# repository's ROADMAP.md down to a thin, human-facing file with no item
# numbers. The sweep that cleared every existing mention reached zero on
# 2026-08-31; this now hard-fails on any new mention of those files, or of a
# ROADMAP.md item number, in the product tree.
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
if [ "$boundary_lint_count" -gt 0 ]; then
    echo "FAIL: $boundary_lint_count mention(s) of agent bookkeeping files in the product tree." >&2
    echo "The sweep reached zero on 2026-08-31 and this lint now enforces the boundary:" >&2
    echo "code and its docs must not cite ROADMAP.md, NIGHT-LOG.md, AGY-*, BACKLOG.md or MILESTONES.md." >&2
    grep -rnE "$boundary_lint_pattern" "${boundary_lint_paths[@]}" "${boundary_lint_exclude[@]}" 2>/dev/null | head -20 >&2
    exit 1
fi
echo "-- boundary clean: 0 mentions --"

echo "== all checks passed =="
