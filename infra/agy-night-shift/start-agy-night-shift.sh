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

# Log every exit so it's obvious where the driver terminated.
trap 'log "=== agy night shift exiting (exit=$?, line=$LINENO) ==="' EXIT

# Parse "Resets in 1h39m50s" (or "39m50s", "50s") into seconds.
parse_reset_seconds() {
    local str="$1"
    local hours=0 mins=0 secs=0
    [[ "$str" =~ ([0-9]+)h ]] && hours="${BASH_REMATCH[1]}"
    [[ "$str" =~ ([0-9]+)m ]] && mins="${BASH_REMATCH[1]}"
    [[ "$str" =~ ([0-9]+)s ]] && secs="${BASH_REMATCH[1]}"
    echo $(( hours * 3600 + mins * 60 + secs ))
}

log "=== agy night shift starting ==="

mapfile -t AVAILABLE_MODELS < <(agy models </dev/null | grep -v "Fetching" | awk '{print $1}' || true)
CURRENT_MODEL_INDEX=-1 # -1 means use the CLI default model
QUOTA_RESET_SECONDS=3600 # fallback sleep if we can't parse the reset time
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

    # Detect quota/usage limit.
    # Exact message observed from agy: "Error: Individual quota reached. Please upgrade your subscription to increase your limits. Resets in Xh Ym Zs."
    # Note: agy exits 0 even on quota errors, so we cannot rely on exit code.
    quota_hit=0
    if grep -qiE "Individual quota reached|usage limit|quota exceeded|resource exhausted|rate limit" "$out"; then
        log "Quota error detected in output."
        quota_hit=1
        # Try to parse the reset time from the message, e.g. "Resets in 1h39m50s."
        reset_str=$(grep -oiE "Resets in [0-9hms]+" "$out" | head -1 | awk '{print $3}')
        if [ -n "$reset_str" ]; then
            QUOTA_RESET_SECONDS=$(parse_reset_seconds "$reset_str")
            log "Quota resets in ${QUOTA_RESET_SECONDS}s (parsed from: $reset_str)."
        fi
    fi
    rm -f "$out"

    if [ "$quota_hit" -eq 1 ]; then
        CURRENT_MODEL_INDEX=$(( CURRENT_MODEL_INDEX + 1 ))
        if [ "$CURRENT_MODEL_INDEX" -lt "${#AVAILABLE_MODELS[@]}" ]; then
            log "Switching to fallback model: ${AVAILABLE_MODELS[$CURRENT_MODEL_INDEX]}."
            consecutive_noop=0  # reset so drain detection doesn't fire prematurely
            continue
        else
            # All models exhausted — sleep until the earliest quota resets, then try again.
            sleep_secs=$(( QUOTA_RESET_SECONDS + 60 ))
            log "All models hit quota. Sleeping ${sleep_secs}s until quota resets, then retrying default model."
            sleep "$sleep_secs"
            CURRENT_MODEL_INDEX=-1
            consecutive_noop=0
            continue
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
