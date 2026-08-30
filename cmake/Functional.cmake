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
	foreach(_daemon evolution-source-registry evolution-addressbook-factory
			evolution-calendar-factory)
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

	# The client halves: ordinary libebook and libecal consumers, which are
	# surfaces no crate in this repository binds — see
	# tests/functional/book-client.c.
	pkg_check_modules(LIBEBOOK REQUIRED libebook-1.2>=${REQUIRE_EVOLUTION_VERSION})
	pkg_check_modules(LIBECAL REQUIRED libecal-2.0>=${REQUIRE_EVOLUTION_VERSION})

	# connection-status.c is compiled into both: the question it answers —
	# did EDS decide the backend is connected? — is the same for a book and
	# for a calendar, and so is the main-loop dance it takes to ask.
	add_executable(functional-book-client
		tests/functional/book-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-book-client PRIVATE ${LIBEBOOK_INCLUDE_DIRS})
	target_compile_options(functional-book-client PRIVATE ${LIBEBOOK_CFLAGS_OTHER})
	target_link_libraries(functional-book-client PRIVATE ${LIBEBOOK_LIBRARIES})
	target_link_directories(functional-book-client PRIVATE ${LIBEBOOK_LIBRARY_DIRS})

	# Five calendar clients rather than one, because they ask different
	# questions: cal-client.c creates every event it looks at, cal-edit-client.c
	# reads one the server already held and saves back the members no iCalendar
	# line has room for, cal-zone-client.c asks what instant an event's zone
	# resolves to — before and after each of the two kinds of edit a user can make
	# to such an event — cal-color-client.c asks the same question D1 asks of
	# create/delete, but of D2's colour push: does a live backend instance's
	# `source_changed` vfunc actually fire and reach the server, not just the
	# pure decision function behind it — and cal-free-busy-client.c asks that
	# same "does the live vfunc actually run" question of Track E Path A's
	# `get_free_busy_sync`. See the header of each.
	#
	# The third takes a mode argument, which the others do not, because it asks
	# its one question from both ends: `read` starts from a zone only the server
	# can name, `create` from one only the client can. The two are separate runs
	# and not phases of one — `create`'s zone must be in the calendar's timezone
	# store for one reason only, that the client put it there, and a run that had
	# read a server's zone first would have filled that store from elsewhere.
	#
	# event-start.c holds the one function that reports the instant a start
	# resolves to, so that a difference between what two of these programs answer
	# is a difference in the event rather than in how it was measured. It is
	# compiled into all five because that keeps this a single loop; cal-client.c,
	# cal-color-client.c and cal-free-busy-client.c do not call it, and an
	# unreferenced function costs nothing here.
	foreach(_client cal cal-edit cal-zone cal-color cal-free-busy)
		add_executable(functional-${_client}-client
			tests/functional/${_client}-client.c
			tests/functional/connection-status.c
			tests/functional/event-start.c)
		target_include_directories(functional-${_client}-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
		target_compile_options(functional-${_client}-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
		target_link_libraries(functional-${_client}-client PRIVATE ${LIBECAL_LIBRARIES})
		target_link_directories(functional-${_client}-client PRIVATE ${LIBECAL_LIBRARY_DIRS})
	endforeach()

	# The config-lookup client is the odd one out in a different way: it does
	# not open a `.source` keyfile at all, because a lookup happens *before*
	# an account exists. It links evolution-shell-3.0 rather than a libe*
	# client library — EConfigLookup lives in e-util (Evolution's own
	# library, not EDS's) — which the top-level CMakeLists.txt already
	# requires unconditionally for module-jmap-configuration.so itself, so
	# EVOLUTION_SHELL's variables are already populated here with nothing new
	# to check for.
	# The collection client is another odd one out: it opens no book or
	# calendar and connects to no factory, only to the registry itself —
	# the daemon that loads `module-jmap-backend.so` (`EDS_REGISTRY_MODULES`,
	# staged by `Session::stage_collection_backend`) and is the one process
	# whose populate/fan-out this test is about. `libedataserver` alone is
	# enough, the same library the mail/transport clients below link for
	# their own registry lookups.
	# The calendar analogue of `functional-book-client`'s `list` phase, for
	# `get_changes_sync` coverage — see tests/functional/cal-changes-client.c.
	add_executable(functional-cal-changes-client tests/functional/cal-changes-client.c)
	target_include_directories(functional-cal-changes-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-changes-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-changes-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-changes-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	pkg_check_modules(LIBEDATASERVER REQUIRED libedataserver-1.2>=${REQUIRE_EVOLUTION_VERSION})
	add_executable(functional-collection-client tests/functional/collection-client.c)
	target_include_directories(functional-collection-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-collection-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-collection-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-collection-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# The write half of the same surface: "New Address Book"/"Delete", i.e.
	# `e_source_remote_create_sync`/`e_source_remote_delete_sync` against the
	# same registry, proving `create_resource_sync`/`delete_resource_sync`
	# rather than populate/fan-out. A separate client, not a mode of the one
	# above, for the same reason the cal-client family is split: each answers
	# one question against the registry, not several.
	add_executable(functional-collection-create-client tests/functional/collection-create-client.c)
	target_include_directories(functional-collection-create-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-collection-create-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-collection-create-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-collection-create-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# The calendar sibling of the client above: the same create/delete pair,
	# against E_SOURCE_EXTENSION_CALENDAR instead of the address-book
	# extension, proving create_resource_sync/delete_resource_sync do not
	# mix the two kinds of child up.
	add_executable(functional-collection-create-calendar-client tests/functional/collection-create-calendar-client.c)
	target_include_directories(functional-collection-create-calendar-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-collection-create-calendar-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-collection-create-calendar-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-collection-create-calendar-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# Item 22's reproduction client. Another registry-only consumer like the
	# three above, but standing in for a long-lived backend *factory*: it
	# holds an ESource across a registry restart and asks it for an OAuth 2.0
	# access token afterwards. libedataserver alone, plus the platform's
	# kill(2) — see the file's own header for the EDS source it rests on.
	add_executable(functional-oauth2-stale-proxy-client tests/functional/oauth2-stale-proxy-client.c)
	target_include_directories(functional-oauth2-stale-proxy-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-oauth2-stale-proxy-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-oauth2-stale-proxy-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-oauth2-stale-proxy-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# EDS's OWN registry module, not one this project builds. `EDS_REGISTRY_
	# MODULES` replaces EDS's module directory rather than adding to it
	# (`e-source-registry-server.c:1073`), so a functional session sees only
	# what it stages — and the stale-proxy test needs this one, whose
	# `EOAuth2SourceMonitor` is what exports the `Source.OAuth2Support` D-Bus
	# interface for an account whose `[Authentication] Method` names a
	# registered `EOAuth2Service`. Found rather than assumed, and fatal when
	# missing, for the reason the whole file is built that way: without it the
	# test still runs and still exercises a registry, but the interface it is
	# about is never exported, so it would measure nothing.
	find_file(MODULE_OAUTH2_SERVICES_LIBRARY module-oauth2-services.so
		PATHS /usr/lib/evolution-data-server/registry-modules
		      /usr/lib64/evolution-data-server/registry-modules
		      /usr/local/lib/evolution-data-server/registry-modules
		PATH_SUFFIXES ""
		NO_DEFAULT_PATH)
	if(NOT MODULE_OAUTH2_SERVICES_LIBRARY)
		message(FATAL_ERROR
			"ENABLE_FUNCTIONAL_TESTS is ON but EDS's own "
			"module-oauth2-services.so was not found. It ships with the "
			"evolution-data-server runtime package; docs/ROADMAP.md item 22's "
			"reproduction stages it beside module-jmap-backend.so because "
			"EDS_REGISTRY_MODULES replaces the module directory rather than "
			"extending it.")
	endif()

	# docs/ROADMAP.md item 25's calendar leg. Another libecal consumer, but
	# with two things the five above do not need: it seeds the secret store
	# (`e_secret_store_store_sync`) before it connects, and it asks whether the
	# registry exported `Source.OAuth2Support` for its source
	# (`e_source_ref_dbus_object`). Both are libedataserver, which is therefore
	# named here rather than relied on as a transitive of libecal — and which
	# is why this target sits below `pkg_check_modules(LIBEDATASERVER ...)`
	# rather than beside the other calendar clients.
	add_executable(functional-cal-stale-token-client
		tests/functional/cal-stale-token-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-stale-token-client PRIVATE
		${LIBECAL_INCLUDE_DIRS} ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-cal-stale-token-client PRIVATE
		${LIBECAL_CFLAGS_OTHER} ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-cal-stale-token-client PRIVATE
		${LIBECAL_LIBRARIES} ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-cal-stale-token-client PRIVATE
		${LIBECAL_LIBRARY_DIRS} ${LIBEDATASERVER_LIBRARY_DIRS})

	# docs/ROADMAP.md item 25's address-book leg. The same shape as
	# functional-cal-stale-token-client above, with libebook swapped for
	# libecal — see that target's own comment and the client's header for
	# why libedataserver is named here too.
	add_executable(functional-book-stale-token-client
		tests/functional/book-stale-token-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-book-stale-token-client PRIVATE
		${LIBEBOOK_INCLUDE_DIRS} ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-book-stale-token-client PRIVATE
		${LIBEBOOK_CFLAGS_OTHER} ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-book-stale-token-client PRIVATE
		${LIBEBOOK_LIBRARIES} ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-book-stale-token-client PRIVATE
		${LIBEBOOK_LIBRARY_DIRS} ${LIBEDATASERVER_LIBRARY_DIRS})

	# docs/ROADMAP.md item 26's reproduction client. A registry-only
	# libedataserver consumer like functional-collection-client, plus GSettings
	# — which is not a separate dependency here: `gio-2.0` comes in with
	# libedataserver's own pkg-config, and `g_settings_new` is what lets this
	# client plant the operator's dconf debris itself rather than have the
	# harness hand-edit a GVDB file.
	add_executable(functional-stale-source-uid-client tests/functional/stale-source-uid-client.c)
	target_include_directories(functional-stale-source-uid-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-stale-source-uid-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-stale-source-uid-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-stale-source-uid-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	add_executable(functional-config-lookup-client tests/functional/config-lookup-client.c)
	target_include_directories(functional-config-lookup-client PRIVATE ${EVOLUTION_SHELL_INCLUDE_DIRS})
	target_compile_options(functional-config-lookup-client PRIVATE ${EVOLUTION_SHELL_CFLAGS_OTHER})
	target_link_libraries(functional-config-lookup-client PRIVATE ${EVOLUTION_SHELL_LIBRARIES})
	target_link_directories(functional-config-lookup-client PRIVATE ${EVOLUTION_SHELL_LIBRARY_DIRS})

	# The mail client is the odd one out and does not link a client library
	# at all: there is no libecamel to match libebook and libecal, because a
	# Camel provider is loaded into the mail client's own process. This
	# program *is* that process, so it links camel itself, plus
	# libedataserver for the ESourceRegistry and the ESourceCamel machinery
	# that turns the keyfile's settings into a configured CamelService.
	#
	# No connection-status.c: that is an EClient notion, and a CamelService
	# reports its own connection status synchronously.
	pkg_check_modules(CAMEL_CLIENT REQUIRED camel-1.2>=${REQUIRE_EVOLUTION_VERSION})
	pkg_check_modules(LIBEDATASERVER REQUIRED libedataserver-1.2>=${REQUIRE_EVOLUTION_VERSION})

	# The sending half is a program of its own rather than a mode of the
	# receiving one: it opens no store, and what it walks — account to
	# identity to transport, through two uids — has nothing in common with
	# opening a folder. Same libraries, because it is the same kind of
	# process: a libcamel consumer that is also the provider's host.
	# `mail-stale-token` is a third program of the same kind, and the only
	# one here that subclasses a Camel class: `docs/ROADMAP.md` item 25 needs
	# a session that answers `get_oauth2_access_token_sync`, which the base
	# `CamelSession` does not — see the file's own header.
	foreach(_client mail transport mail-stale-token)
		add_executable(functional-${_client}-client tests/functional/${_client}-client.c)
		target_include_directories(functional-${_client}-client PRIVATE
			${CAMEL_CLIENT_INCLUDE_DIRS} ${LIBEDATASERVER_INCLUDE_DIRS})
		target_compile_options(functional-${_client}-client PRIVATE
			${CAMEL_CLIENT_CFLAGS_OTHER} ${LIBEDATASERVER_CFLAGS_OTHER})
		target_link_libraries(functional-${_client}-client PRIVATE
			${CAMEL_CLIENT_LIBRARIES} ${LIBEDATASERVER_LIBRARIES})
		target_link_directories(functional-${_client}-client PRIVATE
			${CAMEL_CLIENT_LIBRARY_DIRS} ${LIBEDATASERVER_LIBRARY_DIRS})
	endforeach()

	# The Rust side builds the scratch EDS installation, runs the client on
	# a private bus and holds both ends to what they should have said. Each
	# test is registered on its own — `cargo test --test <name>` rather than
	# one run of the whole crate — so that a failure names the surface that
	# broke, and so that each gets only the paths it needs. The paths are the
	# ones only CMake knows: the client just built, and the backend cargo
	# built, which the harness stages as that factory's one module.
	#
	# The two share a cargo target directory, so cargo's own lock serialises
	# them however CTest schedules them.
	add_test(
		NAME functional-book
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test address-book
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

	# Reuses `functional-book-client`'s own binary and module (the `list`
	# phase is just another mode of that program) — the new coverage here is
	# in what the Rust test drives it through: two separate connects sharing
	# one on-disk cache, to reach `get_changes_sync` rather than
	# `list_existing_sync` a second time. See `tests/book-changes.rs`.
	add_test(
		NAME functional-book-changes
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test book-changes
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-book-changes PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_BOOK_CLIENT=$<TARGET_FILE:functional-book-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so"
	)

	add_test(
		NAME functional-cal
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test calendar
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_CLIENT=$<TARGET_FILE:functional-cal-client>;JMAP_FUNCTIONAL_CAL_EDIT_CLIENT=$<TARGET_FILE:functional-cal-edit-client>;JMAP_FUNCTIONAL_CAL_ZONE_CLIENT=$<TARGET_FILE:functional-cal-zone-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# The calendar twin of `functional-book-changes`: two connects sharing
	# one on-disk cache, to reach `get_changes_sync` rather than
	# `list_existing_sync` a second time. See `tests/calendar-changes.rs`.
	add_test(
		NAME functional-cal-changes
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test calendar-changes
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-changes PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT=$<TARGET_FILE:functional-cal-changes-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# D2's write half through a real, running backend instance: a colour edit
	# on the calendar's own `ESourceSelectable`, and whether the running
	# `source_changed` vfunc pushes it — not just whether the pure decision
	# function behind it (`jmap-backend-cal/tests/ops.rs`) would.
	add_test(
		NAME functional-cal-color
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test calendar-color
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-color PROPERTIES
		LABELS functional
		# Generous for the usual reason, plus PUSH_SETTLE_SECONDS' own
		# deliberate wait inside the client.
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_COLOR_CLIENT=$<TARGET_FILE:functional-cal-color-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# Track E Path A's `get_free_busy_sync` through a real, running backend
	# instance: a free/busy query for a seeded principal, and whether the
	# running vfunc actually round-trips it through `Principal/query` +
	# `Principal/getAvailability` — not just whether the pure decision
	# function behind it (`jmap-backend-cal/tests/ops.rs`) would.
	add_test(
		NAME functional-cal-free-busy
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test calendar-free-busy
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-free-busy PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_FREE_BUSY_CLIENT=$<TARGET_FILE:functional-cal-free-busy-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# The collection backend's populate/fan-out: a real
	# evolution-source-registry loading module-jmap-backend.so for the
	# account keyfile the Rust side writes, and the client above waiting for
	# the address-book and calendar children `docs/manual-test-collection-
	# backend.md`'s recipe describes to appear via the registry's own API.
	add_test(
		NAME functional-collection
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test collection
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-collection PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_COLLECTION_CLIENT=$<TARGET_FILE:functional-collection-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so"
	)

	# The write half: "New Address Book"/"Delete" through the same registry,
	# proving create_resource_sync/delete_resource_sync end to end.
	add_test(
		NAME functional-collection-create
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test collection-create
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-collection-create PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_COLLECTION_CREATE_CLIENT=$<TARGET_FILE:functional-collection-create-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so"
	)

	# The calendar leg of the same write half: "New Calendar"/"Delete"
	# against E_SOURCE_EXTENSION_CALENDAR, proving create_resource_sync/
	# delete_resource_sync pick the right `/set` call for the child kind.
	add_test(
		NAME functional-collection-create-calendar
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test collection-create-calendar
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-collection-create-calendar PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_COLLECTION_CREATE_CALENDAR_CLIENT=$<TARGET_FILE:functional-collection-create-calendar-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so"
	)

	# module-jmap-configuration.so, not one of the four EDS/Camel backends:
	# the client loads it itself (see config-lookup-client.c), so it needs the
	# built module's own path rather than a daemon's module-directory
	# variable.
	add_test(
		NAME functional-config-lookup
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test config-lookup
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-config-lookup PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CONFIG_LOOKUP_CLIENT=$<TARGET_FILE:functional-config-lookup-client>;JMAP_FUNCTIONAL_CONFIG_LOOKUP_MODULE=${CARGO_TARGET_DIR}/release/libjmap_config_module.so"
	)

	# The mail leg needs a third path the other two do not: the `.urls` file,
	# which is what makes Camel open the module at all and which is
	# therefore staged from the source tree rather than written by the test.
	add_test(
		NAME functional-mail
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test mail
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-mail PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_MAIL_CLIENT=$<TARGET_FILE:functional-mail-client>;JMAP_FUNCTIONAL_MAIL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_mail.so;JMAP_FUNCTIONAL_MAIL_URLS=${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls"
	)

	# The send half, which stages the same module and the same `.urls` file
	# under the same two variable names: it is one provider, and a test that
	# was given a module of its own could pass against a build the receiving
	# leg never saw.
	add_test(
		NAME functional-transport
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test transport
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-transport PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_TRANSPORT_CLIENT=$<TARGET_FILE:functional-transport-client>;JMAP_FUNCTIONAL_MAIL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_mail.so;JMAP_FUNCTIONAL_MAIL_URLS=${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls"
	)

	# docs/ROADMAP.md item 25: item 23's acceptance test, made headless — the
	# hourly re-consent, driven through a real `CamelService` rather than
	# through the operator leaving Evolution open for an afternoon. Stages the
	# same module and `.urls` file as the two legs above, under the same two
	# variable names and for the same reason: it is one provider.
	add_test(
		NAME functional-mail-stale-token
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test mail-stale-token -- --test-threads=1
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-mail-stale-token PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_MAIL_STALE_TOKEN_CLIENT=$<TARGET_FILE:functional-mail-stale-token-client>;JMAP_FUNCTIONAL_MAIL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_mail.so;JMAP_FUNCTIONAL_MAIL_URLS=${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls"
	)

	# docs/ROADMAP.md item 25's calendar leg: the same acceptance test as
	# `functional-mail-stale-token`, for the backend whose refresh goes through
	# an `ESource` rather than a `CamelSession`. Needs three modules where
	# every other calendar test needs one — the calendar backend in the
	# factory, and, in the registry, `module-jmap-backend.so` (the "JMAP"
	# EOAuth2Service and the `[JMAP OAuth2]` extension type) beside EDS's own
	# oauth2-services module (which exports `Source.OAuth2Support`). See the
	# test's own header for why none of the three is optional.
	# `--test-threads=1`: the two tests each stand up a registry, a factory and
	# a keyring daemon of their own.
	add_test(
		NAME functional-cal-stale-token
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test cal-stale-token -- --test-threads=1
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-stale-token PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_STALE_TOKEN_CLIENT=$<TARGET_FILE:functional-cal-stale-token-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so;JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE=${MODULE_OAUTH2_SERVICES_LIBRARY}"
	)

	# docs/ROADMAP.md item 25's address-book leg: the same acceptance test as
	# `functional-cal-stale-token`, for `jmap-backend-book`'s own
	# `with_connection`/`retry_on_authentication_failure` wiring. Same three
	# modules, same reasoning, with libebook's factory and backend in place of
	# libecal's.
	add_test(
		NAME functional-book-stale-token
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test book-stale-token -- --test-threads=1
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-book-stale-token PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_BOOK_STALE_TOKEN_CLIENT=$<TARGET_FILE:functional-book-stale-token-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so;JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE=${MODULE_OAUTH2_SERVICES_LIBRARY}"
	)

	# docs/ROADMAP.md item 26: whether a source UID reachable only from
	# dconf/GSettings can drive the registry into a credential lookup and a
	# consent window, and — since it cannot — what actually produces the
	# operator's `Failed to lookup password for source <uid>` line for a UID
	# with no keyfile in the config directory. Uses the collection backend's
	# module because the answer is a collection *child*, whose keyfile EDS
	# writes to the cache directory instead.
	# `--test-threads=1`: the three tests each stand up a registry and a
	# keyring daemon of their own, and two of them a mock server as well.
	add_test(
		NAME functional-stale-source-uid
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test stale-source-uid -- --test-threads=1
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-stale-source-uid PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_STALE_SOURCE_UID_CLIENT=$<TARGET_FILE:functional-stale-source-uid-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so"
	)

	# docs/ROADMAP.md item 22 Do(1): the stale Source.OAuth2Support proxy that
	# turns a silent token fetch into G_DBUS_ERROR_SERVICE_UNKNOWN and then a
	# consent window. Reuses the collection backend's module (it is what
	# registers the "JMAP" EOAuth2Service the account's `[Authentication]
	# Method` names) and adds EDS's own oauth2-services module beside it.
	add_test(
		NAME functional-oauth2-stale-proxy
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test oauth2-stale-proxy
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-oauth2-stale-proxy PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_OAUTH2_STALE_PROXY_CLIENT=$<TARGET_FILE:functional-oauth2-stale-proxy-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so;JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE=${MODULE_OAUTH2_SERVICES_LIBRARY}"
	)

	# `gnome-keyring-daemon` is what `Session::run` (rust/crates/jmap-functional/
	# src/lib.rs) unlocks a login keyring on before every other test's client
	# runs — every account with an `[Authentication]` extension makes EDS ask a
	# `org.freedesktop.secrets` provider, per docs/ROADMAP.md item 18. Checked
	# here, loudly, for the same reason the three daemons above are: a missing
	# secret store fails a functional test with a bare D-Bus activation error
	# or timeout that says nothing about what is missing.
	find_program(GNOME_KEYRING_DAEMON_EXECUTABLE gnome-keyring-daemon
		PATHS /usr/bin /usr/libexec)
	if(NOT GNOME_KEYRING_DAEMON_EXECUTABLE)
		message(FATAL_ERROR
			"ENABLE_FUNCTIONAL_TESTS is ON but gnome-keyring-daemon was not "
			"found (Debian/Ubuntu: gnome-keyring). Every functional test's "
			"session needs a secret store to answer EDS's credential lookups; "
			"see docs/ROADMAP.md item 18.")
	endif()

	# The harness's own secret-store step, proven with no backend, module, or
	# `ESource` involved — see rust/crates/jmap-functional/tests/secret-store.rs.
	add_test(
		NAME functional-secret-store
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test secret-store
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-secret-store PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT "CARGO_INCREMENTAL=0"
	)

	# The other half of the secret-store story: not the harness's own unlock
	# step but the PRODUCT reading a real secret service, both what it asks
	# of one. Its lock state (`docs/ROADMAP.md` item 17(a)), and whether there
	# is a service to be had at all (item 31), which is a different question
	# with a different consequence: a locked store still holds the token, so
	# the user is told to unlock it, while an absent one cannot hold a token
	# at all, so no sign-in window is offered. Registered here rather than
	# left to
	# `rust-test-eds` because it needs `gnome-keyring-daemon` and
	# `dbus-run-session`, which only ci/install-deps-functional.sh installs —
	# hence the `#[ignore]` on every test in the file and the `--ignored`
	# here. `--test-threads=1`: each test starts a keyring daemon of its own,
	# and running them concurrently would have them race for the same
	# `gnome-keyring-daemon` control socket discovery.
	add_test(
		NAME functional-secret-store-lock
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-backend-core
			--test secret_store -- --ignored --test-threads=1
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-secret-store-lock PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT "CARGO_INCREMENTAL=0"
	)
endif()
