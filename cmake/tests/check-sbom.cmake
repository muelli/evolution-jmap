# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Checks for the release SBOM's system-library half. Run with `cmake -P`; the
# caller sets:
#
#   SOURCE_DIR  the source tree (for ci/deb-dependencies.sh and ci/sbom.sh)
#   STAGE_DIR   a scratch directory for the throwaway packages and outputs
#   PART        "dependencies" or "sbom"
#
# The packages are built here with dpkg-deb from a control file alone, so the
# Depends are chosen by the test and not by whatever this build links against.
# They name packages every Debian-family host has installed (libc6, dpkg,
# coreutils), and versions are compared with what dpkg-query reports for them.
#
# PART=dependencies: ci/deb-dependencies.sh records, per Depends clause, the
# installed version of the alternative that satisfies it, and refuses a clause
# nothing installed satisfies.
#
# PART=sbom: ci/sbom.sh puts those versions into the pkg:deb components,
# refuses a resolution recorded for a different package, and two runs with the
# same SOURCE_DATE_EPOCH are byte-identical. Needs cargo-cyclonedx.

cmake_minimum_required(VERSION 3.14)

foreach(_var SOURCE_DIR STAGE_DIR PART)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

find_program(DPKG_DEB dpkg-deb REQUIRED)
find_program(DPKG_QUERY dpkg-query REQUIRED)
find_program(JQ jq REQUIRED)

file(REMOVE_RECURSE "${STAGE_DIR}")
file(MAKE_DIRECTORY "${STAGE_DIR}")

# Builds ${STAGE_DIR}/<name>.deb whose only content is a control file with the
# given Depends.
function(fake_deb name depends)
	set(_root "${STAGE_DIR}/${name}-root")
	file(MAKE_DIRECTORY "${_root}/DEBIAN")
	file(WRITE "${_root}/DEBIAN/control"
		"Package: sbom-test-${name}\n"
		"Version: 1.0\n"
		"Architecture: all\n"
		"Maintainer: test <test@example.invalid>\n"
		"Depends: ${depends}\n"
		"Description: SBOM test package\n")
	execute_process(
		COMMAND ${DPKG_DEB} --root-owner-group --build "${_root}" "${STAGE_DIR}/${name}.deb"
		RESULT_VARIABLE _result
		OUTPUT_QUIET
		ERROR_VARIABLE _error)
	if(NOT _result EQUAL 0)
		message(FATAL_ERROR "dpkg-deb --build ${name} failed: ${_error}")
	endif()
endfunction()

function(installed_version package out_var)
	execute_process(
		COMMAND ${DPKG_QUERY} -W -f "\${Version}" "${package}"
		RESULT_VARIABLE _result
		OUTPUT_VARIABLE _version
		ERROR_VARIABLE _error)
	if(NOT _result EQUAL 0 OR _version STREQUAL "")
		message(FATAL_ERROR "this host has no ${package} installed: ${_error}")
	endif()
	set(${out_var} "${_version}" PARENT_SCOPE)
endfunction()

# Runs ci/deb-dependencies.sh; sets <out_var> to its exit code.
function(resolve name out_var)
	execute_process(
		COMMAND bash "${SOURCE_DIR}/ci/deb-dependencies.sh"
			"${STAGE_DIR}/${name}.deb" "${STAGE_DIR}/${name}.json"
		RESULT_VARIABLE _result
		OUTPUT_VARIABLE _output
		ERROR_VARIABLE _error)
	message(STATUS "deb-dependencies.sh ${name}: ${_result} ${_output}${_error}")
	set(${out_var} "${_result}" PARENT_SCOPE)
endfunction()

# Sets <out_var> to the jq expression's raw output on the given file.
function(jq_raw file expression out_var)
	execute_process(
		COMMAND ${JQ} -r "${expression}" "${file}"
		RESULT_VARIABLE _result
		OUTPUT_VARIABLE _output
		ERROR_VARIABLE _error
		OUTPUT_STRIP_TRAILING_WHITESPACE)
	if(NOT _result EQUAL 0)
		message(FATAL_ERROR "jq '${expression}' ${file} failed: ${_error}")
	endif()
	set(${out_var} "${_output}" PARENT_SCOPE)
endfunction()

function(expect_equal what actual expected)
	if(NOT "${actual}" STREQUAL "${expected}")
		message(FATAL_ERROR "${what}: expected '${expected}', got '${actual}'")
	endif()
endfunction()

installed_version(libc6 _libc6)
installed_version(dpkg _dpkg)
installed_version(coreutils _coreutils)

# The first alternative of the second clause is not a package anywhere, so the
# clause must resolve to the second one. libc6 is bounded from both sides, as
# this project's Depends bound libevolution: two clauses, one package.
set(_good_depends "libc6 (>= 2.17), not-a-real-package-sbom-test | dpkg, coreutils, libc6 (<< 99999)")
fake_deb(good "${_good_depends}")
resolve(good _result)
expect_equal("deb-dependencies.sh exit code for satisfiable Depends" "${_result}" "0")
set(_good_json "${STAGE_DIR}/good.json")

if(PART STREQUAL "dependencies")
	jq_raw("${_good_json}" "[.packages[] | .name + \"=\" + .version] | join(\",\")" _packages)
	expect_equal("resolved packages" "${_packages}"
		"libc6=${_libc6},dpkg=${_dpkg},coreutils=${_coreutils},libc6=${_libc6}")

	execute_process(COMMAND dpkg --print-architecture
		OUTPUT_VARIABLE _native OUTPUT_STRIP_TRAILING_WHITESPACE)
	jq_raw("${_good_json}" ".packages[0].arch" _arch)
	expect_equal("libc6 architecture" "${_arch}" "${_native}")

	jq_raw("${_good_json}" ".depends" _depends)
	expect_equal("recorded Depends" "${_depends}" "${_good_depends}")

	jq_raw("${_good_json}" ".distro.id" _distro_id)
	if(_distro_id STREQUAL "")
		message(FATAL_ERROR "no distro id recorded")
	endif()

	fake_deb(too-new "libc6 (>= 99999)")
	resolve(too-new _result)
	if(_result EQUAL 0)
		message(FATAL_ERROR
			"deb-dependencies.sh accepted libc6 (>= 99999) with libc6 ${_libc6} installed")
	endif()

	fake_deb(missing "libc6, not-a-real-package-sbom-test")
	resolve(missing _result)
	if(_result EQUAL 0)
		message(FATAL_ERROR "deb-dependencies.sh accepted a Depends nothing installed satisfies")
	endif()

	message(STATUS "deb-dependencies.sh: resolved versions match dpkg-query")
	return()
endif()

if(NOT PART STREQUAL "sbom")
	message(FATAL_ERROR "unknown PART '${PART}'")
endif()

# Runs ci/sbom.sh on the given package and resolution; sets <out_var> to its
# exit code.
function(sbom deb resolved out out_var)
	execute_process(
		COMMAND ${CMAKE_COMMAND} -E env SOURCE_DATE_EPOCH=1600000000
			bash "${SOURCE_DIR}/ci/sbom.sh" "${deb}" "${resolved}" "${out}"
		WORKING_DIRECTORY "${SOURCE_DIR}"
		RESULT_VARIABLE _result
		OUTPUT_VARIABLE _output
		ERROR_VARIABLE _error)
	message(STATUS "sbom.sh ${out}: ${_result}\n${_output}${_error}")
	set(${out_var} "${_result}" PARENT_SCOPE)
endfunction()

set(_deb "${STAGE_DIR}/good.deb")
sbom("${_deb}" "${_good_json}" "${STAGE_DIR}/first.cdx.json" _result)
expect_equal("first sbom.sh exit code" "${_result}" "0")
sbom("${_deb}" "${_good_json}" "${STAGE_DIR}/second.cdx.json" _result)
expect_equal("second sbom.sh exit code" "${_result}" "0")

file(SHA256 "${STAGE_DIR}/first.cdx.json" _first)
file(SHA256 "${STAGE_DIR}/second.cdx.json" _second)
if(NOT _first STREQUAL _second)
	message(FATAL_ERROR
		"two sbom.sh runs with the same SOURCE_DATE_EPOCH differ: "
		"${STAGE_DIR}/first.cdx.json and ${STAGE_DIR}/second.cdx.json")
endif()

set(_sbom "${STAGE_DIR}/first.cdx.json")
jq_raw("${_sbom}"
	"[.components[] | select(.purl | startswith(\"pkg:deb/\")) | .name + \"=\" + .version] | join(\",\")"
	_deb_components)
# One component per package: CycloneDX requires every bom-ref to be unique.
expect_equal("pkg:deb components" "${_deb_components}"
	"libc6=${_libc6},dpkg=${_dpkg},coreutils=${_coreutils}")

jq_raw("${_sbom}" "[.components[] | select(.name == \"libc6\") | .purl][0]" _purl)
jq_raw("${_good_json}" "\"pkg:deb/\" + .distro.id + \"/libc6@\" + (.packages[0].version | @uri)" _purl_prefix)
string(FIND "${_purl}" "${_purl_prefix}?arch=" _at)
if(NOT _at EQUAL 0)
	message(FATAL_ERROR "libc6 purl '${_purl}' does not start with '${_purl_prefix}?arch='")
endif()

jq_raw("${_sbom}" "[.components[] | select(.type == \"library\" and (.purl | startswith(\"pkg:cargo/\")))] | length" _crates)
if(_crates LESS 1)
	message(FATAL_ERROR "the SBOM lists no Rust crates")
endif()

# A resolution recorded for another package must not be folded in silently.
fake_deb(other "libc6")
sbom("${STAGE_DIR}/other.deb" "${_good_json}" "${STAGE_DIR}/stale.cdx.json" _result)
if(_result EQUAL 0)
	message(FATAL_ERROR "sbom.sh accepted a dependency resolution made for a different package")
endif()

# The per-crate files cargo-cyclonedx writes into the source tree are scratch,
# on success and on failure alike.
file(GLOB_RECURSE _leftovers "${SOURCE_DIR}/rust/*.cdx.json")
if(_leftovers)
	message(FATAL_ERROR "sbom.sh left files in the source tree: ${_leftovers}")
endif()

message(STATUS "sbom.sh: reproducible, pkg:deb components carry resolved versions")
