# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This project's own catalogues, and the check that the machinery compiling
# them works. The function itself lives in cmake/AddTranslations.cmake so the
# fixture project the check builds can include it without also registering
# this test.
#
# Requires cmake/Rust.cmake: po-compile is built by the workspace, so the
# compiler is a file the `rust-build` target produces.

include(${CMAKE_CURRENT_LIST_DIR}/AddTranslations.cmake)

set(PO_COMPILE_EXECUTABLE "${CARGO_TARGET_DIR}/release/po-compile")

add_translations(
	PO_DIR "${CMAKE_SOURCE_DIR}/po"
	DOMAIN "${PACKAGE_NAME}"
	DESTINATION "${LANGUAGE_SUPPORT_DIRECTORY}"
	COMPONENT translations
	COMPILER "${PO_COMPILE_EXECUTABLE}"
	COMPILER_TARGET rust-build
	TARGET translations
	OUTPUT_VARIABLE INSTALLED_CATALOGUES
)

# The check cannot be made of the call above: po/LINGUAS is empty, so every
# assertion about installed catalogues would be true of a build that installs
# nothing. It is made of the same function driven over fixture catalogues
# instead — including the two cases that must be loud, a language with no
# `.po` and a `.po` the compiler refuses.
add_test(
	NAME translations
	COMMAND ${CMAKE_COMMAND}
		"-DPROJECT_DIR=${CMAKE_SOURCE_DIR}/cmake/tests/translations"
		"-DMODULE=${CMAKE_SOURCE_DIR}/cmake/AddTranslations.cmake"
		"-DPO_COMPILE=${PO_COMPILE_EXECUTABLE}"
		"-DSTAGE_DIR=${CMAKE_BINARY_DIR}/translations-test"
		"-DGENERATOR=${CMAKE_GENERATOR}"
		-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-translations.cmake"
)
