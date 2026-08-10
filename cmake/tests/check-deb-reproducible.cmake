# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Reproducibility check for the binary `.deb` (M8). Run with `cmake -P`; the
# caller sets:
#
#   BUILD_DIR  the CMake build tree to package out of
#   STAGE_DIR  a scratch directory the runs package into, one subdirectory each
#   EPOCH_A    a decoy SOURCE_DATE_EPOCH exported for the first run
#   EPOCH_B    a different decoy exported for the second run
#
# This repository builds everything twice and compares checksums, and the `.deb`
# is now one of the things a person is asked to verify against a signed digest.
# That promise is only worth anything if the package is a function of what went
# into it — nothing else.
#
# Two ways it can fail to be, and both are checked here because neither is
# visible by reading the package:
#
#  1. The clock. `cpack` re-runs the install into a fresh staging tree, so every
#     *directory* entry is created at packaging time, and the ar member headers
#     carry the moment the archive was written. Packaging the same tree twice a
#     minute apart therefore produced two different files.
#  2. The caller's environment. Whoever runs `cpack` may or may not have
#     SOURCE_DATE_EPOCH exported, and it need not agree with the one the
#     binaries inside the package were compiled with.
#
# So the package is built three times — twice with deliberately wrong epochs
# exported, once with the variable removed from the environment entirely — and
# all three must be byte-identical. The wrong epochs are what makes this more
# than a "run it twice" test: a package that merely honours whatever the caller
# exported would pass a two-run comparison on a machine that exports nothing,
# and then differ on the release runner, which exports the commit timestamp.
#
# Byte-equality across those three runs still would not prove the *file* entries
# are pinned: the modules keep their build mtimes, which are the same in all
# three runs of one build tree, so a package that inherited them would look
# stable here and change on the next rebuild. Hence the second assertion — every
# entry in the package carries one and the same timestamp. Nothing about a real
# tree makes that true by accident: the modules are linked seconds apart, the
# Camel `.urls` file is as old as the last time anyone edited it, and the
# directories are made at packaging time. Only a timestamp imposed on the whole
# archive collapses them to one value.

# Script mode takes its policies from here, not from the project. 3.24 for
# `cmake -E env --unset=`, which is the only way to ask for a run of cpack with
# SOURCE_DATE_EPOCH genuinely absent rather than empty.
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

# One packaging run into its own wiped subdirectory. `_epoch` is exported as
# SOURCE_DATE_EPOCH, or, when empty, removed from the environment.
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

# Every distinct entry timestamp in a package, control tar and data tar
# together, as `YYYY-MM-DD HH:MM:SS` strings. GNU tar's --full-time is what
# makes this a check rather than a coincidence: to the minute, files linked in
# the same build and directories made moments later would often agree.
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

# Reported together, because "which of the three differs" is the diagnosis: A
# against B says the exported epoch leaked into the package, either against the
# unset run says the packaging clock did.
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

# Pinned, not merely stable: one timestamp for the whole archive.
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
