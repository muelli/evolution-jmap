# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Cargo integration: reproducibility plumbing, test registration, and
# `add_cargo_cdylib()` — the install rule for the backend modules EDS and
# Camel dlopen out of their own module directories.

find_program(CARGO_EXECUTABLE cargo REQUIRED)

set(CARGO_TARGET_DIR "${CMAKE_BINARY_DIR}/cargo-target")

# Reproducible builds: deterministic timestamp from the last commit, and
# source/cargo paths remapped out of the binaries.
execute_process(
	COMMAND git -C "${CMAKE_SOURCE_DIR}" log -1 --format=%ct
	OUTPUT_VARIABLE SOURCE_DATE_EPOCH
	OUTPUT_STRIP_TRAILING_WHITESPACE
	ERROR_QUIET
)
set(CARGO_ENV
	"SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}"
	"CARGO_INCREMENTAL=0"
	"RUSTFLAGS=--remap-path-prefix=${CMAKE_SOURCE_DIR}=/build --remap-path-prefix=$ENV{HOME}/.cargo=/cargo"
)

# Build the whole workspace (all members — needs Evolution headers for the
# example module). Part of ALL because the cdylibs add_cargo_cdylib()
# installs are files, not CMake targets: nothing else would build them, and
# `cmake --install` would find nothing to copy.
add_custom_target(rust-build ALL
	COMMAND ${CMAKE_COMMAND} -E env ${CARGO_ENV}
		${CARGO_EXECUTABLE} build --workspace --locked --release
		--target-dir "${CARGO_TARGET_DIR}"
	WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	COMMENT "cargo build --workspace --release"
	VERBATIM
)

# Run the Rust test suite through CTest (`ctest` or `make test`).
add_test(
	NAME rust-test
	COMMAND ${CARGO_EXECUTABLE} test --locked
	WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
)
set_tests_properties(rust-test PROPERTIES
	ENVIRONMENT "CARGO_INCREMENTAL=0"
)

# Crates kept out of the workspace's default-members because they need the
# EDS/Evolution development headers, so plain `cargo test` skips them. CMake
# has already established the headers are present, so run them here — this is
# the only place eds-sys's g_type_query layout checks get exercised.
add_test(
	NAME rust-test-eds
	COMMAND ${CARGO_EXECUTABLE} test --locked -p eds-sys -p jmap-backend-core
		-p jmap-backend-book -p jmap-backend-cal -p jmap-mail
	WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
)
set_tests_properties(rust-test-eds PROPERTIES
	ENVIRONMENT "CARGO_INCREMENTAL=0"
)

# Install a cdylib built by `rust-build` as a loadable module of some host —
# EDS names its address book backends libebookbackend<name>.so and looks for
# them in one directory, Camel has its own convention, and cargo knows about
# neither. Also registers a CTest that stages the install and checks the
# module came out the far end loadable, because an install rule that quietly
# copies nothing looks exactly like one that works.
#
#   add_cargo_cdylib(<cargo-lib-name>
#       OUTPUT_NAME <installed file name>
#       DESTINATION <absolute directory>
#       COMPONENT   <install component>
#       SYMBOLS     <entry point> ...
#       [DATA <file> ...]
#       [VERIFY_DESTINATION_FROM <pkg-config module> <variable>])
#
# DATA names files installed beside the module and checked to have arrived —
# Camel's `.urls` file is one, and it is not optional decoration: Camel reads
# it to decide whether to dlopen the module at all, so a component that
# installs the `.so` alone is a provider that is never loaded.
function(add_cargo_cdylib _lib_name)
	cmake_parse_arguments(_arg "" "OUTPUT_NAME;DESTINATION;COMPONENT" "SYMBOLS;DATA;VERIFY_DESTINATION_FROM" ${ARGN})

	foreach(_required OUTPUT_NAME DESTINATION COMPONENT SYMBOLS)
		if(NOT _arg_${_required})
			message(FATAL_ERROR "add_cargo_cdylib(${_lib_name}): ${_required} is required and must not be empty")
		endif()
	endforeach()
	# The destinations come from pkg_check_variable(), which reports a
	# missing variable as the empty string; that would silently install
	# into the prefix root.
	if(NOT IS_ABSOLUTE "${_arg_DESTINATION}")
		message(FATAL_ERROR "add_cargo_cdylib(${_lib_name}): DESTINATION '${_arg_DESTINATION}' is not an absolute path")
	endif()

	# PROGRAMS rather than FILES: a shared module wants mode 0755.
	install(PROGRAMS "${CARGO_TARGET_DIR}/release/lib${_lib_name}.so"
		DESTINATION "${_arg_DESTINATION}"
		RENAME "${_arg_OUTPUT_NAME}"
		COMPONENT "${_arg_COMPONENT}"
	)

	# FILES rather than PROGRAMS: these are read, not executed.
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

	# Let the test re-derive the directory from pkg-config, so DESTINATION
	# is checked against its source. Not under FORCE_INSTALL_PREFIX, where
	# the point is that the destination has deliberately been moved.
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
