# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Cargo integration: reproducible-build settings, test registration, and
# `add_cargo_cdylib()`. That function is the install rule for the backend
# modules that EDS and Camel dlopen from their module directories.

find_program(CARGO_EXECUTABLE cargo REQUIRED)

set(CARGO_TARGET_DIR "${CMAKE_BINARY_DIR}/cargo-target")

# Reproducible builds: a fixed timestamp from the environment or the last
# commit. The binaries get remapped source and cargo paths.
#
# SOURCE_DATE_EPOCH must be a valid integer or not set. clang stops with an
# error on an incorrect value, which bindgen shows only as "libclang error".
# This occurred in CI: git refuses checkouts with a different owner, and the
# variable was then empty.
if(DEFINED ENV{SOURCE_DATE_EPOCH} AND NOT "$ENV{SOURCE_DATE_EPOCH}" STREQUAL "")
	set(SOURCE_DATE_EPOCH "$ENV{SOURCE_DATE_EPOCH}")
else()
	execute_process(
		COMMAND git -C "${CMAKE_SOURCE_DIR}" log -1 --format=%ct
		OUTPUT_VARIABLE SOURCE_DATE_EPOCH
		OUTPUT_STRIP_TRAILING_WHITESPACE
		ERROR_QUIET
	)
endif()

# EVOLUTION_JMAP_LOCALEDIR: the directory where the modules find their
# translation catalogues.
#
# The build.rs of jmap-backend-core puts this value into the binaries. A
# dlopened module cannot find its install location, and the gettext default
# (/usr/share/locale) is correct only for the /usr prefix. The test commands
# below intentionally do not get this variable. A test binary has no install
# location, and without the variable build.rs uses /usr/share/locale, the
# same as gettext.
set(CARGO_ENV
	"CARGO_INCREMENTAL=0"
	"EVOLUTION_JMAP_LOCALEDIR=${LANGUAGE_SUPPORT_DIRECTORY}"
	"RUSTFLAGS=--remap-path-prefix=${CMAKE_SOURCE_DIR}=/build --remap-path-prefix=$ENV{HOME}/.cargo=/cargo"
)

find_program(SCCACHE_EXECUTABLE sccache)
if(SCCACHE_EXECUTABLE)
	list(APPEND CARGO_ENV "RUSTC_WRAPPER=${SCCACHE_EXECUTABLE}")
endif()

if(SOURCE_DATE_EPOCH MATCHES "^[0-9]+$")
	list(APPEND CARGO_ENV "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}")
else()
	message(WARNING
		"SOURCE_DATE_EPOCH unavailable (not exported, and git gave no commit "
		"timestamp) — the build will work but will not be reproducible")
endif()

# Build all workspace members (the example module needs the Evolution
# headers). This target is part of ALL because the cdylibs that
# add_cargo_cdylib() installs are files, not CMake targets. No other target
# builds them, and `cmake --install` would find nothing to copy.
add_custom_target(rust-build ALL
	COMMAND ${CMAKE_COMMAND} -E env ${CARGO_ENV}
		${CARGO_EXECUTABLE} build --workspace --locked --release
		--target-dir "${CARGO_TARGET_DIR}"
	WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	COMMENT "cargo build --workspace --release"
	VERBATIM
)

# Run the Rust tests through CTest (`ctest` or `make test`).
add_test(
	NAME rust-test
	COMMAND ${CARGO_EXECUTABLE} test --locked
	WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
)
set(_test_env "CARGO_INCREMENTAL=0")
if(SCCACHE_EXECUTABLE)
	list(APPEND _test_env "RUSTC_WRAPPER=${SCCACHE_EXECUTABLE}")
endif()
set_tests_properties(rust-test PROPERTIES ENVIRONMENT "${_test_env}")

# These crates are not in the workspace default-members because they need the
# EDS/Evolution development headers, so plain `cargo test` skips them. CMake
# has already found the headers, so run them here. This is the only test that
# runs the g_type_query layout checks in eds-sys and evo-sys.
add_test(
	NAME rust-test-eds
	COMMAND ${CARGO_EXECUTABLE} test --locked -p eds-sys -p evo-sys
		-p jmap-backend-core
		-p jmap-backend-book -p jmap-backend-cal -p jmap-mail
		-p jmap-backend-collection -p jmap-config -p jmap-ui
	WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
)
set_tests_properties(rust-test-eds PROPERTIES ENVIRONMENT "${_test_env}")

# Plain `cargo clippy` lints only the default members. This test lints the
# full workspace, so the crates that need EDS also get the -D warnings check.
add_test(
	NAME rust-clippy-workspace
	COMMAND ${CARGO_EXECUTABLE} clippy --workspace --all-targets --locked
		-- -D warnings
	WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
)
set_tests_properties(rust-clippy-workspace PROPERTIES ENVIRONMENT "${_test_env}")

# Install a cdylib from `rust-build` as a loadable module of a host. EDS
# names its address book backends libebookbackend<name>.so and finds them in
# one directory. Camel has its own convention. cargo knows neither.
# The function also registers a CTest that stages the install and checks that
# the module is loadable. An install rule that silently copies nothing looks
# the same as one that works.
#
#   add_cargo_cdylib(<cargo-lib-name>
#       OUTPUT_NAME <installed file name>
#       DESTINATION <absolute directory>
#       COMPONENT   <install component>
#       SYMBOLS     <entry point> ...
#       [DATA <file> ...]
#       [VERIFY_DESTINATION_FROM <pkg-config module> <variable>])
#
# DATA gives files that go next to the module; the test checks that they are
# installed. Camel's `.urls` file is one, and it is necessary: Camel reads it
# to decide whether to dlopen the module. Without it, Camel never loads the
# provider.
function(add_cargo_cdylib _lib_name)
	cmake_parse_arguments(_arg "" "OUTPUT_NAME;DESTINATION;COMPONENT" "SYMBOLS;DATA;VERIFY_DESTINATION_FROM" ${ARGN})

	foreach(_required OUTPUT_NAME DESTINATION COMPONENT SYMBOLS)
		if(NOT _arg_${_required})
			message(FATAL_ERROR "add_cargo_cdylib(${_lib_name}): ${_required} is required and must not be empty")
		endif()
	endforeach()
	# The destinations come from pkg_check_variable(), which gives an empty
	# string for a missing variable. The install would then silently go into
	# the prefix root.
	if(NOT IS_ABSOLUTE "${_arg_DESTINATION}")
		message(FATAL_ERROR "add_cargo_cdylib(${_lib_name}): DESTINATION '${_arg_DESTINATION}' is not an absolute path")
	endif()

	# PROGRAMS, not FILES: a shared module needs mode 0755.
	install(PROGRAMS "${CARGO_TARGET_DIR}/release/lib${_lib_name}.so"
		DESTINATION "${_arg_DESTINATION}"
		RENAME "${_arg_OUTPUT_NAME}"
		COMPONENT "${_arg_COMPONENT}"
	)

	# lintian's unstripped-binary-or-object. CMake's STRIP option applies only
	# to `install(TARGETS ...)`, not to `install(PROGRAMS ...)`. CPack's
	# CPACK_STRIP_FILES did not strip these five modules, for the same reason.
	# An install(CODE) block strips the file at its install location. This
	# applies to a real `cmake --install` and to the CPack staging tree.
	install(CODE
		"execute_process(COMMAND \"${CMAKE_STRIP}\" \"\$ENV{DESTDIR}${_arg_DESTINATION}/${_arg_OUTPUT_NAME}\")"
		COMPONENT "${_arg_COMPONENT}"
	)

	# FILES, not PROGRAMS: these are read, not executed.
	set(_expected_data)
	foreach(_data IN LISTS _arg_DATA)
		if(NOT EXISTS "${_data}")
			message(FATAL_ERROR "add_cargo_cdylib(${_lib_name}): DATA file '${_data}' does not exist")
		endif()
		install(FILES "${_data}"
			DESTINATION "${_arg_DESTINATION}"
			COMPONENT "${_arg_COMPONENT}"
		)
		get_filename_component(_data_name "${_data}" NAME)
		list(APPEND _expected_data "${_arg_DESTINATION}/${_data_name}")
	endforeach()

	# The test gets the directory from pkg-config again, to check DESTINATION
	# against its source. Not with FORCE_INSTALL_PREFIX, which intentionally
	# moves the destination.
	set(_pkg_args)
	if(_arg_VERIFY_DESTINATION_FROM AND NOT FORCE_INSTALL_PREFIX)
		list(LENGTH _arg_VERIFY_DESTINATION_FROM _count)
		if(NOT _count EQUAL 2)
			message(FATAL_ERROR "add_cargo_cdylib(${_lib_name}): VERIFY_DESTINATION_FROM takes a pkg-config module and a variable name")
		endif()
		list(GET _arg_VERIFY_DESTINATION_FROM 0 _pkg_module)
		list(GET _arg_VERIFY_DESTINATION_FROM 1 _pkg_variable)
		list(APPEND _pkg_args "-DPKG_MODULE=${_pkg_module}" "-DPKG_VARIABLE=${_pkg_variable}")
	endif()

	set(_data_args)
	if(_expected_data)
		string(REPLACE ";" "|" _expected_data "${_expected_data}")
		list(APPEND _data_args "-DEXPECTED_DATA=${_expected_data}")
	endif()

	string(REPLACE ";" "|" _symbols "${_arg_SYMBOLS}")
	add_test(
		NAME install-${_arg_COMPONENT}
		COMMAND ${CMAKE_COMMAND}
			${_pkg_args}
			${_data_args}
			"-DBUILD_DIR=${CMAKE_BINARY_DIR}"
			"-DSTAGE_DIR=${CMAKE_BINARY_DIR}/install-test/${_arg_COMPONENT}"
			"-DCOMPONENT=${_arg_COMPONENT}"
			"-DEXPECTED=${_arg_DESTINATION}/${_arg_OUTPUT_NAME}"
			"-DSYMBOLS=${_symbols}"
			-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-installed-module.cmake"
	)
endfunction()
