# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The release workflow, read as a document rather than run (M8). Run with
# `cmake -P`; the caller sets:
#
#   SOURCE_DIR    the checkout root, which is where the CI configs live
#
# A release workflow can only be *executed* by pushing a tag, so the first
# time anyone finds out it is wrong is the moment a release is already
# half-published. The four properties below do not need it to run, and each
# of them is one an ordinary edit can break silently:
#
# 1. Everything the release publishes is attested, and everything attested is
#    published. Sigstore provenance is worth having precisely because its
#    absence is invisible: an artifact added to the release and forgotten in
#    the attestation step downloads exactly like its signed neighbours and
#    `gh attestation verify` on it fails with "no attestation found", which
#    reads like a tooling problem rather than a gap. Keeping both sides the
#    same glob is what makes the sets equal by construction instead of by
#    someone remembering.
# 2. The package is built, and reaches the release. `apt install
#    ./evolution-jmap.deb` from a nightly is what M8 exists to deliver; a
#    release that quietly stops carrying it is a regression nothing else
#    notices.
# 3. Every CI config that pins the shared image pins the *same* image. The
#    .deb's dependencies are derived by dpkg-shlibdeps from the EDS in that
#    image, so a release built in a stale one would declare dependencies
#    against a distribution nobody is testing against.
# 4. The package's SBOM is attested against the package. The SBOM file is a
#    release artifact like any other, so (1) already gives it provenance; what
#    (1) cannot show is which package it describes. An `actions/attest-sbom`
#    step whose subject is the .deb and whose SBOM is the one collected in
#    dist/ is what lets `gh attestation verify --predicate-type
#    https://cyclonedx.org/bom` answer that.

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

# Line-oriented from here on. Semicolons would become list separators, and a
# workflow has none; brackets would, and it has plenty.
string(REPLACE "[" "\\[" _release_text_escaped "${_release_text}")
string(REPLACE "\n" ";" _lines "${_release_text_escaped}")

#
# 1. Attested set == published set.
#

# `subject-path:` takes either one value on the key's own line or a block
# scalar whose entries are the more-indented lines below it. Both spellings
# are in the wild, so both are read, and the entries are compared as written:
# two globs that happen to cover the same files today are exactly the drift
# this is meant to catch.
#
# Only the build provenance step's subjects count here. Other attest actions
# take a `subject-path:` too, and the SBOM one (4) names just the package, so
# reading every `subject-path:` in the file would mix a deliberate subset
# into the set that has to equal what is published. A step starts at a list
# entry whose first key is one a step can open with; the action it runs is
# its `uses:`.
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
	# The step test comes first: a second MATCHES in the same condition would
	# overwrite the CMAKE_MATCH_<n> the key's own match leaves.
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
			# Blank lines inside a block scalar are content, not its end.
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

# What `gh release create` uploads. Taken as the tokens naming something under
# dist/ on that command's logical line — the tag and `--generate-notes` are not
# files, and the release directory is the one thing every artifact passes
# through.
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
	# ci-image.yml is the workflow that *builds* the image, so a digest it
	# mentions is an output, not a pin.
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

# The release workflow must be one of the configs that pins it: the package's
# Depends come from dpkg-shlibdeps reading the modules against the libraries in
# that image.
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
# Single-line values only: the SBOM step names one package glob and one file,
# and a block scalar here would be a change worth a reviewer's look anyway.
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
	# Whole values: the SBOM's name carries `${{ github.ref_name }}`, spaces
	# and all.
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
