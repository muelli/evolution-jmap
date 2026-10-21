# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The catalogues of this project, and the test of the function that compiles
# them. The function is in cmake/AddTranslations.cmake, so the test's fixture
# project can include it without a second registration of this test.
#
# Requires cmake/Rust.cmake: the workspace builds po-compile, so the compiler
# is a file that the `rust-build` target makes.

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

# The test does not use the call above, because po/LINGUAS can be empty. Then
# each assertion about installed catalogues is true of a build that installs
# nothing.
#
# The test runs the same function on fixture catalogues. These include the two
# cases that must fail: a language with no `.po`, and a `.po` that the
# compiler refuses.
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
