# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Track C2: docs/packaging/copyright's own-source stanzas are generated from
# REUSE.toml, and docs/packaging/third-party-notices from `cargo metadata`,
# both by tools/generate-debian-copyright.py, not hand-maintained. This keeps
# the committed files from drifting out of sync with the generator (or with
# REUSE.toml / Cargo.lock) the way hand-edited copies silently could. Run
# with `cmake -P`; the caller sets:
#
#   SOURCE_DIR         the repository root
#   PYTHON3_EXECUTABLE the python3 binary

cmake_minimum_required(VERSION 3.14)

foreach(_var SOURCE_DIR PYTHON3_EXECUTABLE)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

execute_process(
	COMMAND "${PYTHON3_EXECUTABLE}" "${SOURCE_DIR}/tools/generate-debian-copyright.py"
	OUTPUT_VARIABLE _generated
	ERROR_VARIABLE _error
	RESULT_VARIABLE _result
)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "generate-debian-copyright.py failed (${_result}):\n${_error}")
endif()

file(READ "${SOURCE_DIR}/docs/packaging/copyright" _committed)

if(NOT _generated STREQUAL _committed)
	message(FATAL_ERROR
		"docs/packaging/copyright is out of sync with REUSE.toml. Run:\n"
		"  python3 tools/generate-debian-copyright.py > docs/packaging/copyright\n"
		"and commit the result.")
endif()

message(STATUS "docs/packaging/copyright: in sync with REUSE.toml")

execute_process(
	COMMAND "${PYTHON3_EXECUTABLE}" "${SOURCE_DIR}/tools/generate-debian-copyright.py" --third-party-notices
	OUTPUT_VARIABLE _generated_tpn
	ERROR_VARIABLE _error_tpn
	RESULT_VARIABLE _result_tpn
)
if(NOT _result_tpn EQUAL 0)
	message(FATAL_ERROR "generate-debian-copyright.py --third-party-notices failed (${_result_tpn}):\n${_error_tpn}")
endif()

file(READ "${SOURCE_DIR}/docs/packaging/third-party-notices" _committed_tpn)

if(NOT _generated_tpn STREQUAL _committed_tpn)
	message(FATAL_ERROR
		"docs/packaging/third-party-notices is out of sync with `cargo metadata`. Run:\n"
		"  python3 tools/generate-debian-copyright.py --third-party-notices > docs/packaging/third-party-notices\n"
		"and commit the result.")
endif()

message(STATUS "docs/packaging/third-party-notices: in sync with cargo metadata")
