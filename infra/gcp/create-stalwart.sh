#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Create a small VM running the Stalwart mail server (full JMAP: mail,
# submission, contacts, calendars) as the real-server target for the
# integration test round.
#
# Usage:
#   ./create-stalwart.sh
#
# Security: the firewall only admits YOUR current public IP (JMAP on 8080,
# admin on 8080 too; SMTP stays closed — we feed mail in via JMAP, not SMTP,
# so the box can never be an open relay). Re-run
#   ./create-stalwart.sh --update-firewall
# when your IP changes.
#
# Cost: the VM naps itself after 60 minutes without logins or Stalwart
# traffic (any packet on 8080 is genuine use — the firewall admits only
# the operator). A stopped instance costs only its disk (~$2/month);
# wake it with `gcloud compute instances start <name>`. NB: the ephemeral
# external IP changes across restarts.
#
# The admin password is generated on the VM at first boot:
#   gcloud compute ssh stalwart-1 --zone europe-west3-c -- sudo cat /opt/stalwart/admin-password

set -euo pipefail

NAME=${NAME:-stalwart-1}
ZONE=${ZONE:-europe-west3-c}          # different region than the runner (trial vCPU quota)
MACHINE=${MACHINE:-e2-small}
MY_IP=$(curl -sf https://ifconfig.me)/32

if [[ "${1:-}" == "--update-firewall" ]]; then
    gcloud compute firewall-rules update allow-stalwart-jmap --source-ranges="$MY_IP"
    echo "Firewall now admits $MY_IP"
    exit 0
fi

STARTUP=$(mktemp)
trap 'rm -f "$STARTUP"' EXIT
cat > "$STARTUP" <<'EOF'
#!/bin/bash
set -eux
apt-get update -q && apt-get install -y --no-install-recommends docker.io cron

# Counting rule: how many packets have reached Stalwart's HTTP port. The
# GCP firewall only admits the operator's IP, so any count movement is
# genuine use. (Startup scripts run on every boot; guard against dupes.)
iptables -C INPUT -p tcp --dport 8080 -j ACCEPT 2>/dev/null \
    || iptables -I INPUT -p tcp --dport 8080 -j ACCEPT

# Idle watchdog: shut down after 60 minutes with no login session and no
# movement on the packet counter. Boot counts as activity (fresh /run).
cat > /usr/local/bin/idle-watchdog <<'WATCHDOG'
#!/bin/bash
STAMP=/run/stalwart-last-active
COUNTS=/run/stalwart-pkt-count
[ -f "$STAMP" ] || touch "$STAMP"
pkts=$(iptables -nvxL INPUT | awk '/tcp dpt:8080/ {print $1; exit}')
prev=$(cat "$COUNTS" 2>/dev/null || echo -1)
if [ -n "$(who)" ] || [ "$pkts" != "$prev" ]; then
    touch "$STAMP"
fi
echo "$pkts" > "$COUNTS"
if [ $(( $(date +%s) - $(stat -c %Y "$STAMP") )) -gt 3600 ]; then
    logger "idle-watchdog: no Stalwart traffic or logins for 60 minutes, shutting down"
    /sbin/shutdown -h now
fi
WATCHDOG
chmod +x /usr/local/bin/idle-watchdog
echo '*/5 * * * * root /usr/local/bin/idle-watchdog' > /etc/cron.d/idle-watchdog

mkdir -p /opt/stalwart
if [ ! -f /opt/stalwart/admin-password ]; then
    tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 24 > /opt/stalwart/admin-password
fi
docker rm -f stalwart || true
docker run -d --name stalwart --restart unless-stopped \
    -p 8080:8080 \
    -v /opt/stalwart/data:/opt/stalwart \
    -e ADMIN_SECRET="$(cat /opt/stalwart/admin-password)" \
    stalwartlabs/stalwart:latest
EOF

gcloud compute instances create "$NAME" \
    --zone "$ZONE" \
    --machine-type "$MACHINE" \
    --image-family=ubuntu-2404-lts-amd64 \
    --image-project=ubuntu-os-cloud \
    --boot-disk-size=20GB \
    --tags=stalwart \
    --metadata-from-file=startup-script="$STARTUP"

gcloud compute firewall-rules describe allow-stalwart-jmap >/dev/null 2>&1 \
    || gcloud compute firewall-rules create allow-stalwart-jmap \
        --allow=tcp:8080 --target-tags=stalwart --source-ranges="$MY_IP"

IP=$(gcloud compute instances describe "$NAME" --zone "$ZONE" \
    --format='get(networkInterfaces[0].accessConfigs[0].natIP)')
echo
echo "Stalwart VM: http://${IP}:8080  (admin UI + JMAP; reachable from ${MY_IP} only)"
echo "Admin password: gcloud compute ssh ${NAME} --zone ${ZONE} -- sudo cat /opt/stalwart/admin-password"
echo "Session endpoint for the client: http://${IP}:8080/.well-known/jmap"
