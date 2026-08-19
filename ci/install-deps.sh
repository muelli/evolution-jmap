#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The one authoritative list of Evolution/EDS build dependencies (3.52
# series). Used by CI's build job, and the reference list for the runner
# VM and the CI container image. Debian/Ubuntu (apt); sudo used only when
# not already root.

set -euo pipefail

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

$SUDO apt-get update -q
$SUDO apt-get install -y --no-install-recommends \
    cmake \
    ninja-build \
    pkg-config \
    libglib2.0-dev \
    libgtk-3-dev \
    libcamel1.2-dev \
    libedataserver1.2-dev \
    libebackend1.2-dev \
    libebook1.2-dev \
    libedata-book1.2-dev \
    libecal2.0-dev \
    libedata-cal2.0-dev \
    evolution-dev \
    lintian
