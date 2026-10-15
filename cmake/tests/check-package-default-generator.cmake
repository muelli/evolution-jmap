# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regression test for the `package` target itself. `.github/workflows/
# release.yml` builds the .deb with `cmake --build build --target package`,
# which runs cpack against CPACK_GENERATOR's full configured list. That is a
# different invocation from the package-deb* tests above, which all call
# `cpack -G DEB` directly and so never notice a generator added to that list
# whose toolchain is not installed (RPM without rpmbuild did exactly this on
# the release image, which has no rpmbuild: cpack aborted the whole run,
# taking the .deb down with it, while `-G DEB` runs stayed green). This
# exercises the exact command the release workflow runs instead.
#
#   BUILD_DIR   the CMake build tree to package out of

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
