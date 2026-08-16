# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# End-to-end check of add_translations(): configure, build and install the
# fixture project in cmake/tests/translations/ once per scenario, and hold the
# result against what each scenario is supposed to produce.
#
# Run with `cmake -P`; the caller sets:
#
#   PROJECT_DIR  the fixture project's source directory
#   MODULE       path to cmake/Translations.cmake, the code under test
#   PO_COMPILE   the po-compile binary built by the outer build
#   STAGE_DIR    scratch directory for the fixture builds and install trees
#   GENERATOR    the generator the outer build is using
#
# Four scenarios, because the interesting behaviour is not "it works": a
# language whose catalogue is missing, a catalogue the compiler refuses, and
# an empty LINGUAS are the three states this repository will actually be in,
# and only one of them may be silent.

cmake_minimum_required(VERSION 3.14)

foreach(_var PROJECT_DIR MODULE PO_COMPILE STAGE_DIR GENERATOR)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

if(NOT EXISTS "${PO_COMPILE}")
	message(FATAL_ERROR
		"${PO_COMPILE} does not exist; build the workspace before running "
		"this test (ninja, then ctest)")
endif()

file(REMOVE_RECURSE "${STAGE_DIR}")

# Configure the fixture project against one of the fixture `po/` directories.
# Answers with the exit status and the combined output, so a scenario that
# expects failure can say what it wanted to read in it.
function(configure_fixture _scenario _result_var _output_var)
	set(_build "${STAGE_DIR}/${_scenario}/build")
	set(_prefix "${STAGE_DIR}/${_scenario}/prefix")
	execute_process(
		COMMAND ${CMAKE_COMMAND}
			-S "${PROJECT_DIR}"
			-B "${_build}"
			-G "${GENERATOR}"
			"-DPO_DIR=${PROJECT_DIR}/po-${_scenario}"
			"-DPO_COMPILE=${PO_COMPILE}"
			"-DTRANSLATIONS_MODULE=${MODULE}"
			"-DCMAKE_INSTALL_PREFIX=${_prefix}"
		RESULT_VARIABLE _result
		OUTPUT_VARIABLE _stdout
		ERROR_VARIABLE _stderr
	)
	set(${_result_var} "${_result}" PARENT_SCOPE)
	set(${_output_var} "${_stdout}${_stderr}" PARENT_SCOPE)
endfunction()

function(build_fixture _scenario _result_var _output_var)
	execute_process(
		COMMAND ${CMAKE_COMMAND} --build "${STAGE_DIR}/${_scenario}/build"
		RESULT_VARIABLE _result
		OUTPUT_VARIABLE _stdout
		ERROR_VARIABLE _stderr
	)
	set(${_result_var} "${_result}" PARENT_SCOPE)
	set(${_output_var} "${_stdout}${_stderr}" PARENT_SCOPE)
endfunction()

function(install_fixture _scenario _result_var _output_var)
	execute_process(
		COMMAND ${CMAKE_COMMAND} --install "${STAGE_DIR}/${_scenario}/build"
			--component translations
		RESULT_VARIABLE _result
		OUTPUT_VARIABLE _stdout
		ERROR_VARIABLE _stderr
	)
	set(${_result_var} "${_result}" PARENT_SCOPE)
	set(${_output_var} "${_stdout}${_stderr}" PARENT_SCOPE)
endfunction()

# A compiled catalogue, checked as gettext would find it: the magic number
# first, then the translation itself. The magic is the format's own
# 0x950412de, stored little-endian on every architecture this is built on, so
# reading it back proves the file is a `.mo` and not, say, the `.po` copied
# across.
function(assert_catalogue _path _marker)
	if(NOT EXISTS "${_path}")
		message(FATAL_ERROR "no catalogue at ${_path}")
	endif()
	file(READ "${_path}" _magic LIMIT 4 HEX)
	if(NOT _magic STREQUAL "de120495")
		message(FATAL_ERROR
			"${_path} starts with ${_magic}, which is not a .mo file's magic "
			"number (de120495 as it is stored)")
	endif()
	file(STRINGS "${_path}" _found REGEX "${_marker}")
	if(NOT _found)
		message(FATAL_ERROR "${_path} does not contain ${_marker}")
	endif()
	file(SIZE "${_path}" _size)
	message(STATUS "${_path}: ${_size} bytes, holds ${_marker}")
endfunction()

#
# Scenario 1 — two languages, both installed where gettext looks.
#
configure_fixture(translated _result _output)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "configuring the translated fixture failed (${_result}):\n${_output}")
endif()
build_fixture(translated _result _output)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "building the translated fixture failed (${_result}):\n${_output}")
endif()
install_fixture(translated _result _output)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "installing the translated fixture failed (${_result}):\n${_output}")
endif()

set(_prefix "${STAGE_DIR}/translated/prefix")
assert_catalogue("${_prefix}/share/locale/xx/LC_MESSAGES/evolution-jmap.mo" "FIXTURE-XX-DESCRIPTION")
assert_catalogue("${_prefix}/share/locale/zz_ZZ/LC_MESSAGES/evolution-jmap.mo" "FIXTURE-ZZ-PROTOCOL")

# The two languages must be two catalogues: a build that compiled xx.po into
# both paths would pass the check above for xx and fail here.
file(STRINGS "${_prefix}/share/locale/zz_ZZ/LC_MESSAGES/evolution-jmap.mo" _leaked
	REGEX "FIXTURE-XX-")
if(_leaked)
	message(FATAL_ERROR "the zz_ZZ catalogue holds the xx translation: ${_leaked}")
endif()

# What the function reported back is what packaging declares, so it has to
# name exactly the two files that arrived.
file(STRINGS "${STAGE_DIR}/translated/build/installed-catalogues.txt" _reported)
set(_expected
	"${_prefix}/share/locale/xx/LC_MESSAGES/evolution-jmap.mo"
	"${_prefix}/share/locale/zz_ZZ/LC_MESSAGES/evolution-jmap.mo"
)
list(SORT _reported)
list(SORT _expected)
if(NOT _reported STREQUAL _expected)
	message(FATAL_ERROR
		"add_translations() reported\n  ${_reported}\nbut the files installed are\n  ${_expected}")
endif()

#
# Scenario 2 — LINGUAS names a language with no catalogue.
#
configure_fixture(missing _result _output)
if(_result EQUAL 0)
	message(FATAL_ERROR
		"configuring succeeded with a LINGUAS naming 'qq' and no qq.po; that "
		"build would ship one language fewer than it promises")
endif()
if(NOT _output MATCHES "qq")
	message(FATAL_ERROR "the failure does not name the language 'qq':\n${_output}")
endif()
message(STATUS "a language with no .po fails to configure, naming it")

#
# Scenario 3 — no languages at all, which is this repository today.
#
configure_fixture(empty _result _output)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "configuring the empty fixture failed (${_result}):\n${_output}")
endif()
build_fixture(empty _result _output)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "building the empty fixture failed (${_result}):\n${_output}")
endif()
install_fixture(empty _result _output)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "installing the empty fixture failed (${_result}):\n${_output}")
endif()
if(EXISTS "${STAGE_DIR}/empty/prefix/share/locale")
	message(FATAL_ERROR
		"an empty LINGUAS still created ${STAGE_DIR}/empty/prefix/share/locale")
endif()
file(READ "${STAGE_DIR}/empty/build/installed-catalogues.txt" _reported)
if(NOT _reported STREQUAL "")
	message(FATAL_ERROR "an empty LINGUAS reported catalogues: ${_reported}")
endif()
message(STATUS "an empty LINGUAS builds and installs nothing")

#
# Scenario 4 — a catalogue the compiler refuses.
#
configure_fixture(broken _result _output)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "configuring the broken fixture failed (${_result}):\n${_output}")
endif()
build_fixture(broken _result _output)
if(_result EQUAL 0)
	message(FATAL_ERROR
		"building succeeded on a .po po-compile refuses; the failure was "
		"swallowed and that language would silently be English:\n${_output}")
endif()
if(NOT _output MATCHES "msgctxt")
	message(FATAL_ERROR
		"the build failure does not carry the compiler's reason:\n${_output}")
endif()
if(EXISTS "${STAGE_DIR}/broken/prefix/share/locale/bb/LC_MESSAGES/evolution-jmap.mo")
	message(FATAL_ERROR "a refused .po left a catalogue behind")
endif()
message(STATUS "a .po the compiler refuses fails the build, with its reason")
