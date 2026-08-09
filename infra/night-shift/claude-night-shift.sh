#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Night-shift driver: repeatedly hands Claude Code the next-roadmap-item
# prompt, pushing each green increment. Deployed to ~/ on the runner VM
# and started inside tmux BY THE OPERATOR (deliberately not automated).
#
# Lifecycle & cost: the driver does NOT loop forever. It exits when it
# can no longer make progress — a drained backlog (several no-op
# iterations) or a usage limit — so that (a) it stops spending tokens on
# sessions that conclude "nothing to do", and (b) the idle watchdog can
# nap the VM. The hourly GCE instance schedule then reboots the VM to
# poll for new work, and the @reboot cron relaunches this driver. So
# "wait for more work" and "wait for a quota reset" are both handled by
# nap + hourly reboot rather than by an expensive in-VM sleep. NOTE: the
# watchdog's activity signal is the running `claude` session's argv, not
# this script's name, so this driver sleeping or exiting never keeps the
# VM alive by itself.

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
cd "$HOME/evolution-jmap" || exit 1
LOG="$HOME/night-shift.log"
PROMPT_FILE="$HOME/night-prompt.md"
DRAIN_LIMIT=3          # consecutive no-op iterations that mean "backlog drained"

log() { echo "$(date -Is) $*" >> "$LOG"; }

# If $1 (a captured session log) shows a usage limit, echo a short human
# ETA for the reset and return 0; otherwise return 1. Used only for an
# informative log line now — the driver exits on a limit rather than
# sleeping until reset, because sleeping in-VM would pay for idle time the
# nap is meant to save.
usage_limit_eta() {
    local out="$1" reset tstr
    grep -qiE "usage limit|rate limit" "$out" || return 1
    reset=$(grep -oE '\|[0-9]{10}' "$out" | tr -d '|' | head -1)
    if [ -z "$reset" ]; then
        tstr=$(grep -oiE "resets? at [0-9apm: ]{1,8}" "$out" | head -1 | sed -E 's/resets? at //I')
        [ -n "$tstr" ] && reset=$(date -d "$tstr" +%s 2>/dev/null)
    fi
    if [ -n "$reset" ]; then echo "resets ~$(date -Is -d "@$reset" 2>/dev/null)"; else echo "reset time unknown"; fi
    return 0
}

log "=== night shift starting ==="
consecutive_noop=0
while true; do
    git pull --rebase --quiet >> "$LOG" 2>&1 || true
    start=$(date +%s)
    out=$(mktemp)
    claude --dangerously-skip-permissions -p "$(cat "$PROMPT_FILE")" > "$out" 2>&1
    status=$?
    cat "$out" >> "$LOG"
    duration=$(( $(date +%s) - start ))
    log "iteration finished: exit=$status duration=${duration}s"

    if eta=$(usage_limit_eta "$out"); then
        rm -f "$out"
        log "usage limit ($eta) - exiting so the VM naps; hourly reboot resumes once quota is back"
        exit 0
    fi
    rm -f "$out"

    if [ "$duration" -lt 120 ]; then
        # A fast exit is a no-op or a transient error. Ride out a couple
        # (short sleep, retry while the VM is up anyway) before concluding
        # the backlog is drained and exiting.
        consecutive_noop=$(( consecutive_noop + 1 ))
        log "short iteration ${consecutive_noop}/${DRAIN_LIMIT}"
        if [ "$consecutive_noop" -ge "$DRAIN_LIMIT" ]; then
            log "backlog appears drained - exiting; hourly reboot re-checks for new work"
            exit 0
        fi
        sleep 300
    else
        consecutive_noop=0
        sleep 600
    fi
done
