#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Antigravity Night-shift driver. Started BY THE OPERATOR inside tmux.
# Ensure that the agy CLI is installed and accessible in PATH.

set -euo pipefail

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
cd "$HOME/evolution-jmap" || exit 1
LOG="$HOME/agy-night-shift.log"
PROMPT_FILE="$HOME/evolution-jmap/infra/night-shift/night-prompt.md"

log() { echo "$(date -Is) $*" >> "$LOG"; }

log "=== agy night shift starting ==="

consecutive_noop=0
while true; do
    git pull --rebase --quiet >> "$LOG" 2>&1 || true
    start=$(date +%s)
    log "launching agy in autonomous mode with /goal"
    # Using --dangerously-skip-permissions to bypass prompts, and --print to run non-interactively.
    agy --dangerously-skip-permissions --print-timeout 8h --print "/goal $(cat "$PROMPT_FILE")" >> "$LOG" 2>&1
    status=$?
    
    duration=$(( $(date +%s) - start ))
    log "agy finished: exit=$status duration=${duration}s"

    if [ "$duration" -lt 120 ]; then
        consecutive_noop=$(( consecutive_noop + 1 ))
        log "short iteration ${consecutive_noop}/3"
        if [ "$consecutive_noop" -ge 3 ]; then
            log "backlog appears drained or agent crashed repeatedly - exiting. VM will be napped by idle-watchdog once idle."
            exit 0
        fi
        sleep 300
    else
        consecutive_noop=0
        sleep 600
    fi
done
