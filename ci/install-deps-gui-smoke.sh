#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Extra dependencies for M9 Tier 2 (docs/gui-smoke-test.md): a full Evolution,
# a virtual X server to run it on, and AT-SPI to drive it. Additional to, not
# a replacement for, ci/install-deps.sh and ci/install-deps-functional.sh —
# run all three.

set -euo pipefail

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

$SUDO apt-get update -q
$SUDO apt-get install -y --no-install-recommends \
    evolution \
    xvfb \
    python3-pyatspi \
    imagemagick \
    ffmpeg
