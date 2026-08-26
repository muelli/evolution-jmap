#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Extra dependencies for M9 layer 1 (docs/functional-tests.md): the EDS
# *runtime* daemons, a D-Bus session bus to run them on, and a secret store
# for EDS's credential lookups to reach (docs/ROADMAP.md item 18 — a session
# with no `org.freedesktop.secrets` provider fails any test whose account has
# an `[Authentication]` extension, not just ones that store a real password).
# ci/install-deps.sh only installs the -dev headers every other target needs;
# these packages are additional, not a replacement, so run both.

set -euo pipefail

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

$SUDO apt-get update -q
$SUDO apt-get install -y --no-install-recommends \
    evolution-data-server \
    dbus-daemon \
    gnome-keyring
