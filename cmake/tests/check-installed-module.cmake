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
# and optionally:
#
#   EXPECTED_DATA '|'-separated absolute paths of files installed beside the
#                 module that must also have arrived
#
# and, to check the destination against something other than itself:
#
#   PKG_MODULE    pkg-config module owning the directory
#   PKG_VARIABLE  variable in it naming the directory
#
# The build system cannot check this itself: after `cmake --install`, is the
# module on disk, with the name and directory that EDS uses, and with the entry
# points that EDS resolves?

# Script mode takes its policies from here, not from the project. file(SIZE)
# and IN_LIST need a minimum version newer than the project minimum.
cmake_minimum_required(VERSION 3.14)

foreach(_var BUILD_DIR STAGE_DIR COMPONENT EXPECTED SYMBOLS)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

# Ask pkg-config where the host scans. Do not trust the configured
# destination: EXPECTED and the install rule come from the same variable, so
# they always agree, even if the variable is wrong.
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

# Use nm(1), not a scan of the file for the name. `.dynstr` also holds the
# *undefined* symbols. Thus a file(STRINGS) match proves only that the module
# mentions the entry point. Each module that includes the declaring header
# does that, with or without a definition. Mutation test: with the definition
# renamed to `camel_provider_module_lnit`, the string stayed and the scan
# passed. --defined-only is necessary.
find_program(NM_EXECUTABLE nm REQUIRED)
execute_process(
	COMMAND ${NM_EXECUTABLE} --dynamic --defined-only --format=posix "${_module}"
	OUTPUT_VARIABLE _nm_output
	ERROR_VARIABLE _nm_error
	RESULT_VARIABLE _result
)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "nm failed on ${_module} (${_result}):\n${_nm_error}")
endif()

# POSIX format is one `name type value size` line per symbol.
set(_exported)
string(REPLACE "\n" ";" _nm_lines "${_nm_output}")
foreach(_line IN LISTS _nm_lines)
	if(_line MATCHES "^([^ ]+) ")
		list(APPEND _exported "${CMAKE_MATCH_1}")
	endif()
endforeach()

string(REPLACE "|" ";" _wanted "${SYMBOLS}")
foreach(_symbol IN LISTS _wanted)
	if(NOT "${_symbol}" IN_LIST _exported)
		message(FATAL_ERROR "${_module} does not export ${_symbol}")
	endif()
endforeach()

message(STATUS "${_module}: ${_size} bytes, exports ${SYMBOLS}")

# Files installed beside the module, for example the Camel `.urls` file. It
# controls whether Camel dlopens the module. An empty file is as bad as a
# missing file, and the build system cannot see the difference. Thus also
# check the size.
if(DEFINED EXPECTED_DATA)
	string(REPLACE "|" ";" _data_files "${EXPECTED_DATA}")
	foreach(_data IN LISTS _data_files)
		if(NOT EXISTS "${STAGE_DIR}${_data}")
			message(FATAL_ERROR
				"component '${COMPONENT}' installed nothing at ${STAGE_DIR}${_data}")
		endif()
		file(SIZE "${STAGE_DIR}${_data}" _data_size)
		if(_data_size EQUAL 0)
			message(FATAL_ERROR "${STAGE_DIR}${_data} is empty")
		endif()
		message(STATUS "${STAGE_DIR}${_data}: ${_data_size} bytes")
	endforeach()
endif()

# The check above examines only the files that the caller declares. A build
# that does not declare its `.urls` passes it. For a Camel provider, that is
# the failure itself. Camel reads libcamel<protocol>.urls to decide whether to
# dlopen libcamel<protocol>.so. Without the file, the module is installed and
# correct, but Camel never loads it. Thus apply the Camel rule here.
get_filename_component(_module_name "${EXPECTED}" NAME)
if(_module_name MATCHES "^libcamel(.+)\\.so$")
	get_filename_component(_module_dir "${EXPECTED}" DIRECTORY)
	set(_urls "${STAGE_DIR}${_module_dir}/libcamel${CMAKE_MATCH_1}.urls")
	if(NOT EXISTS "${_urls}")
		message(FATAL_ERROR
			"${_module_name} is a Camel provider, but no ${_urls} was "
			"installed beside it; Camel would never dlopen it")
	endif()
endif()
