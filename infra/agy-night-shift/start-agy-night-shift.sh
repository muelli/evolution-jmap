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

AVAILABLE_MODELS=($(agy models </dev/null | grep -v "Fetching" | awk '{print $1}'))
CURRENT_MODEL_INDEX=-1 # -1 means use the CLI default model
consecutive_noop=0
while true; do
    git pull --rebase --quiet >> "$LOG" 2>&1 || true
    start=$(date +%s)
    
    model_arg=""
    current_model_name="default"
    if [ "$CURRENT_MODEL_INDEX" -ge 0 ] && [ "$CURRENT_MODEL_INDEX" -lt "${#AVAILABLE_MODELS[@]}" ]; then
        current_model_name="${AVAILABLE_MODELS[$CURRENT_MODEL_INDEX]}"
        model_arg="--model $current_model_name"
    fi
    
    log "launching agy in autonomous mode with /goal (model: $current_model_name)"
    
    out=$(mktemp)
    agy $model_arg --dangerously-skip-permissions --print-timeout 8h --print "/goal $(cat "$PROMPT_FILE")" > "$out" 2>&1
    status=$?
    cat "$out" >> "$LOG"
    
    duration=$(( $(date +%s) - start ))
    log "agy finished: exit=$status duration=${duration}s"

    # Detect quota/usage limit: either by explicit message in output, or by
    # a fast failure (< 30s) with non-zero exit — agy often just exits silently.
    quota_hit=0
    if grep -qiE "usage limit|quota exceeded|resource exhausted|rate limit|429" "$out"; then
        log "Quota error detected in output."
        quota_hit=1
    elif [ "$status" -ne 0 ] && [ "$duration" -lt 30 ]; then
        log "Fast non-zero exit (${duration}s) — likely a quota/auth error."
        quota_hit=1
    fi
    rm -f "$out"

    if [ "$quota_hit" -eq 1 ]; then
        CURRENT_MODEL_INDEX=$(( CURRENT_MODEL_INDEX + 1 ))
        if [ "$CURRENT_MODEL_INDEX" -lt "${#AVAILABLE_MODELS[@]}" ]; then
            log "Switching to fallback model: ${AVAILABLE_MODELS[$CURRENT_MODEL_INDEX]}."
            consecutive_noop=0  # reset so drain detection doesn't fire prematurely
            continue
        else
            log "Usage limit reached for ALL fallback models. Exiting so VM naps."
            exit 0
        fi
    fi

    # Reset model to default after a successful long shift (quota may have recovered)
    if [ "$duration" -ge 120 ] && [ "$status" -eq 0 ]; then
        CURRENT_MODEL_INDEX=-1
    fi

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
