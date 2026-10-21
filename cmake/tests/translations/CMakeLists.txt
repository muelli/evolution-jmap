# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The smallest project that uses add_translations().
# cmake/tests/check-translations.cmake configures and builds it once for each
# scenario, each time with a different `po/` directory.
#
# The real call cannot answer the questions of the check. This repository's
# po/LINGUAS can be empty, and then an assertion about installed files is
# true of a build that installs nothing. Fixture catalogues give a known set
# of languages. They also let the check cause the failure cases on purpose: a
# language with no `.po`, and a `.po` that the compiler refuses.
#
# Set by the caller:
#
#   PO_DIR               directory holding LINGUAS and the .po files
#   PO_COMPILE           the po-compile binary to compile them with
#   TRANSLATIONS_MODULE  path to cmake/AddTranslations.cmake

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

# Write the list that the function returns, for the check to read. The
# packaging rules use this list as the package contents, so it must agree
# with the files that the install really makes.
string(REPLACE ";" "\n" _installed "${INSTALLED_CATALOGUES}")
file(WRITE "${CMAKE_BINARY_DIR}/installed-catalogues.txt" "${_installed}")
