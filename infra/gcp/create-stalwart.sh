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
apt-get update -q && apt-get install -y --no-install-recommends docker.io
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
