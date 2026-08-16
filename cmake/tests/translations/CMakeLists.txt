# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The smallest project that uses add_translations(), configured and built by
# cmake/tests/check-translations.cmake once per scenario with a different
# `po/` directory each time.
#
# It exists because the real call cannot answer the questions the check asks:
# this repository's own po/LINGUAS is empty, so an assertion about what the
# build tree installs would today be vacuously true of a build that installs
# nothing at all. A fixture catalogue makes the same machinery answer for a
# language that does exist, and lets the failing cases — a language with no
# `.po`, a `.po` the compiler refuses — be run on purpose rather than waited
# for.
#
# Set by the caller:
#
#   PO_DIR               directory holding LINGUAS and the .po files
#   PO_COMPILE           the po-compile binary to compile them with
#   TRANSLATIONS_MODULE  path to cmake/Translations.cmake

cmake_minimum_required(VERSION 3.14)

project(translations-fixture NONE)

foreach(_var PO_DIR PO_COMPILE TRANSLATIONS_MODULE)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

include("${TRANSLATIONS_MODULE}")

add_translations(
	PO_DIR "${PO_DIR}"
	DOMAIN evolution-jmap
	DESTINATION "${CMAKE_INSTALL_PREFIX}/share/locale"
	COMPONENT translations
	COMPILER "${PO_COMPILE}"
	TARGET translations
	OUTPUT_VARIABLE INSTALLED_CATALOGUES
)

# The list the function reports back, written out so the check can read it:
# it is what the packaging rules use to declare which files the package
# contains, so "the function says nothing is installed" and "nothing is
# installed" have to be the same statement.
string(REPLACE ";" "\n" _installed "${INSTALLED_CATALOGUES}")
file(WRITE "${CMAKE_BINARY_DIR}/installed-catalogues.txt" "${_installed}")
