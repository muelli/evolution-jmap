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
DRAIN_LIMIT=3          # consecutive no-op iterations that mean "backlog drained"
UNKNOWN_RESET_BACKOFF=21600   # 6h, if a limit is seen but its reset can't be parsed

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

# Refuse to invoke Claude while a recorded limit is still in the future.
# This is what a reboot during a multi-day weekly limit hits: a `date`
# comparison and an immediate exit, not a rejected API call.
if [ -f "$LIMIT_FILE" ]; then
    until=$(cat "$LIMIT_FILE" 2>/dev/null)
    if [[ "$until" =~ ^[0-9]+$ ]] && [ "$until" -gt "$(date +%s)" ]; then
        log "still limited until $(date -Is -d "@$until"); not invoking Claude, exiting to nap"
        exit 0
    fi
    rm -f "$LIMIT_FILE"   # reset has passed
fi

log "=== night shift starting ==="
consecutive_noop=0
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
    git pull --rebase --quiet >> "$LOG" 2>&1 || true

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
    claude --model "$model" --dangerously-skip-permissions -p "$(cat "$PROMPT_FILE")" > "$out" 2>&1
    status=$?
    cat "$out" >> "$LOG"
    duration=$(( $(date +%s) - start ))
    log "iteration finished: model=$model exit=$status duration=${duration}s"

    # Consume a one-shot escalation only after it was actually used, so an
    # escalation the *agent* just wrote (on a default-model triage pass) still
    # applies to the next iteration rather than being cleared unused.
    [ "$escalated" = 1 ] && rm -f "$ESCALATE_FILE"

    reset_epoch=$(limit_reset_epoch "$out")
    rm -f "$out"
    if [ -n "$reset_epoch" ] && [ "$reset_epoch" -gt "$(date +%s)" ]; then
        echo "$reset_epoch" > "$LIMIT_FILE"
        log "hit Claude limit; resets $(date -Is -d "@$reset_epoch"). Recorded; exiting to nap — no retry until then."
        exit 0
    fi

    if [ "$duration" -lt 120 ]; then
        # A fast exit with no limit message is a no-op or transient error.
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
