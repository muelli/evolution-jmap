#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Extra dependencies for M9 layer 1 (docs/functional-tests.md): the EDS
# *runtime* daemons and a D-Bus session bus to run them on. ci/install-deps.sh
# only installs the -dev headers every other target needs; these packages are
# additional, not a replacement, so run both.

set -euo pipefail

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

$SUDO apt-get update -q
$SUDO apt-get install -y --no-install-recommends \
    evolution-data-server \
    dbus-daemon
