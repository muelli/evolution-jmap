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

git pull --rebase --quiet >> "$LOG" 2>&1 || true

log "launching agy in autonomous mode with /goal"
# Using --yolo to bypass prompts, and /goal to keep it running until the task is complete.
agy --yolo -m "/goal $(cat "$PROMPT_FILE")" >> "$LOG" 2>&1
status=$?

log "agy finished: exit=$status. VM will be napped by idle-watchdog once idle."
