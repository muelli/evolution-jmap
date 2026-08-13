#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Create a spot-instance GCP VM for the Antigravity Agent.
#
# Usage:
#   ./create-agy-runner.sh
#

set -euo pipefail

NAME=${NAME:-gha-agy-runner-1}
ZONE=${ZONE:-europe-west1-b}
MACHINE=${MACHINE:-c2d-standard-8}
IDLE_MINUTES=${IDLE_MINUTES:-15}
IMAGE_FAMILY=${IMAGE_FAMILY:-ubuntu-2404-lts-amd64}

STARTUP=$(mktemp)
trap 'rm -f "$STARTUP"' EXIT
cat > "$STARTUP" <<EOF
#!/bin/bash
set -eux

# Idle watchdog: refresh a timestamp whenever the VM is genuinely in use
cat > /usr/local/bin/idle-watchdog <<'WATCHDOG'
#!/bin/bash
STAMP=/run/runner-last-active
[ -f "\$STAMP" ] || touch "\$STAMP"
if [ -n "\$(who)" ] || pgrep -f "agy " > /dev/null; then
    touch "\$STAMP"
fi
if [ "\$(awk '{printf "%d", \$1}' /proc/uptime)" -gt 86400 ]; then
    logger "idle-watchdog: 24h uptime cap reached, shutting down (self-heal will restart)"
    /sbin/shutdown -h now
fi
if [ \$(( \$(date +%s) - \$(stat -c %Y "\$STAMP") )) -gt \$(( ${IDLE_MINUTES} * 60 )) ]; then
    logger "idle-watchdog: idle ${IDLE_MINUTES} minutes (no session or login), shutting down"
    /sbin/shutdown -h now
fi
WATCHDOG
chmod +x /usr/local/bin/idle-watchdog
echo '*/5 * * * * root /usr/local/bin/idle-watchdog' > /etc/cron.d/idle-watchdog

apt-get update -q
apt-get install -y --no-install-recommends \
    build-essential cmake ninja-build pkg-config git curl ca-certificates \
    libglib2.0-dev libgtk-3-dev libcamel1.2-dev libedataserver1.2-dev \
    libebackend1.2-dev libebook1.2-dev libedata-book1.2-dev \
    libecal2.0-dev libedata-cal2.0-dev evolution-dev libclang-dev jq \
    cron docker.io npm

useradd -m -s /bin/bash runner || true
usermod -aG docker runner
sudo -u runner bash -c '
    set -eux
    cd ~
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # Note: Install agy globally (assuming npm is the distribution method)
    # sudo npm install -g @google/antigravity-cli || true
'
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
echo "Agy Agent VM created."
