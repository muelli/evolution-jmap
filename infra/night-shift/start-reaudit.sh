#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Recurring security re-audit driver. Each launch forks a fresh dated
# branch off current master, re-verifies the prior findings still hold,
# and audits everything new since the last audit (see the ROADMAP
# "Recurring security re-audit" directive). Runs in its OWN clone
# (~/audit-ffi) and pushes only to its dated branch — never master, never
# the roadmap shift's checkout. Started BY THE OPERATOR inside tmux.
#
# To run it on a cadence instead of by hand, add a weekly cron entry, e.g.
#   0 3 * * 1 tmux new-session -d -s reaudit "$HOME/start-reaudit.sh"
# (A fresh dated branch per week; the driver name contains "claude" via
# the session it launches, and long audits keep the watchdog satisfied.)

set -uo pipefail
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
LOG="$HOME/reaudit.log"

# The shell has a real clock (unlike the workflow sandbox); date is fine.
DATESTAMP=$(date -u +%Y%m%d)
BRANCH="audit/reaudit-${DATESTAMP}"
export REAUDIT_REPORT="docs/AUDIT-FFI-${DATESTAMP}.md"

mkdir -p "$HOME/audit-ffi"
cd "$HOME/audit-ffi" || exit 1
[ -d evolution-jmap ] || git clone -q git@github.com:muelli/evolution-jmap.git
cd evolution-jmap || exit 1
git config user.name "Tobias Mueller"
git config user.email "muelli@cryptobitch.de"

echo "$(date -Is) === re-audit ${DATESTAMP} starting, report ${REAUDIT_REPORT} ===" >> "$LOG"
for i in $(seq 1 8); do
    git fetch -q origin
    if git rev-parse --verify -q "origin/${BRANCH}" > /dev/null; then
        git checkout -q "$BRANCH" 2>/dev/null || git checkout -qb "$BRANCH" "origin/${BRANCH}"
        git reset -q --hard "origin/${BRANCH}"
    else
        git checkout -q -B "$BRANCH" origin/master   # fresh dated branch off current master
        git push -q -u origin "$BRANCH" 2>>"$LOG" || true
    fi
    # Reboot-safe: a finished re-audit stays finished.
    if [ -f "$REAUDIT_REPORT" ] && grep -q "^AUDIT COMPLETE$" "$REAUDIT_REPORT"; then
        echo "$(date -Is) re-audit ${DATESTAMP} already complete" >> "$LOG"
        break
    fi
    claude --dangerously-skip-permissions -p "$(cat infra/night-shift/reaudit-prompt.md)" >> "$LOG" 2>&1
    echo "$(date -Is) re-audit session $i finished: exit=$?" >> "$LOG"
    if [ -f "$REAUDIT_REPORT" ] && grep -q "^AUDIT COMPLETE$" "$REAUDIT_REPORT"; then
        echo "$(date -Is) re-audit ${DATESTAMP} declared complete" >> "$LOG"
        break
    fi
    sleep 300
done
echo "$(date -Is) === re-audit driver done ===" >> "$LOG"
