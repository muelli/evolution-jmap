#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script is the Fedora (dnf) equivalent of ci/install-deps-gui-smoke.sh.
# It installs a full Evolution, Xvfb, D-Bus, AT-SPI, and openbox.
# It installs the screenshot and recording tools that ci/gui-smoke.sh keeps
# on failure.
# These packages are additional, not a replacement, for the build
# dependencies in ci/install-deps-rpm.sh.
# ci/gui-smoke-fedora.sh uses this script inside the pinned Fedora image.
# sudo runs only when the user is not already root.

set -euo pipefail

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

$SUDO dnf -y install \
	evolution \
	evolution-data-server \
	xorg-x11-server-Xvfb \
	dbus-daemon \
	dconf \
	at-spi2-core \
	python3-pyatspi \
	openbox \
	ImageMagick \
	ffmpeg-free
