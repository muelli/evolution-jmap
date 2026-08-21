#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Night-shift driver: repeatedly hands Claude Code the next-roadmap-item
# prompt, pushing each green increment. Deployed to ~/ on the runner VM
# and started inside tmux BY THE OPERATOR (deliberately not automated).
#
# Swapping this driver losslessly: `touch ~/.night-shift-stop`, wait for
# the log to show it exited between iterations, then cp the new script and
# relaunch the tmux session. Do NOT gate a swap on
# `pgrep -f dangerously-skip-permissions` from a shell whose OWN command
# line contains that string — pgrep -f matches the waiting shell itself and
# the wait never ends. If you must poll for the session, match the process,
# not your own argv: `pgrep -f '[c]laude --dangerously-skip-permissions'`.
#
# Lifecycle & cost: the driver does NOT loop forever. It exits when it
# can no longer make progress — a drained backlog (several no-op
# iterations) or a usage limit — so the idle watchdog can nap the VM and
# the hourly GCE instance schedule reboots it to poll for new work
# (@reboot relaunches this driver).
#
# Usage limits: Claude Code prints "You've hit your weekly|session limit ·
# resets <time> (UTC)" and exits within seconds. Two things matter about
# that. First it must be *detected* — an earlier version grepped for
# "usage limit", which this message never contains, so 150 rate-limited
# runs were misread as a drained backlog. Second, a *weekly* reset can be
# days away, and the hourly reboot would otherwise fire a fresh (rejected)
# call every hour for the whole week. So on a limit the driver records the
# reset time in $LIMIT_FILE and exits, and every startup refuses to invoke
# Claude at all until that time has passed — a napped, near-zero-cost wait
# that resumes on the first reboot after the reset.

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
cd "$HOME/evolution-jmap" || exit 1
LOG="$HOME/night-shift.log"
PROMPT_FILE="$HOME/night-prompt.md"
LIMIT_FILE="$HOME/.claude-limited-until"   # epoch seconds; present only while limited
STOP_FILE="$HOME/.night-shift-stop"        # one-shot: stop cleanly once, then cleared
PAUSE_FILE="$HOME/.night-shift-paused"     # durable: stay stopped across reboots until removed
ESCALATE_FILE="$HOME/.night-shift-escalate"  # one-shot: model to use for the NEXT iteration only
DEFAULT_MODEL="claude-sonnet-5"            # the shared subscription's workhorse; escalate by exception
DRAIN_LIMIT=3          # consecutive short/no-op iterations (crash/transient) → exit
BLOCKED_LIMIT=3        # consecutive "agent reported BLOCKED" iterations → durable pause
UNKNOWN_RESET_BACKOFF=21600   # 6h, if a limit is seen but its reset can't be parsed

# --- API-key overflow (all state lives on the VM, never in the repo) ---
# When the subscription reaches USAGE_RESERVE_PCT (or hits its hard limit),
# iterations switch to Console API-key billing instead of napping, keeping the
# remaining subscription headroom for the operator's own interactive use.
# The operator provisions $KEYS_FILE by hand: one API key per line, chmod 600.
# A key that fails auth/billing is logged (fingerprint only) to $KEYLOG and
# removed from the list; when no keys remain, behavior reverts to the nap.
KEYS_FILE="$HOME/.night-shift-api-keys"
KEYLOG="$HOME/.night-shift-keys.log"
USAGE_RESERVE_PCT=95
CREDS_FILE="$HOME/.claude/.credentials.json"

# Only these models may be escalated to — a typo or junk in the escalate
# file must not reach the CLI. Anything else falls back to the default.
is_known_model() { case "$1" in claude-sonnet-5|claude-opus-5|claude-fable-5) return 0;; *) return 1;; esac; }

log() { echo "$(date -Is) $*" >> "$LOG"; }

# If the captured session output ($1) shows a Claude Code limit, echo the
# reset time in epoch seconds and return 0; otherwise return 1. Handles the
# real message ("hit your weekly|session limit · resets Aug 15, 8pm (UTC)",
# and the shorter "resets 8:20pm (UTC)"), falls back to a fixed backoff if
# the wording matches but the time does not parse, and keeps the old
# "usage/rate limit" strings as a backstop.
limit_reset_epoch() {
    local out="$1" line tstr epoch
    line=$(grep -oiE "hit your [a-z]+ limit.*\(UTC\)" "$out" | head -1)
    if [ -n "$line" ]; then
        tstr=$(sed -E 's/.*resets //I; s/,//g; s/\(UTC\)/UTC/' <<<"$line")
        epoch=$(date -d "$tstr" +%s 2>/dev/null)
        [ -n "$epoch" ] && echo "$epoch" || echo $(( $(date +%s) + UNKNOWN_RESET_BACKOFF ))
        return 0
    fi
    # Backstop for any other limit wording.
    if grep -qiE "usage limit|rate limit" "$out"; then
        echo $(( $(date +%s) + UNKNOWN_RESET_BACKOFF ))
    fi
}

# First provisioned key, if any. Lines not shaped like a key are ignored, so
# the operator can keep comments in the file.
current_key() { grep -m1 -E '^sk-ant' "$KEYS_FILE" 2>/dev/null; }

# Identifying but non-secret: enough to match a Console key page, useless to
# an attacker. Never write the full key to any log.
key_fingerprint() { local k="$1"; echo "${k:0:10}…${k: -4}"; }

drop_current_key() {   # $1 = key, $2 = one-line reason
    local fp; fp=$(key_fingerprint "$1")
    echo "$(date -Is) DROPPED $fp: $2" >> "$KEYLOG"
    grep -vF "$1" "$KEYS_FILE" > "$KEYS_FILE.tmp" && mv "$KEYS_FILE.tmp" "$KEYS_FILE"
    chmod 600 "$KEYS_FILE" 2>/dev/null
    log "overflow key $fp failed ($2) — removed; $(grep -cE '^sk-ant' "$KEYS_FILE" 2>/dev/null || echo 0) key(s) left (see $KEYLOG)"
}

# Best-effort subscription utilization, read from the same internal endpoint
# Claude Code's /usage view uses (Bearer = the CLI's own OAuth token; the token
# never leaves this function). Undocumented → fail OPEN: on any error this
# prints "unknown" and the driver behaves exactly as it did before this
# feature (hard-limit detection only). Prints "<max-utilization-percent-int>
# <reset-epoch-or-0>" on success.
usage_snapshot() {
    local tok json
    tok=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["claudeAiOauth"]["accessToken"])' \
        "$CREDS_FILE" 2>/dev/null) || { echo unknown; return; }
    [ -n "$tok" ] || { echo unknown; return; }
    json=$(curl -sf -m 10 https://api.anthropic.com/api/oauth/usage \
        -H "Authorization: Bearer $tok" \
        -H "anthropic-beta: oauth-2025-04-20") || { echo unknown; return; }
    printf '%s' "$json" | python3 -c '
import datetime, json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    raise SystemExit(1)
best, reset, best_is_frac = -1.0, 0, False
def walk(o):
    global best, reset, best_is_frac
    if isinstance(o, dict):
        u = o.get("utilization")
        if isinstance(u, (int, float)) and u > best:
            best = float(u)
            best_is_frac = isinstance(u, float) and u <= 1.0
            r = o.get("resets_at") or o.get("reset_at") or 0
            if isinstance(r, str):
                try:
                    reset = int(datetime.datetime.fromisoformat(r.replace("Z", "+00:00")).timestamp())
                except Exception:
                    reset = 0
            elif isinstance(r, (int, float)):
                reset = int(r)
        for v in o.values():
            walk(v)
    elif isinstance(o, list):
        for v in o:
            walk(v)
walk(d)
if best < 0:
    raise SystemExit(1)
if best_is_frac:
    best *= 100    # endpoint variant reporting 0..1 instead of 0..100
print(int(best), reset)
' || echo unknown
}

# Refuse to invoke Claude on the SUBSCRIPTION while a recorded limit is still
# in the future. This is what a reboot during a multi-day weekly limit hits: a
# `date` comparison and an immediate exit, not a rejected API call. With
# operator-provisioned API keys present, we don't exit — the loop below runs
# those iterations in overflow (API-key) mode until the reset passes.
if [ -f "$LIMIT_FILE" ]; then
    until=$(cat "$LIMIT_FILE" 2>/dev/null)
    if [[ "$until" =~ ^[0-9]+$ ]] && [ "$until" -gt "$(date +%s)" ]; then
        if [ -n "$(current_key)" ]; then
            log "subscription limited until $(date -Is -d "@$until") but API keys are provisioned — continuing in overflow mode"
        else
            log "still limited until $(date -Is -d "@$until"); not invoking Claude, exiting to nap"
            exit 0
        fi
    else
        rm -f "$LIMIT_FILE"   # reset has passed
    fi
fi

log "=== night shift starting ==="
consecutive_noop=0
consecutive_blocked=0
while true; do
    # Both flags are checked only between iterations, never mid-session, so a
    # running increment always finishes and pushes before the driver exits —
    # touching either costs no work and no tokens.
    #
    # Durable pause: stays in effect across the idle-shutdown/hourly-reboot
    # cycle, because it is NOT cleared — every @reboot relaunch re-reads it
    # here and exits again. `touch ~/.night-shift-paused` to pause after the
    # current iteration; `rm ~/.night-shift-paused` (and relaunch, or wait for
    # the next hourly reboot) to resume.
    if [ -f "$PAUSE_FILE" ]; then
        log "paused (found $PAUSE_FILE); exiting between iterations. rm it to resume."
        exit 0
    fi
    # One-shot stop: cleared on use, so the next relaunch runs normally. This
    # is the lossless driver-swap seam: touch it, wait for the shift to exit,
    # replace the script, relaunch.
    if [ -f "$STOP_FILE" ]; then
        rm -f "$STOP_FILE"
        log "stop requested; exiting cleanly between iterations (no work lost)"
        exit 0
    fi
    # Start every iteration from a clean, current master. An iteration killed
    # mid-increment — e.g. the weekly usage limit is reached while the agent is
    # editing — leaves uncommitted changes (and possibly an interrupted
    # rebase/merge). `git pull --rebase` then refuses, the `|| true` masks it,
    # and we would launch a fresh agent on a stale, dirty tree. A cleanly-
    # finished iteration commits+pushes and exits, so there is nothing to lose
    # here: only a killed iteration's partial, uncommitted edits are discarded.
    # Any green local commit is kept (reset to HEAD, not origin) and pushed with
    # the next increment. We deliberately do NOT `git clean`: it could delete
    # untracked operator-provisioned files, and stray untracked files do not
    # block a rebase anyway.
    git rebase --abort >/dev/null 2>&1 || true
    git merge  --abort >/dev/null 2>&1 || true
    git reset  --hard  >> "$LOG" 2>&1 || true
    git pull --rebase --quiet >> "$LOG" 2>&1 || true

    # --- Auth mode for this iteration: subscription (default) or overflow ---
    # Overflow triggers when the subscription's utilization reaches the
    # operator's reserve threshold, or when a hard limit was recorded. With no
    # keys left/provisioned, the reserve is protected by napping instead.
    mode=subscription; iter_key=""
    snap=$(usage_snapshot)
    if [ "$snap" != "unknown" ]; then
        util=${snap%% *}; snap_reset=${snap##* }
        log "subscription usage: ${util}% (reserve ${USAGE_RESERVE_PCT}%)"
    else
        util=""; snap_reset=0
    fi
    over=0
    if [ -f "$LIMIT_FILE" ]; then
        until=$(cat "$LIMIT_FILE" 2>/dev/null)
        if [[ "$until" =~ ^[0-9]+$ ]] && [ "$until" -gt "$(date +%s)" ]; then over=1; else rm -f "$LIMIT_FILE"; fi
    fi
    if [ -n "$util" ] && [ "$util" -ge "$USAGE_RESERVE_PCT" ]; then over=1; fi
    if [ "$over" = 1 ]; then
        iter_key=$(current_key)
        if [ -n "$iter_key" ]; then
            mode=overflow
            log "subscription at reserve/limit — this iteration runs on API key $(key_fingerprint "$iter_key")"
        else
            if [ ! -f "$LIMIT_FILE" ]; then
                if [ "$snap_reset" -gt "$(date +%s)" ] 2>/dev/null; then echo "$snap_reset" > "$LIMIT_FILE"
                else echo $(( $(date +%s) + UNKNOWN_RESET_BACKOFF )) > "$LIMIT_FILE"; fi
            fi
            log "reserve ${USAGE_RESERVE_PCT}% reached and no API keys in $KEYS_FILE — napping until $(date -Is -d "@$(cat "$LIMIT_FILE")") to keep the remaining quota for the operator"
            exit 0
        fi
    fi

    # Model for this iteration: the shared subscription's Sonnet by default,
    # or a one-shot escalation to Opus/Fable for a single hard item. The
    # escalation is requested either by a human (`echo claude-opus-5 >
    # ~/.night-shift-escalate`) or by the agent itself, which the prompt tells
    # to defer an increment beyond Sonnet's reliable reach rather than botch it.
    model="$DEFAULT_MODEL"; escalated=0
    if [ -s "$ESCALATE_FILE" ]; then
        want=$(head -1 "$ESCALATE_FILE" | tr -dc 'a-z0-9-')
        if is_known_model "$want"; then model="$want"; escalated=1; log "escalating this iteration to $model"
        else log "ignoring unknown escalation model '$want'; using $DEFAULT_MODEL"; rm -f "$ESCALATE_FILE"; fi
    fi

    start=$(date +%s)
    out=$(mktemp)
    if [ "$mode" = overflow ]; then
        ANTHROPIC_API_KEY="$iter_key" claude --model "$model" --dangerously-skip-permissions -p "$(cat "$PROMPT_FILE")" > "$out" 2>&1
    else
        claude --model "$model" --dangerously-skip-permissions -p "$(cat "$PROMPT_FILE")" > "$out" 2>&1
    fi
    status=$?
    cat "$out" >> "$LOG"
    duration=$(( $(date +%s) - start ))
    log "iteration finished: model=$model mode=$mode exit=$status duration=${duration}s"

    # Consume a one-shot escalation only after it was actually used, so an
    # escalation the *agent* just wrote (on a default-model triage pass) still
    # applies to the next iteration rather than being cleared unused.
    [ "$escalated" = 1 ] && rm -f "$ESCALATE_FILE"

    # Overflow-mode key failure: auth/billing rejection means THIS key is bad,
    # not that work is blocked or the subscription is limited. Log it
    # (fingerprint only), drop it from the list, and retry promptly — the next
    # iteration picks the next key, or naps if none remain.
    if [ "$mode" = overflow ]; then
        keyfail=$(grep -oiE "invalid api key|invalid x-api-key|authentication_error|credit balance is too low|permission_error|please run /login" "$out" | head -1)
        if [ -n "$keyfail" ]; then
            drop_current_key "$iter_key" "$keyfail"
            rm -f "$out"
            sleep 10
            continue
        fi
    fi

    reset_epoch=$(limit_reset_epoch "$out")
    # The prompt tells the agent to print exactly this line (and make no pointer
    # commit) when there is no unblocked work to progress. It is the reliable
    # "blocked" signal — unlike duration, since a blocked session that re-surveys
    # and writes prose runs well over 120s and would never trip the drain path.
    blocked=0; grep -q "NIGHT-SHIFT: BLOCKED" "$out" && blocked=1
    rm -f "$out"
    # Only a SUBSCRIPTION iteration can hit the subscription's limit. In
    # overflow mode any "rate limit" wording is a transient API-side 429 —
    # recording it as a multi-day nap would be wrong; the drain logic retries.
    if [ "$mode" = subscription ] && [ -n "$reset_epoch" ] && [ "$reset_epoch" -gt "$(date +%s)" ]; then
        echo "$reset_epoch" > "$LIMIT_FILE"
        if [ -n "$(current_key)" ]; then
            log "hit Claude limit; resets $(date -Is -d "@$reset_epoch"). Recorded; API keys present — switching to overflow mode."
            sleep 10
            continue
        fi
        log "hit Claude limit; resets $(date -Is -d "@$reset_epoch"). Recorded; exiting to nap — no retry until then."
        exit 0
    fi

    # Every sleep below is kept UNDER the VM's idle-watchdog timeout (5 min), so a
    # working loop keeps the VM awake but the moment the driver stops invoking
    # Claude — by exiting (drain/limit/stop) or pausing (blocked) — the VM naps.

    # Blocked: no unblocked work. Do not keep waking hourly to re-confirm it (that
    # burns the shared quota and never naps). After BLOCKED_LIMIT in a row, set
    # the DURABLE pause so it stays down across reboots until a human unblocks
    # something and removes the sentinel.
    if [ "$blocked" = 1 ]; then
        consecutive_blocked=$(( consecutive_blocked + 1 ))
        log "agent reported BLOCKED ${consecutive_blocked}/${BLOCKED_LIMIT}"
        if [ "$consecutive_blocked" -ge "$BLOCKED_LIMIT" ]; then
            touch "$PAUSE_FILE"
            log "blocked ${BLOCKED_LIMIT}x — pausing (touched $PAUSE_FILE). Unblock work, then rm it and relaunch (or wait for a reboot)."
            exit 0
        fi
        sleep 120
        continue
    fi
    consecutive_blocked=0

    if [ "$duration" -lt 120 ]; then
        # Fast exit, no BLOCKED marker, no limit → a crash or transient error.
        # A few in a row → exit; the hourly reboot retries from a clean slate.
        consecutive_noop=$(( consecutive_noop + 1 ))
        log "short iteration ${consecutive_noop}/${DRAIN_LIMIT}"
        if [ "$consecutive_noop" -ge "$DRAIN_LIMIT" ]; then
            log "drained or agent crashing - exiting; hourly reboot re-checks for new work"
            exit 0
        fi
        sleep 120
    else
        consecutive_noop=0
        sleep 240
    fi
done
