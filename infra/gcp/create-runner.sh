#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Create a spot-instance GitHub Actions runner VM on GCP.
#
# Usage:
#   RUNNER_TOKEN=<token> ./create-runner.sh
#
# Get the token from: https://github.com/muelli/evolution-jmap/settings/actions/runners/new
# (short-lived, ~1 hour).
#
# Shutdown policy: an idle watchdog powers the VM off once there has been
# no CI job, login session, or actively-running agent session for
# IDLE_MINUTES (default 5; boot counts as activity, so a freshly started
# VM gets a grace period to begin work). Crucially the session signal is
# the running claude process, not the driver loop — a driver spinning over
# an empty backlog must NOT keep the VM alive. A 24h maximum-uptime guard
# caps runaway cost if the activity signals misbehave. Start the VM again
# with
# `gcloud compute instances start <name>`.
#
# Trial-quota note: this uses 8 vCPUs — the whole per-region allowance on a
# free-trial account. Keep other VMs in a different region.

set -euo pipefail

NAME=${NAME:-gha-runner-1}
ZONE=${ZONE:-europe-west1-b}
MACHINE=${MACHINE:-c2d-standard-8}
REPO=${REPO:-muelli/evolution-jmap}
IDLE_MINUTES=${IDLE_MINUTES:-5}
# 24.04 matches the hosted runners, the CI container, and the Evolution
# 3.52 target. Override (e.g. IMAGE_FAMILY=ubuntu-2604-lts-amd64) to get a
# newer-EDS build target once the backends exist.
IMAGE_FAMILY=${IMAGE_FAMILY:-ubuntu-2404-lts-amd64}
: "${RUNNER_TOKEN:?set RUNNER_TOKEN (repo settings → Actions → Runners → New self-hosted runner)}"

STARTUP=$(mktemp)
trap 'rm -f "$STARTUP"' EXIT
cat > "$STARTUP" <<EOF
#!/bin/bash
set -eux

# Idle watchdog: refresh a timestamp whenever the VM is genuinely in use
# (and at boot); power off once it is ${IDLE_MINUTES} minutes stale.
cat > /usr/local/bin/idle-watchdog <<'WATCHDOG'
#!/bin/bash
STAMP=/run/runner-last-active
[ -f "\$STAMP" ] || touch "\$STAMP"          # /run is tmpfs: boot = activity
# Activity = a CI job (Runner.Worker), a login session, or an ACTIVELY
# RUNNING agent session. The session signal matches the claude process's
# own argv ("dangerously-skip-permissions"), NOT a driver script named
# claude-*.sh — otherwise a driver merely looping over no-op iterations
# with an empty backlog would pin the VM up forever. Only real work, a
# login, or a CI job keeps it awake; the hourly instance schedule re-boots
# an idle VM to poll for new work.
if pgrep -f Runner.Worker > /dev/null \
   || [ -n "\$(who)" ] \
   || pgrep -f dangerously-skip-permissions > /dev/null; then
    touch "\$STAMP"
fi
# Hard 24h uptime cap: cost hygiene and a periodic clean slate. This does
# NOT end long-running work — an instance schedule restarts the VM within
# the hour and @reboot cron entries resume the agent shifts.
if [ "\$(awk '{printf "%d", \$1}' /proc/uptime)" -gt 86400 ]; then
    logger "idle-watchdog: 24h uptime cap reached, shutting down (self-heal will restart)"
    /sbin/shutdown -h now
fi
# IDLE_MINUTES must stay ABOVE the driver's longest between-iteration sleep
# (now 4 min) so a working shift is never cut off mid-loop, but short enough
# that once the driver exits or pauses (drained backlog, usage limit, or
# "blocked") the VM naps promptly — each idle hour then costs ~IDLE_MINUTES
# of uptime, not a full 60.
if [ \$(( \$(date +%s) - \$(stat -c %Y "\$STAMP") )) -gt \$(( ${IDLE_MINUTES} * 60 )) ]; then
    logger "idle-watchdog: idle ${IDLE_MINUTES} minutes (no session, login, or CI job), shutting down"
    /sbin/shutdown -h now
fi
WATCHDOG
chmod +x /usr/local/bin/idle-watchdog
echo '*/5 * * * * root /usr/local/bin/idle-watchdog' > /etc/cron.d/idle-watchdog

apt-get update -q
apt-get install -y --no-install-recommends \\
    build-essential cmake ninja-build pkg-config git curl ca-certificates \\
    libglib2.0-dev libgtk-3-dev libcamel1.2-dev libedataserver1.2-dev \\
    libebackend1.2-dev libebook1.2-dev libedata-book1.2-dev \\
    libecal2.0-dev libedata-cal2.0-dev evolution-dev libclang-dev jq \\
    cron docker.io

useradd -m -s /bin/bash runner || true
usermod -aG docker runner
sudo -u runner bash -c '
    set -eux
    cd ~
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    mkdir -p actions-runner && cd actions-runner
    VER=\$(curl -s https://api.github.com/repos/actions/runner/releases/latest | jq -r .tag_name | tr -d v)
    curl -sL "https://github.com/actions/runner/releases/download/v\${VER}/actions-runner-linux-x64-\${VER}.tar.gz" | tar xz
    ./config.sh --unattended --url "https://github.com/${REPO}" --token "${RUNNER_TOKEN}" \\
        --name "\$(hostname)" --labels self-hosted,gcp,linux,x64
'
(cd /home/runner/actions-runner && ./svc.sh install runner && ./svc.sh start)
EOF

gcloud compute instances create "$NAME" \
    --zone "$ZONE" \
    --machine-type "$MACHINE" \
    --provisioning-model=SPOT \
    --instance-termination-action=STOP \
    --image-family="$IMAGE_FAMILY" \
    --image-project=ubuntu-os-cloud \
    --boot-disk-size=60GB \
    --boot-disk-type=pd-balanced \
    --metadata-from-file=startup-script="$STARTUP"

echo
echo "Runner VM created. It appears under the repo's runners in ~3 minutes:"
echo "  https://github.com/${REPO}/settings/actions/runners"
echo "Point jobs at it with:  runs-on: [self-hosted, gcp]"
