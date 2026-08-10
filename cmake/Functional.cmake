# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# M9 layer 1: the headless functional tests, which drive real Evolution Data
# Server daemons against the in-repo mock JMAP server. Requires
# cmake/Rust.cmake for CARGO_TARGET_DIR.
#
# Off by default, and loudly required when on. These tests need the EDS
# *runtime* — evolution-source-registry and evolution-addressbook-factory,
# from the `evolution-data-server` package rather than the `-dev` ones every
# other target here builds against — plus a D-Bus implementation. The shared
# CI image has neither, so a test registered unconditionally would either
# fail every CI run or, worse, be written to skip itself and report green on
# a machine where it never ran. Gating on an explicit option keeps "did not
# run" distinguishable from "passed": with -DENABLE_FUNCTIONAL_TESTS=ON a
# missing runtime is a configure error, never a silent pass.
#
#   cmake -S . -B build -DENABLE_FUNCTIONAL_TESTS=ON
#   cmake --build build
#   ctest --test-dir build -L functional --output-on-failure
#
# See docs/functional-tests.md.

option(ENABLE_FUNCTIONAL_TESTS
	"Build and register the headless functional tests against real EDS daemons"
	OFF)

if(ENABLE_FUNCTIONAL_TESTS)
	# The private session bus each test runs on. Without it the tests would
	# reach the developer's own daemons, started with the developer's own
	# environment and pointed at the developer's own Evolution data.
	find_program(DBUS_RUN_SESSION_EXECUTABLE dbus-run-session)
	if(NOT DBUS_RUN_SESSION_EXECUTABLE)
		message(FATAL_ERROR
			"ENABLE_FUNCTIONAL_TESTS is ON but dbus-run-session was not found "
			"(Debian/Ubuntu: dbus-daemon; Fedora: dbus-daemon)")
	endif()

	# The daemons themselves. They are D-Bus activated, so the tests never
	# name these paths — but a machine without them fails at activation time
	# with a bus error that says nothing about what is missing, and it is
	# worth spending a find_program to say it here instead. libexecdir is
	# not on PATH, and no pkg-config module reports it.
	foreach(_daemon evolution-source-registry evolution-addressbook-factory)
		string(TOUPPER "${_daemon}" _daemon_variable)
		string(REPLACE "-" "_" _daemon_variable "${_daemon_variable}")
		find_program(${_daemon_variable}_EXECUTABLE ${_daemon}
			PATHS /usr/libexec /usr/lib/evolution-data-server
			      /usr/libexec/evolution-data-server /usr/local/libexec)
		if(NOT ${_daemon_variable}_EXECUTABLE)
			message(FATAL_ERROR
				"ENABLE_FUNCTIONAL_TESTS is ON but ${_daemon} was not found. "
				"These tests need the Evolution Data Server runtime installed "
				"(Debian/Ubuntu: evolution-data-server), not just its "
				"development headers.")
		endif()
	endforeach()

	# The client half of the address book test: an ordinary libebook
	# consumer, which is a surface no crate in this repository binds — see
	# tests/functional/book-client.c.
	pkg_check_modules(LIBEBOOK REQUIRED libebook-1.2>=${REQUIRE_EVOLUTION_VERSION})

	add_executable(functional-book-client tests/functional/book-client.c)
	target_include_directories(functional-book-client PRIVATE ${LIBEBOOK_INCLUDE_DIRS})
	target_compile_options(functional-book-client PRIVATE ${LIBEBOOK_CFLAGS_OTHER})
	target_link_libraries(functional-book-client PRIVATE ${LIBEBOOK_LIBRARIES})
	target_link_directories(functional-book-client PRIVATE ${LIBEBOOK_LIBRARY_DIRS})

	# The Rust side builds the scratch EDS installation, runs the client on
	# a private bus and holds both ends to what they should have said. It is
	# handed the two paths only CMake knows: the client just built, and the
	# backend cargo built, which the harness stages as the factory's one
	# address book module.
	add_test(
		NAME functional-book
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-book PROPERTIES
		LABELS functional
		# Generous, because it covers activating two daemons and a first
		# connect; the point of a limit at all is that a wedged factory
		# fails the run instead of hanging it.
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_BOOK_CLIENT=$<TARGET_FILE:functional-book-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so"
	)
endif()
