# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Packaging check for the binary `.deb` (M8). Run with `cmake -P`; the caller
# sets:
#
#   BUILD_DIR        the CMake build tree to package out of
#   STAGE_DIR        a scratch directory cpack writes into
#   PACKAGE_NAME     the Debian package name the .deb must declare
#   PACKAGE_VERSION  the version it must declare
#   PACKAGE_SUMMARY  the one-line synopsis its Description must open with
#   EXPECTED         '|'-separated absolute paths of every regular file the
#                    package must contain, and — since the comparison is an
#                    equality, not a subset — the only ones it may contain
#
# The per-component `install-*` tests already answer "does this module reach
# the right directory". This answers the question one layer up, which they
# cannot: does the *package* a user installs carry those same files, and
# nothing else?
#
# "Nothing else" is the half worth having. This build tree also installs the
# upstream C example module into Evolution's module directory, with no install
# component of its own — so a package built the obvious way (monolithic, "just
# install everything") ships a demonstration module into the module directory
# of every machine that installs JMAP support. That is a silent, plausible,
# wrong package: it installs, it works, and it carries something nobody asked
# for. An equality over the file list is what makes it a failing test rather
# than a discovery made by whoever reads a directory listing later.

# Script mode takes its policies from here, not from the project: file(SIZE),
# list(JOIN), IN_LIST and REMOVE_ITEM-on-empty all need a newer minimum than
# the project's own.
cmake_minimum_required(VERSION 3.14)

foreach(_var BUILD_DIR STAGE_DIR PACKAGE_NAME PACKAGE_VERSION PACKAGE_SUMMARY EXPECTED)
	if(NOT DEFINED ${_var})
		message(FATAL_ERROR "-D${_var}= is required")
	endif()
endforeach()

find_program(CPACK_EXECUTABLE cpack REQUIRED)
find_program(DPKG_DEB_EXECUTABLE dpkg-deb REQUIRED)
find_program(DPKG_EXECUTABLE dpkg REQUIRED)

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

# Exactly one, so that a stale .deb from an earlier run cannot be the one
# inspected — which is also why STAGE_DIR is wiped above.
file(GLOB _debs "${STAGE_DIR}/*.deb")
list(LENGTH _debs _deb_count)
if(NOT _deb_count EQUAL 1)
	message(FATAL_ERROR
		"expected exactly one .deb in ${STAGE_DIR}, found ${_deb_count}: ${_debs}\n"
		"cpack said:\n${_output}${_error}")
endif()
list(GET _debs 0 _deb)

#
# Contents.
#
execute_process(
	COMMAND ${DPKG_DEB_EXECUTABLE} --contents "${_deb}"
	OUTPUT_VARIABLE _contents
	ERROR_VARIABLE _contents_error
	RESULT_VARIABLE _result
)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "dpkg-deb --contents failed on ${_deb} (${_result}):\n${_contents_error}")
endif()

# `dpkg-deb --contents` prints one `ls -l`-ish line per member, paths relative
# to the package root and so `./`-prefixed. Only regular files are compared:
# the directories leading to them are an artefact of how the archive was
# built, not a decision anyone made, and a package that creates
# /usr/lib/evolution-data-server on the way to a backend is not a finding.
set(_actual)
string(REPLACE "\n" ";" _lines "${_contents}")
foreach(_line IN LISTS _lines)
	if(_line MATCHES "^-.* \\.(/[^ ].*)$")
		list(APPEND _actual "${CMAKE_MATCH_1}")
	endif()
endforeach()

string(REPLACE "|" ";" _expected "${EXPECTED}")
list(SORT _expected)
list(SORT _actual)

set(_missing ${_expected})
if(_actual)
	list(REMOVE_ITEM _missing ${_actual})
endif()
set(_unexpected ${_actual})
if(_expected)
	list(REMOVE_ITEM _unexpected ${_expected})
endif()

if(_missing)
	string(REPLACE ";" "\n  " _missing_text "${_missing}")
	message(FATAL_ERROR
		"${_deb} is missing files the install rules install:\n  ${_missing_text}")
endif()
if(_unexpected)
	string(REPLACE ";" "\n  " _unexpected_text "${_unexpected}")
	message(FATAL_ERROR
		"${_deb} carries files nothing in this project installs deliberately:\n"
		"  ${_unexpected_text}\n"
		"Either the packaging picked up an install rule that is not ours "
		"(the C example module installs into Evolution's module directory "
		"with no component), or a new component needs adding to both the "
		"package and this test.")
endif()

#
# Control fields. A package that carries the right files under the wrong name,
# or with no dependencies, is one `apt install ./evolution-jmap.deb` refuses or
# — worse — accepts and then cannot load.
#
function(deb_field _name _out)
	execute_process(
		COMMAND ${DPKG_DEB_EXECUTABLE} --field "${_deb}" "${_name}"
		OUTPUT_VARIABLE _value
		OUTPUT_STRIP_TRAILING_WHITESPACE
		ERROR_VARIABLE _field_error
		RESULT_VARIABLE _field_result
	)
	if(NOT _field_result EQUAL 0)
		message(FATAL_ERROR "dpkg-deb --field ${_name} failed (${_field_result}):\n${_field_error}")
	endif()
	set(${_out} "${_value}" PARENT_SCOPE)
endfunction()

deb_field(Package _package)
if(NOT _package STREQUAL PACKAGE_NAME)
	message(FATAL_ERROR "package declares Package: ${_package}, expected ${PACKAGE_NAME}")
endif()

deb_field(Version _version)
if(NOT _version STREQUAL PACKAGE_VERSION)
	message(FATAL_ERROR "package declares Version: ${_version}, expected ${PACKAGE_VERSION}")
endif()

# The modules are compiled objects, so the package is architecture-specific;
# `all` here would be a package apt installs happily onto a machine that cannot
# load a single file in it.
execute_process(
	COMMAND ${DPKG_EXECUTABLE} --print-architecture
	OUTPUT_VARIABLE _host_arch
	OUTPUT_STRIP_TRAILING_WHITESPACE
	RESULT_VARIABLE _result
)
if(NOT _result EQUAL 0)
	message(FATAL_ERROR "dpkg --print-architecture failed (${_result})")
endif()
deb_field(Architecture _arch)
if(NOT _arch STREQUAL _host_arch)
	message(FATAL_ERROR "package declares Architecture: ${_arch}, built on ${_host_arch}")
endif()

deb_field(Maintainer _maintainer)
if(_maintainer STREQUAL "" OR _maintainer MATCHES "@example|nobody|unknown")
	message(FATAL_ERROR "package declares no real Maintainer: '${_maintainer}'")
endif()

# The dependencies have to be *derived*, not written down: these modules are
# dlopened by EDS and Camel, so a missing library is not a link error at
# install time but a backend that silently never appears. dpkg-shlibdeps
# reading our own ELF files is the only thing that knows what they need, and
# the two names below are how this test can tell it ran — nothing else in the
# package would mention Camel or the address book library.
deb_field(Depends _depends)
if(_depends STREQUAL "")
	message(FATAL_ERROR
		"package declares no Depends; dpkg-shlibdeps cannot have inspected the "
		"modules, so apt would install a package whose libraries may be absent")
endif()
foreach(_needed camel edata-book)
	if(NOT _depends MATCHES "${_needed}")
		message(FATAL_ERROR
			"Depends does not mention a lib${_needed} package, so the "
			"dependencies were not derived from the installed modules:\n"
			"  ${_depends}")
	endif()
endforeach()

# The Description is the one field a person reads before installing, in
# `apt show` and in every package browser, and Debian policy §5.6.13 gives it a
# shape: one synopsis line, then an extended description whose every line
# begins with exactly one space, with " ." for a paragraph break. The extra
# space that says "render this verbatim" is easy to emit by accident — it is
# what happens when a description written with its own indentation is handed to
# a generator that adds its own — and the result is an `apt show` that does not
# wrap. Checked here because nothing else in the build would notice.
deb_field(Description _description)
string(REPLACE "\n" ";" _description_lines "${_description}")
list(GET _description_lines 0 _synopsis)
if(NOT _synopsis STREQUAL PACKAGE_SUMMARY)
	message(FATAL_ERROR
		"package Description opens with '${_synopsis}', expected '${PACKAGE_SUMMARY}'")
endif()

list(REMOVE_AT _description_lines 0)
foreach(_line IN LISTS _description_lines)
	if(_line STREQUAL " ${PACKAGE_SUMMARY}")
		message(FATAL_ERROR
			"package Description repeats its own synopsis in the extended "
			"description; `apt show` would print it twice")
	endif()
	if(NOT _line MATCHES "^ ([^ ]|$)")
		message(FATAL_ERROR
			"extended Description line is not one space followed by text, so "
			"Debian reads it as preformatted and apt will not wrap it:\n"
			"  '${_line}'")
	endif()
endforeach()

file(SIZE "${_deb}" _deb_size)
get_filename_component(_deb_name "${_deb}" NAME)
list(LENGTH _actual _file_count)
message(STATUS
	"${_deb_name}: ${_deb_size} bytes, ${_file_count} files, "
	"${_package} ${_version} ${_arch}")
message(STATUS "  Depends: ${_depends}")
