#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# disk-guard: keep the root disk from filling on a build/agent VM.
#
# Why this exists: a full root disk silently breaks the google-guest-agent — it
# can no longer write ~/.ssh/authorized_keys, which locks out SSH entirely (CLI
# AND browser console alike, since both rely on that same key application) and
# stalls the agent driver. It looks like an auth/OOM problem; it is neither.
# Nothing on these VMs redirects cargo target/, sccache, or Docker images off the
# root disk, so build junk accumulates until the disk crosses full (this bit the
# agy runner on 2026-08-19). This guard reclaims that junk before it can.
#
# Runs from cron as root. Everything it frees is re-derivable: re-pullable Docker
# images, re-downloadable crates, rebuildable target/. Reclaims in escalating
# order of disruption and stops as soon as usage is back under threshold.
set -u
THRESHOLD=${DISK_GUARD_PCT:-85}       # begin reclaiming at/above this % of /
HARD=${DISK_GUARD_HARD_PCT:-92}       # clear cargo target/ only above this %
pct() { df --output=pcent / | tail -1 | tr -dc '0-9'; }

used=$(pct); [ -z "$used" ] && exit 0
[ "$used" -lt "$THRESHOLD" ] && exit 0
logger -t disk-guard "root at ${used}% (>= ${THRESHOLD}%); reclaiming"

# 1. Docker images/containers/build cache — usually the biggest, always
#    re-pullable. prune -a keeps images backing a *running* container, so an
#    in-flight build is safe.
command -v docker >/dev/null 2>&1 && docker system prune -af >/dev/null 2>&1

# 2. Pure caches: sccache, and the cargo registry download/extract caches.
rm -rf /home/*/.cache/sccache 2>/dev/null
rm -rf /home/*/.cargo/registry/cache /home/*/.cargo/registry/src 2>/dev/null

used=$(pct)
logger -t disk-guard "after prune: root at ${used}%"

# 3. Last resort above the hard threshold: cargo build outputs. Forces a rebuild,
#    but a full disk that breaks SSH and the agent is strictly worse.
if [ "${used:-0}" -ge "$HARD" ]; then
    for t in /home/*/*/rust/target /home/*/rust/target; do
        [ -d "$t" ] && rm -rf "$t"
    done
    logger -t disk-guard "hard threshold: cleared cargo target/, now at $(pct)%"
fi
