#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Adversarial FFI audit driver: runs in its OWN clone (~/audit-ffi) and
# pushes to branch audit/ffi only, so it can never race the roadmap shift
# working in ~/evolution-jmap. Iterates until the audit declares itself
# complete (AUDIT COMPLETE marker) or 8 sessions have run. Started BY THE
# OPERATOR inside tmux, like the night shift.

set -uo pipefail
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
LOG="$HOME/audit-ffi.log"

mkdir -p "$HOME/audit-ffi"
cd "$HOME/audit-ffi"
[ -d evolution-jmap ] || git clone -q git@github.com:muelli/evolution-jmap.git
cd evolution-jmap
git config user.name "Tobias Mueller"
git config user.email "muelli@cryptobitch.de"

echo "$(date -Is) === audit starting ===" >> "$LOG"
for i in $(seq 1 8); do
    git fetch -q origin
    if git rev-parse --verify -q origin/audit/ffi > /dev/null; then
        git checkout -q audit/ffi 2>/dev/null || git checkout -qb audit/ffi origin/audit/ffi
        git reset -q --hard origin/audit/ffi
    else
        git checkout -q master && git reset -q --hard origin/master
    fi
    # Also checked before running: a completed audit stays completed even
    # across the daily self-heal reboot.
    if [ -f docs/AUDIT-FFI.md ] && grep -q "^AUDIT COMPLETE$" docs/AUDIT-FFI.md; then
        echo "$(date -Is) audit already complete" >> "$LOG"
        break
    fi
    claude --dangerously-skip-permissions \
        -p "$(cat infra/night-shift/audit-prompt.md)" >> "$LOG" 2>&1
    echo "$(date -Is) audit session $i finished: exit=$?" >> "$LOG"
    if [ -f docs/AUDIT-FFI.md ] && grep -q "^AUDIT COMPLETE$" docs/AUDIT-FFI.md; then
        echo "$(date -Is) audit declared complete" >> "$LOG"
        break
    fi
    sleep 300
done
echo "$(date -Is) === audit driver done ===" >> "$LOG"
