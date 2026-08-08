# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Cargo integration: reproducibility plumbing and test registration.
# The `add_cargo_cdylib()` helper for installable backend modules lands
# with the first EDS backend.

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
# example module).
add_custom_target(rust-build
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
	COMMAND ${CARGO_EXECUTABLE} test --locked -p eds-sys -p jmap-backend-core -p jmap-backend-book
	WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
)
set_tests_properties(rust-test-eds PROPERTIES
	ENVIRONMENT "CARGO_INCREMENTAL=0"
)
