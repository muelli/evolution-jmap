#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script installs extra dependencies for the functional tests. See
# docs/functional-tests.md for details.
# It installs the EDS runtime daemons, not the dev headers.
# It installs a D-Bus session bus, so the daemons have somewhere to run.
# It installs a secret store, for EDS to reach during credential lookups.
# A session with no org.freedesktop.secrets provider fails any test whose
# account has an [Authentication] extension, even without a stored password.
# ci/install-deps.sh installs only the dev headers that every other target
# needs.
# These packages are additional, not a replacement, for that list.
# Run ci/install-deps.sh and this script together.

set -euo pipefail

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

$SUDO apt-get update -q
$SUDO apt-get install -y --no-install-recommends \
    evolution-data-server \
    dbus-daemon \
    gnome-keyring
