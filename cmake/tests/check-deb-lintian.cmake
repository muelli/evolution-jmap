# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# lintian check for the binary `.deb` (Track C1). Run with `cmake -P`. The
# caller sets:
#
#   BUILD_DIR          the CMake build tree to package
#   STAGE_DIR          a scratch directory for cpack output
#   LINTIAN_EXECUTABLE the lintian binary
#
# cmake/tests/check-deb-package.cmake checks the package content and the
# control fields. This test adds the generic lintian checks (for example,
# unstripped binaries, missing doc/copyright files, bad directory permissions).
# Thus a packaging regression causes a failed CI check, not a problem that a
# person finds only during a manual release build.

cmake_minimum_required(VERSION 3.14)

foreach(_var BUILD_DIR STAGE_DIR LINTIAN_EXECUTABLE)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

find_program(CPACK_EXECUTABLE cpack REQUIRED)

if(NOT EXISTS "${BUILD_DIR}/CPackConfig.cmake")
	message(FATAL_ERROR
		"${BUILD_DIR}/CPackConfig.cmake does not exist; the build tree has no "
		"CPack configuration, so there is nothing to package")
endif()

file(REMOVE_RECURSE "${STAGE_DIR}")
file(MAKE_DIRECTORY "${STAGE_DIR}")

execute_process(
	COMMAND ${CPACK_EXECUTABLE} -G DEB
		--config "${BUILD_DIR}/CPackConfig.cmake"
		-B "${STAGE_DIR}"
	WORKING_DIRECTORY "${BUILD_DIR}"
	RESULT_VARIABLE _result
	OUTPUT_VARIABLE _output
	ERROR_VARIABLE _error
)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "cpack -G DEB failed (${_result}):\n${_output}${_error}")
endif()

file(GLOB _debs "${STAGE_DIR}/*.deb")
list(LENGTH _debs _deb_count)
if(NOT _deb_count EQUAL 1)
	message(FATAL_ERROR
		"expected exactly one .deb in ${STAGE_DIR}, found ${_deb_count}: ${_debs}")
endif()
list(GET _debs 0 _deb)

# --pedantic shows more tags than the default profile, including the
# non-standard-dir-perm tag that this project had.
# Put a tag that you cannot fix in an override file next to this file.
# Give the reason in a comment. Do not go back to the default profile.
execute_process(
	COMMAND ${LINTIAN_EXECUTABLE} --pedantic "${_deb}"
	OUTPUT_VARIABLE _lintian_output
	ERROR_VARIABLE _lintian_error
	RESULT_VARIABLE _lintian_result
)
if(NOT _lintian_result EQUAL 0)
	message(FATAL_ERROR
		"lintian found something to say about ${_deb}:\n"
		"${_lintian_output}${_lintian_error}")
endif()

get_filename_component(_deb_name "${_deb}" NAME)
message(STATUS "${_deb_name}: lintian --pedantic clean")
