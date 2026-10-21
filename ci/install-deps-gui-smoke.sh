#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script installs extra dependencies for the GUI smoke test. See
# docs/gui-smoke-test.md for details.
# It installs a full Evolution.
# It installs a virtual X server (Xvfb), to run Evolution on.
# It installs AT-SPI, to drive Evolution.
# It installs a minimal window manager (openbox).
# ci/gui-smoke-example-module-folder-popup.sh needs the window manager, for
# its own real right-click test.
# Without a window manager, Xvfb never gives a window an absolute screen
# position or X input focus.
# Without that position and focus, neither coordinate-based mouse synthesis
# nor synthetic keyboard events reach the window.
# These packages are additional, not a replacement, for ci/install-deps.sh
# and ci/install-deps-functional.sh.
# Run all three scripts.

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
