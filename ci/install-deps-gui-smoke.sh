#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Extra dependencies for M9 Tier 2 (docs/gui-smoke-test.md): a full Evolution,
# a virtual X server to run it on, AT-SPI to drive it, and a minimal window
# manager (ci/gui-smoke-example-module-folder-popup.sh's own real right-click:
# without one, Xvfb never assigns a toplevel an absolute screen position or
# X input focus, so neither coordinate-based mouse synthesis nor synthetic
# keyboard events reach the window). Additional to, not a replacement for,
# ci/install-deps.sh and ci/install-deps-functional.sh — run all three.

set -euo pipefail

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

$SUDO apt-get update -q
$SUDO apt-get install -y --no-install-recommends \
    evolution \
    xvfb \
    openbox \
    python3-pyatspi \
    imagemagick \
    ffmpeg
