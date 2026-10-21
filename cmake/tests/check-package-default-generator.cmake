# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regression test for the `package` target. `.github/workflows/release.yml`
# builds the .deb with `cmake --build build --target package`.
# That command runs cpack with all generators in CPACK_GENERATOR.
# The package-deb* tests in cmake/Packaging.cmake call `cpack -G DEB`.
# Thus they do not see a generator in that list whose tool is not installed.
# History: the release image has no rpmbuild, but RPM was in the list.
# cpack stopped the full run and made no .deb, but the `-G DEB` tests passed.
# Thus this test runs the same command as the release workflow.
#
#   BUILD_DIR   the CMake build tree to package

cmake_minimum_required(VERSION 3.14)

if(NOT DEFINED BUILD_DIR)
	message(FATAL_ERROR "-DBUILD_DIR= is required")
endif()

execute_process(
	COMMAND ${CMAKE_COMMAND} --build "${BUILD_DIR}" --target package
	RESULT_VARIABLE _result
	OUTPUT_VARIABLE _output
	ERROR_VARIABLE _error
)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR
		"cmake --build ${BUILD_DIR} --target package failed (${_result}), "
		"the exact command release.yml's package job runs:\n${_output}${_error}")
endif()
