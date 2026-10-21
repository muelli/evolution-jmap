# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Packaging check for the binary `.deb`. Run with `cmake -P`. The caller
# sets:
#
#   BUILD_DIR        the CMake build tree to package
#   STAGE_DIR        a scratch directory for cpack output
#   PACKAGE_NAME     the Debian package name that the .deb must declare
#   PACKAGE_VERSION  the version that the .deb must declare
#   PACKAGE_SUMMARY  the one-line synopsis at the start of the Description
#   EXPECTED         '|'-separated absolute paths of all regular files in the
#                    package. The test compares for equality, not subset, so
#                    the package must not contain other files.
#
# The per-component `install-*` tests make sure that each module goes into the
# correct directory. This test makes sure that the installed *package* has the
# same files and no other files.
#
# The "no other files" part is the important part. This build tree also
# installs the upstream C example module into the Evolution module directory,
# without an install component. A monolithic package ("install all") thus puts
# a demonstration module on each machine that installs JMAP support.
# That package installs and works, but it contains an unwanted file.
# The file-list equality makes this a test failure.

# Script mode uses the policies from this line, not from the project.
# file(SIZE) needs 3.14, which is newer than the project minimum.
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

# Require exactly one .deb, so that the test cannot examine an old .deb from a
# previous run. For the same reason, the script deletes STAGE_DIR above.
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

# `dpkg-deb --contents` prints one line per member, similar to `ls -l`.
# Each path is relative to the package root, so it starts with `./`.
# The test compares only regular files. The parent directories come from the
# archive build. A package that makes /usr/lib/evolution-data-server for a
# backend is not an error.
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

foreach(_file IN LISTS _actual)
	if(_file MATCHES "^/usr/local/")
		message(FATAL_ERROR
			"${_deb} carries files under /usr/local, violating Debian policy:\n"
			"  ${_file}\n"
			"Package install destinations must use /usr, never /usr/local.")
	endif()
endforeach()

#
# Control fields. If the package has the correct files but an incorrect name,
# or no dependencies, `apt install ./evolution-jmap.deb` can refuse it.
# Worse, apt can accept it, and then the modules do not load.
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

# The modules are compiled objects, so the package is architecture-specific.
# With `all`, apt installs the package on a machine that cannot load its files.
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

# The build must *derive* the dependencies, not use a written list.
# EDS and Camel dlopen these modules. Thus a missing library does not cause a
# link error at install time. It causes a backend that does not appear, with
# no error. Only dpkg-shlibdeps knows what the ELF files of this project need.
# The two names below show that dpkg-shlibdeps ran. No other part of the
# package mentions Camel or the address book library.
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

# A person reads the Description before installation, in `apt show` and in
# package browsers. Debian policy §5.6.13 gives its format: one synopsis line,
# then an extended description.
# Each extended line starts with exactly one space. " ." is a paragraph break.
# A second space means "show this line verbatim", and is easy to add by
# accident. This occurs when a generator adds indentation to an already
# indented description. Then `apt show` does not wrap the text.
# No other part of the build checks this.
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
