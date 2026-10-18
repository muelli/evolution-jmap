#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Fedora (dnf) build dependencies for the RPM: the toolchain, the
# Evolution/EDS headers and rpmbuild plus rpmlint. Used by ci/rpm.sh's
# container and by GitLab's rpm job, which runs in the same Fedora image.
# No evolution-mapi-devel: this tree does not build against MAPI. sudo
# used only when not already root.

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
