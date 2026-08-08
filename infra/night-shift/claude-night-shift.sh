#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Night-shift driver: repeatedly hands Claude Code the next-roadmap-item
# prompt, pushing each green increment. Deployed to ~/ on the runner VM
# and started inside tmux BY THE OPERATOR (deliberately not automated).
#
# Usage-limit handling: when an iteration fails with a usage-limit error,
# the reset time is parsed out of the message — preferably the epoch
# timestamp Claude Code embeds ("...|1754630400"), falling back to the
# human "resets at 4pm" form — and the driver sleeps until then (plus a
# 2-minute buffer) instead of polling. Unparseable limit messages fall
# back to a 60-minute nap; sleeps are capped at 6h so a bad parse
# self-corrects. The script name contains "claude" on purpose: the idle
# watchdog counts it as activity, so backoff sleeps do not power the VM
# off mid-shift. The 24h uptime cap ends the shift.

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
cd "$HOME/evolution-jmap" || exit 1
LOG="$HOME/night-shift.log"
PROMPT_FILE="$HOME/night-prompt.md"

log() { echo "$(date -Is) $*" >> "$LOG"; }

# Print the seconds to sleep if $1 contains a usage-limit error; print
# nothing otherwise.
limit_backoff() {
    local out="$1" reset now tstr
    grep -qiE "usage limit|rate limit" "$out" || return 0
    now=$(date +%s)
    # Machine-readable epoch, e.g. "Claude AI usage limit reached|1754630400"
    reset=$(grep -oE '\|[0-9]{10}' "$out" | tr -d '|' | head -1)
    if [ -z "$reset" ]; then
        # Human form, e.g. "Your limit will reset at 4pm". NB: printed in
        # the account's timezone, the VM runs UTC — the 6h cap plus
        # re-probing absorbs a mismatch.
        tstr=$(grep -oiE "resets? at [0-9apm: ]{1,8}" "$out" | head -1 | sed -E 's/resets? at //I')
        [ -n "$tstr" ] && reset=$(date -d "$tstr" +%s 2>/dev/null)
        [ -n "$reset" ] && [ "$reset" -le "$now" ] && reset=$((reset + 86400))
    fi
    if [ -n "$reset" ] && [ "$reset" -gt "$now" ]; then
        echo $(( reset - now + 120 ))
    else
        echo 3600
    fi
}

log "=== night shift starting ==="
while true; do
    git pull --rebase --quiet >> "$LOG" 2>&1 || true
    start=$(date +%s)
    out=$(mktemp)
    claude --dangerously-skip-permissions -p "$(cat "$PROMPT_FILE")" > "$out" 2>&1
    status=$?
    cat "$out" >> "$LOG"
    duration=$(( $(date +%s) - start ))
    log "iteration finished: exit=$status duration=${duration}s"

    backoff=$(limit_backoff "$out")
    rm -f "$out"
    if [ -n "$backoff" ]; then
        [ "$backoff" -gt 21600 ] && backoff=21600   # re-probe at most every 6h
        log "usage limit - sleeping ${backoff}s, resuming ~$(date -Is -d "+${backoff} seconds")"
        sleep "$backoff"
    elif [ "$duration" -lt 120 ]; then
        log "fast exit without limit message - backing off 30 min"
        sleep 1800
    else
        sleep 600
    fi
done
