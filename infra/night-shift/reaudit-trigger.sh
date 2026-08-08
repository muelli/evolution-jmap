#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Fires the recurring security re-audit exactly once, when the mail
# surface has settled. Polls docs/MILESTONES.md (which the roadmap shift
# appends completion tags to) for `M5 COMPLETE`; on first sighting it
# launches the re-audit stream and drops a sentinel so it never
# double-launches. Meant to run from cron every 30 min:
#
#   */30 * * * * $HOME/reaudit-trigger.sh
#
# The crontab lives on the persistent disk, so this survives the daily
# self-heal reboot without an @reboot entry. Installed by the operator
# (it starts a --dangerously-skip-permissions stream, so a human enables
# it knowingly — same trigger model as the other streams).

set -uo pipefail
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
LOG="$HOME/reaudit.log"
SENTINEL="$HOME/.reaudit-triggered"
REPO="$HOME/evolution-jmap"          # read the roadmap shift's checkout, do not write it

[ -f "$SENTINEL" ] && exit 0          # already fired

# Read MILESTONES.md from the remote's master without disturbing any
# working tree (the roadmap shift owns $REPO).
git -C "$REPO" fetch -q origin 2>/dev/null || exit 0
milestones=$(git -C "$REPO" show origin/master:docs/MILESTONES.md 2>/dev/null) || exit 0

if grep -qE '^M5 COMPLETE ' <<<"$milestones"; then
    echo "$(date -Is) reaudit-trigger: M5 COMPLETE seen, launching re-audit" >> "$LOG"
    touch "$SENTINEL"
    if command -v tmux >/dev/null && ! tmux has-session -t reaudit 2>/dev/null; then
        tmux new-session -d -s reaudit "$HOME/start-reaudit.sh"
        echo "$(date -Is) reaudit-trigger: launched tmux session 'reaudit'" >> "$LOG"
    fi
fi
