# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Reads the release workflow as a document, without running it. Run with
# `cmake -P`; the caller sets:
#
#   SOURCE_DIR    the checkout root, which is where the CI configs live
#
# Only a pushed tag runs a release workflow. Thus an error shows only when a
# release is already half-published. The four properties below do not need a
# run, and an ordinary edit can silently break each one:
#
# 1. The release attests all that it publishes, and publishes all that it
#    attests. A missing Sigstore provenance is invisible. An artifact that is
#    in the release but not in the attestation step downloads like the signed
#    artifacts. `gh attestation verify` on it fails with "no attestation
#    found", which looks like a tool problem, not a gap. The same glob on
#    both sides makes the two sets equal by construction.
# 2. The workflow builds the package and releases it. The purpose of the
#    release workflow is `apt install ./evolution-jmap.deb` from a nightly.
#    If a release silently stops carrying the package, no other check finds
#    the regression.
# 3. Each CI config that pins the shared image pins the *same* image.
#    dpkg-shlibdeps derives the .deb dependencies from the EDS in that image.
#    A release built in an old image declares dependencies on a distribution
#    that no test uses.
# 4. The package SBOM is attested against the package. The SBOM file is a
#    release artifact, so (1) gives it provenance. But (1) cannot show which
#    package the SBOM describes. An `actions/attest-sbom` step with the .deb
#    as subject and the SBOM from dist/ lets `gh attestation verify
#    --predicate-type https://cyclonedx.org/bom` show that.

cmake_minimum_required(VERSION 3.14)

if(NOT DEFINED SOURCE_DIR)
	message(FATAL_ERROR "-DSOURCE_DIR= is required")
endif()

set(RELEASE_WORKFLOW ".github/workflows/release.yml")
set(CI_IMAGE_REPO "ghcr.io/muelli/evolution-jmap/ci-image")

if(NOT EXISTS "${SOURCE_DIR}/${RELEASE_WORKFLOW}")
	message(FATAL_ERROR "${RELEASE_WORKFLOW} does not exist; nothing publishes a release")
endif()
file(READ "${SOURCE_DIR}/${RELEASE_WORKFLOW}" _release_text)

# From here, read line by line. Semicolons are list separators, but a
# workflow has none. Brackets also change list splitting, and a workflow has
# many, so escape them.
string(REPLACE "[" "\\[" _release_text_escaped "${_release_text}")
string(REPLACE "\n" ";" _lines "${_release_text_escaped}")

#
# 1. Attested set == published set.
#

# `subject-path:` takes one value on the same line as the key, or a block
# scalar with more-indented lines below it. Workflows use both forms, so read
# both. Compare the entries as written: two different globs that match the
# same files today are the drift that this check must find.
#
# Only the subjects of the build provenance step count here. Other attest
# actions also take `subject-path:`, and the SBOM step (4) names only the
# package. That subset must not enter the set that must equal the published
# set. A step starts at a list entry whose first key can open a step. Its
# `uses:` gives its action.
set(_attested)
set(_in_subject_block FALSE)
set(_subject_indent "")
set(_found_subject_path FALSE)
set(_step_action "")
foreach(_line IN LISTS _lines)
	if(_line MATCHES "^[ \t]*-[ \t]+(name|uses|run|id|if|with|env):")
		set(_step_action "")
	endif()
	if(_line MATCHES "^[ \t]*-?[ \t]*uses:[ \t]*([^ \t#]+)")
		set(_step_action "${CMAKE_MATCH_1}")
	endif()
	# Test the step first: a second MATCHES in the same condition overwrites
	# the CMAKE_MATCH_<n> of the key match.
	if(_step_action MATCHES "^actions/attest-build-provenance@"
			AND _line MATCHES "^([ \t]*)subject-path:[ \t]*(.*)$")
		set(_found_subject_path TRUE)
		set(_subject_indent "${CMAKE_MATCH_1}")
		set(_value "${CMAKE_MATCH_2}")
		string(STRIP "${_value}" _value)
		if(_value STREQUAL "|" OR _value STREQUAL ">" OR _value STREQUAL "")
			set(_in_subject_block TRUE)
		elseif(_value MATCHES "dist/")
			list(APPEND _attested "${_value}")
		endif()
	elseif(_in_subject_block)
		if(_line MATCHES "^[ \t]*$")
			# A blank line in a block scalar is content, not its end.
		elseif(_line MATCHES "^${_subject_indent}[ \t]+(.*)$")
			set(_entry "${CMAKE_MATCH_1}")
			string(REGEX REPLACE "^-[ \t]+" "" _entry "${_entry}")
			string(STRIP "${_entry}" _entry)
			list(APPEND _attested "${_entry}")
		else()
			set(_in_subject_block FALSE)
		endif()
	endif()
endforeach()

if(NOT _found_subject_path)
	message(FATAL_ERROR
		"${RELEASE_WORKFLOW} has no actions/attest-build-provenance step with a "
		"subject-path:, so it attests nothing; every published artifact is meant "
		"to carry Sigstore provenance (see docs/verifying-artifacts.md)")
endif()

# What `gh release create` uploads: the tokens with dist/ on the logical line
# of that command. The tag and `--generate-notes` are not files, and each
# artifact goes through the release directory.
string(REGEX REPLACE "\\\\\n[ \t]*" " " _joined "${_release_text}")
string(REPLACE "[" "\\[" _joined "${_joined}")
string(REPLACE "\n" ";" _joined_lines "${_joined}")
set(_published)
set(_found_create FALSE)
foreach(_line IN LISTS _joined_lines)
	if(_line MATCHES "gh release create")
		set(_found_create TRUE)
		string(REPLACE "\t" " " _line "${_line}")
		string(REPLACE " " ";" _tokens "${_line}")
		foreach(_token IN LISTS _tokens)
			if(_token MATCHES "dist/")
				list(APPEND _published "${_token}")
			endif()
		endforeach()
	endif()
endforeach()

if(NOT _found_create)
	message(FATAL_ERROR
		"${RELEASE_WORKFLOW} never runs `gh release create`, so no artifact "
		"reaches anyone")
endif()
if(NOT _published)
	message(FATAL_ERROR
		"`gh release create` in ${RELEASE_WORKFLOW} uploads nothing from dist/, "
		"which is where every artifact is collected and checksummed")
endif()

list(REMOVE_DUPLICATES _attested)
list(REMOVE_DUPLICATES _published)
list(SORT _attested)
list(SORT _published)

if(NOT _attested STREQUAL _published)
	string(REPLACE ";" "\n  " _attested_text "${_attested}")
	string(REPLACE ";" "\n  " _published_text "${_published}")
	message(FATAL_ERROR
		"${RELEASE_WORKFLOW} attests one set of paths and publishes another:\n"
		"  attested:\n  ${_attested_text}\n"
		"  published:\n  ${_published_text}\n"
		"An artifact in the second list and not the first ships without "
		"provenance; one in the first and not the second is attested and then "
		"never released. Write the same glob on both sides — `dist/*` for both "
		"— so adding an artifact cannot separate them.")
endif()

#
# 2. The package is built, and it is released.
#
set(_builds_package FALSE)
set(_publishes_package FALSE)
foreach(_line IN LISTS _lines)
	if(_line MATCHES "--target[ \t]+package" OR _line MATCHES "(^|[ \t])cpack([ \t]|$)")
		set(_builds_package TRUE)
	endif()
	if(_line MATCHES "\\.deb" AND _line MATCHES "dist")
		set(_publishes_package TRUE)
	endif()
endforeach()

if(NOT _builds_package)
	message(FATAL_ERROR
		"${RELEASE_WORKFLOW} never builds the binary package (no `cpack`, no "
		"`--target package`); a release that carries no .deb is one nobody can "
		"`apt install ./evolution-jmap.deb`")
endif()
if(NOT _publishes_package)
	message(FATAL_ERROR
		"${RELEASE_WORKFLOW} builds a package and never copies a .deb into "
		"dist/, so it is built on the runner and thrown away with it")
endif()

#
# 3. One pinned image, everywhere.
#
file(GLOB _ci_configs
	"${SOURCE_DIR}/.github/workflows/*.yml"
	"${SOURCE_DIR}/.gitlab-ci.yml"
)
set(_digests)
set(_referring_files)
foreach(_config IN LISTS _ci_configs)
	file(READ "${_config}" _text)
	file(RELATIVE_PATH _relative "${SOURCE_DIR}" "${_config}")
	# ci-image.yml *builds* the image, so a digest in it is an output, not a
	# pin.
	if(_relative MATCHES "ci-image\\.yml$")
		continue()
	endif()
	string(REGEX MATCHALL "${CI_IMAGE_REPO}@sha256:[0-9a-f]+" _matches "${_text}")
	foreach(_match IN LISTS _matches)
		string(REGEX REPLACE "^.*@" "" _digest "${_match}")
		list(APPEND _digests "${_digest}")
		list(APPEND _referring_files "${_relative}:${_digest}")
	endforeach()
endforeach()

if(NOT _digests)
	message(FATAL_ERROR
		"no CI config pins ${CI_IMAGE_REPO} by digest, so the build environment "
		"is whatever the tag pointed at that day")
endif()

list(REMOVE_DUPLICATES _digests)
list(LENGTH _digests _digest_count)
if(NOT _digest_count EQUAL 1)
	string(REPLACE ";" "\n  " _referring_text "${_referring_files}")
	message(FATAL_ERROR
		"the shared CI image is pinned to ${_digest_count} different digests:\n"
		"  ${_referring_text}\n"
		"Every config must pin the same one, or the .deb a release publishes is "
		"linked against, and declares dependencies from, a different "
		"distribution than CI tests.")
endif()

# The release workflow must pin the image too: dpkg-shlibdeps makes the
# package Depends from the modules and the libraries in that image.
set(_release_pins FALSE)
foreach(_entry IN LISTS _referring_files)
	if(_entry MATCHES "^${RELEASE_WORKFLOW}:")
		set(_release_pins TRUE)
	endif()
endforeach()
if(NOT _release_pins)
	message(FATAL_ERROR
		"${RELEASE_WORKFLOW} does not pin ${CI_IMAGE_REPO} by digest, so the "
		"package it publishes is not built in the environment CI tests in")
endif()

#
# 4. The SBOM is attested against the package.
#
# Single-line values only: the SBOM step names one package glob and one file.
# A block scalar here is a change that a reviewer must examine.
set(_step_action "")
set(_sbom_subjects)
set(_sbom_paths)
foreach(_line IN LISTS _lines)
	if(_line MATCHES "^[ \t]*-[ \t]+(name|uses|run|id|if|with|env):")
		set(_step_action "")
	endif()
	if(_line MATCHES "^[ \t]*-?[ \t]*uses:[ \t]*([^ \t#]+)")
		set(_step_action "${CMAKE_MATCH_1}")
	endif()
	if(NOT _step_action MATCHES "^actions/attest-sbom@")
		continue()
	endif()
	# Read the full value: the SBOM name contains `${{ github.ref_name }}`,
	# with its spaces.
	if(_line MATCHES "^[ \t]*(subject-path|sbom-path):[ \t]*(.*)$")
		set(_key "${CMAKE_MATCH_1}")
		string(STRIP "${CMAKE_MATCH_2}" _value)
		if(_key STREQUAL "subject-path")
			list(APPEND _sbom_subjects "${_value}")
		else()
			list(APPEND _sbom_paths "${_value}")
		endif()
	endif()
endforeach()

set(_sbom_covers_package FALSE)
foreach(_subject IN LISTS _sbom_subjects)
	if(_subject MATCHES "^dist/.*\\.deb$")
		set(_sbom_covers_package TRUE)
	endif()
endforeach()
set(_sbom_is_released FALSE)
foreach(_path IN LISTS _sbom_paths)
	if(_path MATCHES "^dist/.*\\.cdx\\.json$")
		set(_sbom_is_released TRUE)
	endif()
endforeach()

if(NOT _sbom_covers_package)
	message(FATAL_ERROR
		"${RELEASE_WORKFLOW} has no actions/attest-sbom step whose subject-path: "
		"is the .deb in dist/, so nothing ties the published SBOM to the package "
		"it describes")
endif()
if(NOT _sbom_is_released)
	message(FATAL_ERROR
		"the actions/attest-sbom step in ${RELEASE_WORKFLOW} does not attest a "
		"CycloneDX SBOM from dist/ (sbom-path: dist/....cdx.json), so the SBOM it "
		"signs is not the one the release publishes")
endif()

list(GET _digests 0 _digest)
list(LENGTH _attested _attested_count)
message(STATUS "release workflow: attests and publishes ${_attested_count} path pattern(s): ${_attested}")
message(STATUS "  shared CI image pinned at @${_digest} by every config that names it")
message(STATUS "  SBOM ${_sbom_paths} attested against ${_sbom_subjects}")
