#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script lists the Fedora (dnf) build dependencies for the RPM: the
# toolchain, the Evolution/EDS headers, rpmbuild, and rpmlint.
# ci/rpm.sh's container uses this script.
# GitLab's rpm job also uses this script, which runs in the same Fedora
# image.
# This script does not install evolution-mapi-devel, because this tree
# does not build against MAPI.
# sudo runs only when the user is not already root.

set -euo pipefail

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

$SUDO dnf -y install \
	cmake \
	clang-devel \
	ninja-build \
	gcc \
	pkgconf-pkg-config \
	rust \
	cargo \
	rpm-build \
	rpmlint \
	evolution-data-server-devel \
	evolution-devel \
	glib2-devel \
	gtk4-devel \
	json-glib-devel \
	krb5-devel \
	libadwaita-devel \
	libsoup3-devel \
	gettext \
	python3
