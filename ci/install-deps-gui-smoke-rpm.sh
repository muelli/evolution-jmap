#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Fedora (dnf) twin of ci/install-deps-gui-smoke.sh: a full Evolution,
# Xvfb, D-Bus, AT-SPI, openbox, and the screenshot and recording tools
# ci/gui-smoke.sh keeps on failure. Additional to ci/install-deps-rpm.sh
# (the build dependencies), not a replacement. Used by
# ci/gui-smoke-fedora.sh inside the pinned Fedora image. sudo used only
# when not already root.

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
