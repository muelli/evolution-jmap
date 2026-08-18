#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Source this on the night-shift runner to point the `--features live-server`
# harness (and infra/stalwart/stw) at the real Stalwart test server:
#
#     source infra/live-server/live-server-env.sh
#     cargo test -p <crate> --features live-server        # run the harness
#     ./infra/stalwart/stw create account/user ...         # or seed via stw
#
# Reachability: the runner and stalwart-1 share the default VPC, and
# `default-allow-internal` permits runner -> stalwart-1:8080. So we address
# Stalwart by its INTERNAL DNS name — no external IP, no firewall/IP churn, no
# socat, nothing exposed to the internet. (The config-UI's "no plaintext to a
# non-localhost host" rule does NOT apply here: that lives in the account-setup
# page, not in jmap-client or the harness.)
#
# CREDENTIALS ($STALWART_CREDS, default ~/.config/evolution-jmap/stalwart-creds):
# the OPERATOR provisions this file ONCE. It holds a secret, so it is NOT in the
# repo. Contents (shell assignments):
#     STALWART_USER=admin@example.com     # or `admin` for the pinned recovery admin
#     STALWART_PASSWORD=...               # or, instead of USER/PASSWORD: STALWART_TOKEN=...
# The pinned recovery admin's secret:
#   gcloud compute ssh stalwart-1 --zone europe-west3-c -- sudo cat /opt/stalwart/admin-password

STALWART_VM=${STALWART_VM:-stalwart-1}
STALWART_ZONE=${STALWART_ZONE:-europe-west3-c}
STALWART_PROJECT=${STALWART_PROJECT:-evolution-jmap-ci-18696}
STALWART_CREDS=${STALWART_CREDS:-$HOME/.config/evolution-jmap/stalwart-creds}
_gcloud=$(command -v gcloud 2>/dev/null || echo /snap/bin/gcloud)

# Best-effort: wake Stalwart if it has napped. Needs the runner's service account
# to have compute perms; harmless if it does not (then keep Stalwart up instead,
# or the operator starts it). Reachability itself does not depend on this.
if [ -x "$_gcloud" ]; then
    _st=$("$_gcloud" compute instances describe "$STALWART_VM" --zone "$STALWART_ZONE" \
          --format='value(status)' 2>/dev/null)
    [ -n "$_st" ] && [ "$_st" != "RUNNING" ] && \
        "$_gcloud" compute instances start "$STALWART_VM" --zone "$STALWART_ZONE" >/dev/null 2>&1
fi

# Internal DNS name — resolves within the project VPC, stable across nap/start
# (unlike the external IP), and needs no gcloud call.
export STALWART_URL="http://${STALWART_VM}.${STALWART_ZONE}.c.${STALWART_PROJECT}.internal:8080"

if [ -f "$STALWART_CREDS" ]; then
    set -a; . "$STALWART_CREDS"; set +a
else
    echo "live-server-env: $STALWART_CREDS is missing — the operator must create it" \
         "(STALWART_USER + STALWART_PASSWORD, or STALWART_TOKEN); see this file's header." >&2
fi
echo "live-server-env: STALWART_URL=$STALWART_URL user=${STALWART_USER:-<none>}"
