# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Staged-install check for a cdylib installed by add_cargo_cdylib().
# Run with `cmake -P`; the caller sets:
#
#   BUILD_DIR  the CMake build tree to install out of
#   STAGE_DIR  a scratch directory used as DESTDIR
#   COMPONENT  the install component to install
#   EXPECTED   absolute path of the module inside the install prefix
#   SYMBOLS    '|'-separated symbol names the module must export
#
# and optionally, to check the destination against something other than
# itself:
#
#   PKG_MODULE    pkg-config module owning the directory
#   PKG_VARIABLE  variable in it naming the directory
#
# It answers the question the build system cannot answer for itself: after
# `cmake --install`, is the module on disk, under the name and in the
# directory EDS will look for, with the entry points EDS resolves?

# Script mode takes its policies from here, not from the project: file(SIZE),
# list(JOIN) and IN_LIST all need a minimum newer than the project's own.
cmake_minimum_required(VERSION 3.14)

foreach(_var BUILD_DIR STAGE_DIR COMPONENT EXPECTED SYMBOLS)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

# Ask pkg-config where the host scans, rather than trusting the destination
# the build was configured with — otherwise EXPECTED and the install rule
# come from one variable and agree with each other no matter what it says.
if(DEFINED PKG_MODULE)
	find_program(PKG_CONFIG_EXECUTABLE pkg-config REQUIRED)
	execute_process(
		COMMAND ${PKG_CONFIG_EXECUTABLE} --variable=${PKG_VARIABLE} ${PKG_MODULE}
		OUTPUT_VARIABLE _scanned_dir
		OUTPUT_STRIP_TRAILING_WHITESPACE
		RESULT_VARIABLE _result
	)
	if(NOT _result EQUAL 0 OR _scanned_dir STREQUAL "")
		message(FATAL_ERROR "pkg-config --variable=${PKG_VARIABLE} ${PKG_MODULE} reported no directory")
	endif()
	get_filename_component(_target_dir "${EXPECTED}" DIRECTORY)
	if(NOT _target_dir STREQUAL _scanned_dir)
		message(FATAL_ERROR
			"module is installed into ${_target_dir}, "
			"but ${PKG_MODULE} scans ${_scanned_dir}")
	endif()
endif()

file(REMOVE_RECURSE "${STAGE_DIR}")

execute_process(
	COMMAND ${CMAKE_COMMAND} -E env "DESTDIR=${STAGE_DIR}"
		${CMAKE_COMMAND} --install "${BUILD_DIR}" --component "${COMPONENT}"
	RESULT_VARIABLE _result
	OUTPUT_VARIABLE _output
	ERROR_VARIABLE _error
)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "cmake --install --component ${COMPONENT} failed (${_result}):\n${_output}${_error}")
endif()

# DESTDIR prefixes the absolute destination, so this is where the file lands.
set(_module "${STAGE_DIR}${EXPECTED}")
if(NOT EXISTS "${_module}")
	message(FATAL_ERROR
		"component '${COMPONENT}' installed nothing at ${_module}\n"
		"install output was:\n${_output}${_error}")
endif()

file(SIZE "${_module}" _size)
if(_size LESS 1024)
	message(FATAL_ERROR "${_module} is ${_size} bytes; that is not a shared module")
endif()

# The dynamic symbol table is NUL-separated ASCII, which file(STRINGS)
# splits into one element per name — no nm(1) needed.
string(REPLACE "|" ";" _wanted "${SYMBOLS}")
list(JOIN _wanted "|" _pattern)
file(STRINGS "${_module}" _found REGEX "^(${_pattern})$")
foreach(_symbol IN LISTS _wanted)
	if(NOT "${_symbol}" IN_LIST _found)
		message(FATAL_ERROR "${_module} does not export ${_symbol}")
	endif()
endforeach()

message(STATUS "${_module}: ${_size} bytes, exports ${SYMBOLS}")
