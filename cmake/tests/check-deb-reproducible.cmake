# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Reproducibility check for the binary `.deb`. Run with `cmake -P`. The
# caller sets:
#
#   BUILD_DIR  the CMake build tree to package
#   STAGE_DIR  a scratch directory with one subdirectory for each run
#   EPOCH_A    a decoy SOURCE_DATE_EPOCH for the first run
#   EPOCH_B    a different decoy SOURCE_DATE_EPOCH for the second run
#
# This repository builds everything twice and compares checksums. Users can
# verify the `.deb` against a signed digest. This is useful only if the
# package depends on its inputs and on nothing else.
#
# Two causes can make the package change. This test checks both, because
# they are not visible in the package:
#
#  1. The clock. `cpack` runs the install again into a new staging tree, so it
#     makes each *directory* entry at packaging time. The ar member headers
#     contain the time of archive creation. Thus, two packaging runs of the
#     same tree one minute apart made two different files.
#  2. The caller environment. The `cpack` caller can export SOURCE_DATE_EPOCH
#     or not. Its value can be different from the value used to compile
#     the binaries in the package.
#
# Thus the test builds the package three times: twice with incorrect epochs
# exported, and once with the variable removed from the environment.
# The three packages must be byte-identical. The incorrect epochs are
# necessary. A package that only uses the exported value passes a two-run test
# on a machine that exports nothing. It then changes on the release runner,
# which exports the commit timestamp.
#
# Byte-equality of the three runs does not prove that the *file* entries have
# a fixed timestamp. The modules keep their build mtimes, which are the same in
# all three runs of one build tree. A package that uses these mtimes passes
# here, but changes after the next rebuild. Thus the second assertion: all
# entries in the package have the same timestamp. A real tree never does this
# by accident. The linker makes the modules seconds apart. The Camel `.urls`
# file has the time of its last edit. Packaging makes the directories.
# Only a timestamp applied to the full archive gives one value.

# Script mode uses the policies from this line, not from the project.
# 3.24 is necessary for `cmake -E env --unset=`. Only this option runs cpack
# with SOURCE_DATE_EPOCH absent, not empty.
cmake_minimum_required(VERSION 3.24)

foreach(_var BUILD_DIR STAGE_DIR EPOCH_A EPOCH_B)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

find_program(CPACK_EXECUTABLE cpack REQUIRED)
find_program(DPKG_DEB_EXECUTABLE dpkg-deb REQUIRED)
find_program(TAR_EXECUTABLE tar REQUIRED)

if(NOT EXISTS "${BUILD_DIR}/CPackConfig.cmake")
	message(FATAL_ERROR
		"${BUILD_DIR}/CPackConfig.cmake does not exist; the build tree has no "
		"CPack configuration, so there is nothing to package")
endif()

# One packaging run into its own empty subdirectory. A non-empty `_epoch`
# becomes SOURCE_DATE_EPOCH. An empty `_epoch` removes it from the environment.
function(pack_once _label _epoch _out_deb)
	set(_dir "${STAGE_DIR}/${_label}")
	file(REMOVE_RECURSE "${_dir}")
	file(MAKE_DIRECTORY "${_dir}")

	if(_epoch STREQUAL "")
		set(_env_args --unset=SOURCE_DATE_EPOCH)
	else()
		set(_env_args "SOURCE_DATE_EPOCH=${_epoch}")
	endif()

	execute_process(
		COMMAND ${CMAKE_COMMAND} -E env ${_env_args}
			${CPACK_EXECUTABLE} -G DEB
			--config "${BUILD_DIR}/CPackConfig.cmake"
			-B "${_dir}"
		WORKING_DIRECTORY "${BUILD_DIR}"
		RESULT_VARIABLE _result
		OUTPUT_VARIABLE _output
		ERROR_VARIABLE _error
	)
	if(NOT _result EQUAL 0)
		message(FATAL_ERROR
			"cpack -G DEB failed for run '${_label}' (${_result}):\n${_output}${_error}")
	endif()

	file(GLOB _debs "${_dir}/*.deb")
	list(LENGTH _debs _deb_count)
	if(NOT _deb_count EQUAL 1)
		message(FATAL_ERROR
			"run '${_label}' produced ${_deb_count} .deb files, expected 1: ${_debs}")
	endif()
	list(GET _debs 0 _deb)
	set(${_out_deb} "${_deb}" PARENT_SCOPE)
endfunction()

# Returns each different entry timestamp in the control tar and the data tar,
# as `YYYY-MM-DD HH:MM:SS` strings. GNU tar --full-time is necessary. Without
# seconds, files linked in one build and directories made a moment later often
# show the same minute.
function(deb_timestamps _deb _out)
	set(_stamps)
	foreach(_member --ctrl-tarfile --fsys-tarfile)
		execute_process(
			COMMAND ${DPKG_DEB_EXECUTABLE} ${_member} "${_deb}"
			COMMAND ${TAR_EXECUTABLE} --utc --full-time -tvf -
			OUTPUT_VARIABLE _listing
			ERROR_VARIABLE _listing_error
			RESULTS_VARIABLE _results
		)
		foreach(_result IN LISTS _results)
			if(NOT _result EQUAL 0)
				message(FATAL_ERROR
					"dpkg-deb ${_member} | tar -tv failed on ${_deb}:\n${_listing_error}")
			endif()
		endforeach()

		string(REPLACE "\n" ";" _lines "${_listing}")
		foreach(_line IN LISTS _lines)
			if(_line MATCHES " ([0-9][0-9-]+ [0-9][0-9:]+) ")
				list(APPEND _stamps "${CMAKE_MATCH_1}")
			elseif(NOT _line STREQUAL "")
				message(FATAL_ERROR
					"cannot read a timestamp out of a tar listing line:\n  '${_line}'")
			endif()
		endforeach()
	endforeach()

	if(NOT _stamps)
		message(FATAL_ERROR "${_deb} listed no archive entries at all")
	endif()
	list(REMOVE_DUPLICATES _stamps)
	list(SORT _stamps)
	set(${_out} "${_stamps}" PARENT_SCOPE)
endfunction()

pack_once(epoch-a "${EPOCH_A}" _deb_a)
pack_once(epoch-b "${EPOCH_B}" _deb_b)
pack_once(epoch-unset "" _deb_unset)

file(SHA256 "${_deb_a}" _sha_a)
file(SHA256 "${_deb_b}" _sha_b)
file(SHA256 "${_deb_unset}" _sha_unset)

# Report the three results together, because the different one gives the
# cause. If A and B differ, the exported epoch went into the package.
# If A or B differs from the unset run, the packaging clock went into it.
if(NOT _sha_a STREQUAL _sha_b OR NOT _sha_a STREQUAL _sha_unset)
	deb_timestamps("${_deb_a}" _stamps_a)
	deb_timestamps("${_deb_b}" _stamps_b)
	deb_timestamps("${_deb_unset}" _stamps_unset)
	string(REPLACE ";" ", " _stamps_a_text "${_stamps_a}")
	string(REPLACE ";" ", " _stamps_b_text "${_stamps_b}")
	string(REPLACE ";" ", " _stamps_unset_text "${_stamps_unset}")
	message(FATAL_ERROR
		"packaging the same build tree three times produced different .deb files, "
		"so the package is not a function of what went into it:\n"
		"  SOURCE_DATE_EPOCH=${EPOCH_A}: ${_sha_a}\n"
		"    entry timestamps: ${_stamps_a_text}\n"
		"  SOURCE_DATE_EPOCH=${EPOCH_B}: ${_sha_b}\n"
		"    entry timestamps: ${_stamps_b_text}\n"
		"  SOURCE_DATE_EPOCH unset:      ${_sha_unset}\n"
		"    entry timestamps: ${_stamps_unset_text}")
endif()

# Fixed, not only stable: one timestamp for the full archive.
deb_timestamps("${_deb_unset}" _stamps)
list(LENGTH _stamps _stamp_count)
if(NOT _stamp_count EQUAL 1)
	string(REPLACE ";" "\n  " _stamps_text "${_stamps}")
	message(FATAL_ERROR
		"the package carries ${_stamp_count} different entry timestamps, so its "
		"contents are dated by the filesystem rather than by the build:\n"
		"  ${_stamps_text}\n"
		"The next rebuild would give the modules new mtimes and produce a "
		"different package from identical sources.")
endif()

get_filename_component(_deb_name "${_deb_unset}" NAME)
message(STATUS "${_deb_name}: ${_sha_unset}")
message(STATUS "  every entry dated ${_stamps}, under three different environments")
