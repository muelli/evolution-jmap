#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Antigravity POLISH-shift driver. Runs on the agy VM as the `runner` user (agy
# is installed and OAuth-authed under /home/runner). Started inside tmux BY THE
# OPERATOR (agy runs with --dangerously-skip-permissions).
#
# Lane: LOW-PRIORITY POLISH only, on the `antigravity` branch, which the
# maintainer merges into `master` every so often. Claude works the priority
# items on master. Each iteration this driver stays on antigravity, merges the
# latest master in (so polish builds on current code), runs ONE agy increment
# against infra/agy-night-shift/agy-prompt.md, and the agent commits+pushes to
# antigravity. Lane rules live in that prompt.
#
# Sentinels (checked only between iterations, so a running increment always
# finishes first):
#   ~/.agy-shift-paused  durable  — stays down across reboots. A "blocked <hash>"
#     pause (lane drained) AUTO-CLEARS when docs/AGY-TASKS.md changes on
#     origin/master: refill the lane and agy resumes on the next self-heal start,
#     no SSH needed (the skill's "steer by pushing a commit"). A "merge-conflict"
#     or a manual/empty pause stays until removed by hand.
#   ~/.agy-shift-stop    one-shot — one clean exit (the lossless driver-swap seam)
# Blocked: the prompt makes agy print "AGY-SHIFT: BLOCKED" when the polish lane
# has nothing unblocked; after BLOCKED_LIMIT in a row the driver sets the durable
# pause rather than spinning (mirrors the Claude driver).

set -uo pipefail   # deliberately not -e: git/agy failures are handled inline

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
cd "$HOME/evolution-jmap" || exit 1
LOG="$HOME/agy-night-shift.log"
PROMPT_FILE="$HOME/evolution-jmap/infra/agy-night-shift/agy-prompt.md"
PAUSE_FILE="$HOME/.agy-shift-paused"
STOP_FILE="$HOME/.agy-shift-stop"
BLOCKED_LIMIT=3        # consecutive "AGY-SHIFT: BLOCKED" reports → durable pause
DRAIN_LIMIT=3          # consecutive short/no-op iterations (crash/transient) → exit
MERGE_CONFLICT_LIMIT=5 # consecutive failures to merge master in → durable pause
QUOTA_RESET_SECONDS=3600

# --- API-key overflow (all state on the VM, never in the repo) ---
# When the Google account's quota is exhausted across every model, iterations
# switch to Gemini API-key billing instead of napping. The operator provisions
# $KEYS_FILE by hand: one API key per line, nothing else, chmod 600. A key that
# fails hard (invalid/permission) is removed and logged fingerprint-only to
# $KEYLOG; a key that merely hits its own quota is rotated to the end of the
# list. Every fresh boot starts back on the account (free) quota.
KEYS_FILE="$HOME/.agy-api-keys"
KEYLOG="$HOME/.agy-keys.log"

current_key() { grep -m1 -vE '^\s*(#|$)' "$KEYS_FILE" 2>/dev/null; }
key_count()   { grep -cvE '^\s*(#|$)' "$KEYS_FILE" 2>/dev/null || echo 0; }
key_fingerprint() { local k="$1"; echo "${k:0:6}…${k: -4}"; }

drop_current_key() {   # $1 = key, $2 = one-line reason
    local fp; fp=$(key_fingerprint "$1")
    echo "$(date -Is) DROPPED $fp: $2" >> "$KEYLOG"
    grep -vF "$1" "$KEYS_FILE" > "$KEYS_FILE.tmp" && mv "$KEYS_FILE.tmp" "$KEYS_FILE"
    chmod 600 "$KEYS_FILE" 2>/dev/null
    log "overflow key $fp failed hard ($2) — removed; $(key_count) key(s) left (see $KEYLOG)"
}

rotate_current_key() { # $1 = key, $2 = reason — key exhausted its own quota, try the next
    local fp; fp=$(key_fingerprint "$1")
    echo "$(date -Is) ROTATED $fp: $2" >> "$KEYLOG"
    { grep -vF "$1" "$KEYS_FILE"; echo "$1"; } > "$KEYS_FILE.tmp" && mv "$KEYS_FILE.tmp" "$KEYS_FILE"
    chmod 600 "$KEYS_FILE" 2>/dev/null
    log "overflow key $fp quota-exhausted — rotated to end of list"
}

log() { echo "$(date -Is) $*" >> "$LOG"; }
trap 'log "=== agy shift exiting (line=$LINENO) ==="' EXIT

# Parse "1h39m50s" / "39m50s" / "50s" into seconds.
parse_reset_seconds() {
    local str="$1" hours=0 mins=0 secs=0
    [[ "$str" =~ ([0-9]+)h ]] && hours="${BASH_REMATCH[1]}"
    [[ "$str" =~ ([0-9]+)m ]] && mins="${BASH_REMATCH[1]}"
    [[ "$str" =~ ([0-9]+)s ]] && secs="${BASH_REMATCH[1]}"
    echo $(( hours * 3600 + mins * 60 + secs ))
}

log "=== agy polish shift starting ==="
# One model per provider family so quota-cycling does not spin same-bucket models.
mapfile -t AVAILABLE_MODELS < <(agy models </dev/null 2>/dev/null | grep -v "Fetching" | awk '{print $1}' | awk -F'-' '!seen[$1]++ { print $0 }' || true)
CURRENT_MODEL_INDEX=-1   # -1 = the CLI default model
consecutive_blocked=0
consecutive_noop=0
consecutive_merge_fail=0
overflow=0               # 0 = Google-account quota; 1 = operator-provisioned API key
key_quota_streak=0       # consecutive key-quota hits; >= key_count means every key is spent

while true; do
    if [ -f "$PAUSE_FILE" ]; then
        # A "blocked <hash>" pause (lane drained) auto-clears once the maintainer
        # refills docs/AGY-TASKS.md on origin/master — so a git push resumes agy on
        # the next self-heal start, no SSH. Any other content (merge-conflict, or a
        # manual/empty pause) stays until removed by hand.
        read -r _pause_reason _paused_tasks_hash < "$PAUSE_FILE" 2>/dev/null || true
        if [ "${_pause_reason:-}" = "blocked" ]; then
            git fetch origin --quiet >> "$LOG" 2>&1 || true
            _cur_tasks_hash=$(git rev-parse origin/master:docs/AGY-TASKS.md 2>/dev/null || true)
            if [ -n "$_cur_tasks_hash" ] && [ "$_cur_tasks_hash" != "${_paused_tasks_hash:-}" ]; then
                rm -f "$PAUSE_FILE"
                log "AGY-TASKS.md changed since the BLOCKED pause — maintainer refilled the lane; auto-resuming."
            else
                log "paused (blocked, lane unchanged); exiting. Refill docs/AGY-TASKS.md to auto-resume, or rm $PAUSE_FILE."
                exit 0
            fi
        else
            log "paused (${_pause_reason:-manual}); exiting. rm $PAUSE_FILE to resume."
            exit 0
        fi
    fi
    if [ -f "$STOP_FILE" ]; then rm -f "$STOP_FILE"; log "stop requested; exiting cleanly between iterations."; exit 0; fi

    # Stay on the antigravity polish branch and keep it current with master. We
    # are the sole writer of antigravity, so pull --rebase on it is safe; the
    # merge of origin/master brings in Claude's latest priority work.
    git fetch origin --quiet >> "$LOG" 2>&1 || true
    git checkout antigravity >> "$LOG" 2>&1 \
        || git checkout -b antigravity origin/antigravity >> "$LOG" 2>&1 \
        || git checkout -b antigravity origin/master >> "$LOG" 2>&1 || true
    git pull --rebase --quiet origin antigravity >> "$LOG" 2>&1 || true
    if git merge --no-edit origin/master >> "$LOG" 2>&1; then
        consecutive_merge_fail=0
    else
        git merge --abort >> "$LOG" 2>&1 || true
        consecutive_merge_fail=$(( consecutive_merge_fail + 1 ))
        log "merge of origin/master conflicted (${consecutive_merge_fail}/${MERGE_CONFLICT_LIMIT}); skipping this round"
        if [ "$consecutive_merge_fail" -ge "$MERGE_CONFLICT_LIMIT" ]; then
            echo 'merge-conflict' > "$PAUSE_FILE"
            log "cannot merge master into antigravity ${MERGE_CONFLICT_LIMIT}x — the polish lane has collided with master's. Pausing; resolve the branch conflict, then rm $PAUSE_FILE and relaunch. (A task refill will NOT auto-clear this one — it needs a human merge.)"
            exit 0
        fi
    fi

    model_arg=""; current_model_name="default"
    if [ "$CURRENT_MODEL_INDEX" -ge 0 ] && [ "$CURRENT_MODEL_INDEX" -lt "${#AVAILABLE_MODELS[@]}" ]; then
        current_model_name="${AVAILABLE_MODELS[$CURRENT_MODEL_INDEX]}"; model_arg="--model $current_model_name"
    fi
    iter_key=""
    if [ "$overflow" = 1 ]; then
        iter_key=$(current_key)
        if [ -z "$iter_key" ]; then
            log "overflow mode but no API keys left in $KEYS_FILE — exiting to nap; hourly reboot retries the account quota"
            exit 0
        fi
        log "launching agy (/goal, model: $current_model_name, API key $(key_fingerprint "$iter_key"))"
    else
        log "launching agy (/goal, model: $current_model_name)"
    fi

    start=$(date +%s); out=$(mktemp)
    if [ "$overflow" = 1 ]; then
        GEMINI_API_KEY="$iter_key" agy $model_arg --dangerously-skip-permissions --print-timeout 8h --print "/goal $(cat "$PROMPT_FILE")" > "$out" 2>&1
    else
        agy $model_arg --dangerously-skip-permissions --print-timeout 8h --print "/goal $(cat "$PROMPT_FILE")" > "$out" 2>&1
    fi
    status=$?
    cat "$out" >> "$LOG"
    duration=$(( $(date +%s) - start ))
    log "agy finished: exit=$status duration=${duration}s"

    # Quota: agy exits 0 even on quota errors, so detect from the output.
    quota_hit=0
    if grep -qiE "Individual quota reached|usage limit|quota exceeded|resource exhausted|rate limit" "$out" 2>/dev/null; then
        quota_hit=1
        reset_str=$(grep -oiE "Resets in [0-9hms ]+" "$out" 2>/dev/null | head -1 | sed -E 's/Resets in //I; s/ //g') || true
        [ -n "$reset_str" ] && QUOTA_RESET_SECONDS=$(parse_reset_seconds "$reset_str")
        log "quota reached; reset ~${QUOTA_RESET_SECONDS}s"
    fi
    # Hard key failure (invalid/revoked/unauthorized — NOT quota): remove the
    # key, log it, and retry promptly with the next one.
    if [ "$overflow" = 1 ]; then
        keyfail=$(grep -oiE "API key not valid|API_KEY_INVALID|invalid api key|PERMISSION_DENIED|unauthorized" "$out" 2>/dev/null | head -1)
        if [ -n "$keyfail" ]; then
            drop_current_key "$iter_key" "$keyfail"
            rm -f "$out"
            if [ -z "$(current_key)" ]; then
                log "no API keys left — exiting to nap; hourly reboot retries the account quota"
                exit 0
            fi
            sleep 10; continue
        fi
    fi

    blocked=0; grep -q "AGY-SHIFT: BLOCKED" "$out" 2>/dev/null && blocked=1
    rm -f "$out"

    # Sleeps below stay UNDER the VM idle-watchdog timeout so a working loop keeps
    # the VM awake; the VM naps once the driver exits (quota/drain/stop) or pauses.

    if [ "$quota_hit" -eq 1 ]; then
        consecutive_blocked=0; consecutive_noop=0
        if [ "$overflow" = 1 ]; then
            # In overflow mode a quota message is THIS KEY's own limit, not the
            # account's. Rotate it to the back and try the next; once every key
            # in the list has hit quota in a row, they are all spent — nap.
            rotate_current_key "$iter_key" "key quota exhausted"
            key_quota_streak=$(( key_quota_streak + 1 ))
            if [ "$key_quota_streak" -ge "$(key_count)" ]; then
                log "every provisioned API key is quota-exhausted — exiting to nap; hourly reboot retries"
                exit 0
            fi
            sleep 10; continue
        fi
        CURRENT_MODEL_INDEX=$(( CURRENT_MODEL_INDEX + 1 ))
        if [ "$CURRENT_MODEL_INDEX" -lt "${#AVAILABLE_MODELS[@]}" ]; then
            log "switching to fallback model ${AVAILABLE_MODELS[$CURRENT_MODEL_INDEX]}"; continue
        fi
        if [ -n "$(current_key)" ]; then
            overflow=1; CURRENT_MODEL_INDEX=-1; key_quota_streak=0
            log "account quota exhausted on all models — switching to API-key overflow ($(key_count) key(s) provisioned)"
            continue
        fi
        log "all models hit quota — exiting to nap; hourly reboot resumes after reset"
        exit 0
    fi

    # Recover to the default model after a healthy long run; a healthy overflow
    # run also resets the every-key-spent streak.
    [ "$duration" -ge 120 ] && [ "$status" -eq 0 ] && { CURRENT_MODEL_INDEX=-1; key_quota_streak=0; }

    # Blocked polish lane → durable pause after BLOCKED_LIMIT in a row.
    if [ "$blocked" = 1 ]; then
        consecutive_blocked=$(( consecutive_blocked + 1 ))
        log "agy reported BLOCKED ${consecutive_blocked}/${BLOCKED_LIMIT}"
        if [ "$consecutive_blocked" -ge "$BLOCKED_LIMIT" ]; then
            printf 'blocked %s\n' "$(git rev-parse origin/master:docs/AGY-TASKS.md 2>/dev/null)" > "$PAUSE_FILE"
            log "polish lane blocked ${BLOCKED_LIMIT}x — pausing. Refill docs/AGY-TASKS.md to auto-resume on the next self-heal (no SSH), or rm $PAUSE_FILE and relaunch."
            exit 0
        fi
        sleep 120; continue
    fi
    consecutive_blocked=0

    if [ "$duration" -lt 120 ]; then
        consecutive_noop=$(( consecutive_noop + 1 ))
        log "short iteration ${consecutive_noop}/${DRAIN_LIMIT}"
        if [ "$consecutive_noop" -ge "$DRAIN_LIMIT" ]; then
            log "drained or agy crashing — exiting; hourly reboot re-checks"
            exit 0
        fi
        sleep 120
    else
        consecutive_noop=0
        sleep 240
    fi
done
