# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The headless functional tests. They drive real Evolution
# Data Server (EDS) daemons against the in-repo mock JMAP server. This file
# requires cmake/Rust.cmake for CARGO_TARGET_DIR.
#
# The option is off by default. When it is on, each dependency is mandatory.
# These tests need the EDS runtime: evolution-source-registry and
# evolution-addressbook-factory from the `evolution-data-server` package,
# not the `-dev` packages that the other targets use. They also need a
# D-Bus implementation. The shared CI image has neither.
# A test that is always registered fails each CI run, or it skips itself
# and reports a pass on a machine where it did not run.
# The explicit option keeps "did not run" different from "passed". With
# -DENABLE_FUNCTIONAL_TESTS=ON, a missing runtime is a configure error.
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
	# The private session bus for each test. Without it, the tests use the
	# developer's own daemons, environment and Evolution data.
	find_program(DBUS_RUN_SESSION_EXECUTABLE dbus-run-session)
	if(NOT DBUS_RUN_SESSION_EXECUTABLE)
		message(FATAL_ERROR
			"ENABLE_FUNCTIONAL_TESTS is ON but dbus-run-session was not found "
			"(Debian/Ubuntu: dbus-daemon; Fedora: dbus-daemon)")
	endif()

	# The daemons. D-Bus activates them, so the tests do not use these paths.
	# But without them, activation fails with a bus error that does not name
	# the missing daemon. This check names it. libexecdir is not on PATH, and
	# no pkg-config module reports it.
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

	# The client programs: usual libebook and libecal consumers. No crate in
	# this repository binds these interfaces. See tests/functional/book-client.c.
	pkg_check_modules(LIBEBOOK REQUIRED libebook-1.2>=${REQUIRE_EVOLUTION_VERSION})
	pkg_check_modules(LIBECAL REQUIRED libecal-2.0>=${REQUIRE_EVOLUTION_VERSION})

	# Both clients compile connection-status.c. It asks if EDS thinks that the
	# backend is connected. The question and its main-loop steps are the same
	# for a book and a calendar.
	add_executable(functional-book-client
		tests/functional/book-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-book-client PRIVATE ${LIBEBOOK_INCLUDE_DIRS})
	target_compile_options(functional-book-client PRIVATE ${LIBEBOOK_CFLAGS_OTHER})
	target_link_libraries(functional-book-client PRIVATE ${LIBEBOOK_LIBRARIES})
	target_link_directories(functional-book-client PRIVATE ${LIBEBOOK_LIBRARY_DIRS})

	# Five calendar clients, because each client asks a different question:
	# - cal-client.c creates each event that it examines.
	# - cal-edit-client.c reads an event that the server already has. It saves
	#   back the members that have no iCalendar line.
	# - cal-zone-client.c asks which instant the zone of an event resolves to,
	#   before and after each of the two kinds of edit that a user can make.
	# - cal-color-client.c asks of the D2 colour push the question that D1 asks
	#   of create/delete: does the `source_changed` vfunc of a live backend
	#   instance run and reach the server? The pure decision function alone
	#   does not answer this.
	# - cal-free-busy-client.c asks the same question of the Track E Path A
	#   `get_free_busy_sync`.
	# See the header of each client.
	#
	# Only the third client takes a mode argument, because it asks its question
	# from both ends. `read` starts from a zone that only the server can name,
	# `create` from a zone that only the client can name. They are separate
	# runs, not phases of one run. In `create`, the zone is in the timezone
	# store of the calendar only because the client put it there. A run that
	# read a server zone first fills that store from a different source.
	#
	# event-start.c holds the one function that reports the instant of a start.
	# Thus a difference between two client answers is a difference in the
	# event, not in the measurement. All five clients compile it, so one loop is
	# sufficient. cal-client.c, cal-color-client.c and cal-free-busy-client.c
	# do not call it, and an unused function costs nothing.
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

	# The live-server client of the calendar test, the twin of
	# functional-book-client. It creates a single event only. It does not reuse
	# functional-cal-client, because that run also creates an all-day event, a
	# zoned event, a recurring series with three kinds of exception and a
	# split, and a second zoned recurring series. A real difference in any of
	# them fails the whole run, not only one field. See the client's header.
	add_executable(functional-cal-live-client
		tests/functional/cal-live-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-live-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-live-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-live-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-live-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	# The live-server attendees client, the twin of functional-cal-live-client.
	# It examines only if ORGANIZER/ATTENDEE survive a local create.
	# See tests/functional/cal-attendees-client.c.
	add_executable(functional-cal-attendees-client
		tests/functional/cal-attendees-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-attendees-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-attendees-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-attendees-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-attendees-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	# The mirror of functional-cal-attendees-client. That client writes through
	# EDS and reads the EDS cache back. This client writes nothing: it only
	# connects and lists. It checks if an event that a different client put
	# directly on the server, with JSCalendar-bis `calendarAddress`
	# participants, shows ORGANIZER/ATTENDEE when a new ECalClient gets it.
	# See tests/functional/cal-attendees-remote-client.c.
	add_executable(functional-cal-attendees-remote-client
		tests/functional/cal-attendees-remote-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-attendees-remote-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-attendees-remote-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-attendees-remote-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-attendees-remote-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	# The capabilities that tell Evolution that the server sends email to the
	# guests of a meeting, and if a create or remove tells the server to send.
	# See tests/functional/cal-scheduling-client.c.
	add_executable(functional-cal-scheduling-client
		tests/functional/cal-scheduling-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-scheduling-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-scheduling-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-scheduling-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-scheduling-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	# Components that arrive through receive_objects, for example from an
	# accepted invitation or an import. See tests/functional/cal-received-client.c.
	add_executable(functional-cal-received-client
		tests/functional/cal-received-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-received-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-received-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-received-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-received-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	# Dismisses a reminder in the same way as the Evolution reminder watcher.
	# See tests/functional/cal-discard-alarm-client.c.
	add_executable(functional-cal-discard-alarm-client
		tests/functional/cal-discard-alarm-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-discard-alarm-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-discard-alarm-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-discard-alarm-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-discard-alarm-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	# The same calls against a real server with two accounts. A create run and
	# a settle run are separate, so the test can read the guest side between
	# them. See tests/functional/cal-scheduling-live-client.c.
	add_executable(functional-cal-scheduling-live-client
		tests/functional/cal-scheduling-live-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-scheduling-live-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-scheduling-live-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-scheduling-live-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-scheduling-live-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	# The calendar equivalent of the `list` phase of `functional-book-client`,
	# for `get_changes_sync` coverage. See tests/functional/cal-changes-client.c.
	# It links connection-status.c for the same reason as cal-live-client.c:
	# the authenticate_sync of a real server does not complete until the main
	# context of this thread iterates. Here, only
	# `functional_report_connection_status` iterates it.
	add_executable(functional-cal-changes-client
		tests/functional/cal-changes-client.c
		tests/functional/connection-status.c)
	target_include_directories(functional-cal-changes-client PRIVATE ${LIBECAL_INCLUDE_DIRS})
	target_compile_options(functional-cal-changes-client PRIVATE ${LIBECAL_CFLAGS_OTHER})
	target_link_libraries(functional-cal-changes-client PRIVATE ${LIBECAL_LIBRARIES})
	target_link_directories(functional-cal-changes-client PRIVATE ${LIBECAL_LIBRARY_DIRS})

	# The collection client opens no book or calendar and connects to no
	# factory. It connects only to the registry: the daemon that loads
	# `module-jmap-backend.so` (`EDS_REGISTRY_MODULES`, staged by
	# `Session::stage_collection_backend`) and does the populate/fan-out that
	# this test examines. `libedataserver` alone is sufficient. The mail and
	# transport clients below link the same library for their registry lookups.
	pkg_check_modules(LIBEDATASERVER REQUIRED libedataserver-1.2>=${REQUIRE_EVOLUTION_VERSION})
	add_executable(functional-collection-client tests/functional/collection-client.c)
	target_include_directories(functional-collection-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-collection-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-collection-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-collection-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# The same consumer. It waits for the children that a share from a
	# different account adds, and for their removal when the share is
	# withdrawn. Their `[Resource] Identity` names them. See the client's header.
	add_executable(functional-collection-shared-client tests/functional/collection-shared-client.c)
	target_include_directories(functional-collection-shared-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-collection-shared-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-collection-shared-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-collection-shared-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# The live-edit reproduction client. It uses the same registry, but it
	# edits the port of the account and commits it with e_source_write_sync.
	# It answers if the "changed" handler of `source_changed` makes the next
	# populate of a broken account succeed. See the client's header.
	add_executable(functional-collection-edit-client tests/functional/collection-edit-client.c)
	target_include_directories(functional-collection-edit-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-collection-edit-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-collection-edit-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-collection-edit-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# The write half of the same interface: "New Address Book"/"Delete", that
	# is `e_source_remote_create_sync`/`e_source_remote_delete_sync` against the
	# same registry. It proves `create_resource_sync`/`delete_resource_sync`,
	# not populate/fan-out. It is a separate client, not a mode of the client
	# above, for the same reason as the cal-client family: each client answers
	# one question against the registry.
	add_executable(functional-collection-create-client tests/functional/collection-create-client.c)
	target_include_directories(functional-collection-create-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-collection-create-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-collection-create-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-collection-create-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# The calendar equivalent of the client above: the same create/delete pair
	# against E_SOURCE_EXTENSION_CALENDAR, not the address-book extension. It
	# proves that create_resource_sync/delete_resource_sync do not mix the two
	# kinds of child.
	add_executable(functional-collection-create-calendar-client tests/functional/collection-create-calendar-client.c)
	target_include_directories(functional-collection-create-calendar-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-collection-create-calendar-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-collection-create-calendar-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-collection-create-calendar-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# A registry-only consumer like the collection clients above, but it acts
	# as a long-lived backend factory. It keeps an ESource across a registry
	# restart, then asks it for an OAuth 2.0 access token. It needs only
	# libedataserver and the platform kill(2). See the client's header for the
	# EDS source code that it relies on.
	add_executable(functional-oauth2-stale-proxy-client tests/functional/oauth2-stale-proxy-client.c)
	target_include_directories(functional-oauth2-stale-proxy-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-oauth2-stale-proxy-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-oauth2-stale-proxy-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-oauth2-stale-proxy-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# A registry-only consumer that runs twice, against two accounts that
	# share an [Authentication] User but use two different mock JMAP
	# deployments. It reproduces the secret-store slot collision that
	# `eos_generate_secret_uid` causes, because that function keys only on
	# (service name, user). See the client's header.
	add_executable(functional-oauth2-token-collision-client tests/functional/oauth2-token-collision-client.c)
	target_include_directories(functional-oauth2-token-collision-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-oauth2-token-collision-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-oauth2-token-collision-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-oauth2-token-collision-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# A registry module of EDS itself, not of this project.
	# `EDS_REGISTRY_MODULES` replaces the EDS module directory and does not add
	# to it (`e-source-registry-server.c:1073`). Thus a functional session sees
	# only the modules that it stages. The stale-proxy test needs this module:
	# its `EOAuth2SourceMonitor` exports the `Source.OAuth2Support` D-Bus
	# interface for an account whose `[Authentication] Method` names a
	# registered `EOAuth2Service`. A missing module is fatal, as everywhere in
	# this file. Without it, the test still runs against a registry, but the
	# interface that it examines does not exist, so it measures nothing.
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
			"evolution-data-server runtime package; the functional harness "
			"stages it beside module-jmap-backend.so because the OAuth2 "
			"credential tests need EDS's own OAuth2Support object to exist, "
			"and EDS_REGISTRY_MODULES replaces the module directory rather "
			"than extending it, so both modules must be present at once.")
	endif()

	# The calendar client of the stale-token-refresh acceptance test. This
	# libecal consumer does two things that the other calendar clients do not:
	# it seeds the secret store (`e_secret_store_store_sync`) before it
	# connects, and it asks if the registry exported `Source.OAuth2Support` for
	# its source (`e_source_ref_dbus_object`). Both functions are in
	# libedataserver. Thus this target names libedataserver explicitly, not as
	# a transitive dependency of libecal. For the same reason, this target
	# comes after `pkg_check_modules(LIBEDATASERVER ...)`.
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

	# The address-book client of the same acceptance test. It has the same
	# structure as functional-cal-stale-token-client, with libebook in place of
	# libecal. See the comment of that target and the client's header for why
	# it names libedataserver.
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

	# A 401 that arrives only after a redirect removed the Authorization header
	# must not cause interactive consent. This libebook consumer needs
	# libedataserver for e_source_store_password_sync, like the stale-token
	# client above. It does not link connection-status.c: a failed connect does
	# not reach `connected`, and this test does not wait for it.
	add_executable(functional-redirect-credentials-withheld-client
		tests/functional/redirect-credentials-withheld-client.c)
	target_include_directories(functional-redirect-credentials-withheld-client PRIVATE
		${LIBEBOOK_INCLUDE_DIRS} ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-redirect-credentials-withheld-client PRIVATE
		${LIBEBOOK_CFLAGS_OTHER} ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-redirect-credentials-withheld-client PRIVATE
		${LIBEBOOK_LIBRARIES} ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-redirect-credentials-withheld-client PRIVATE
		${LIBEBOOK_LIBRARY_DIRS} ${LIBEDATASERVER_LIBRARY_DIRS})

	# The stale-source-uid reproduction client. It is a registry-only
	# libedataserver consumer like functional-collection-client, and it also
	# uses GSettings. GSettings is not a separate dependency: `gio-2.0` comes
	# with the pkg-config data of libedataserver. With `g_settings_new`, the
	# client itself plants the dconf debris of the operator, so the Rust test
	# does not edit a GVDB file by hand.
	add_executable(functional-stale-source-uid-client tests/functional/stale-source-uid-client.c)
	target_include_directories(functional-stale-source-uid-client PRIVATE ${LIBEDATASERVER_INCLUDE_DIRS})
	target_compile_options(functional-stale-source-uid-client PRIVATE ${LIBEDATASERVER_CFLAGS_OTHER})
	target_link_libraries(functional-stale-source-uid-client PRIVATE ${LIBEDATASERVER_LIBRARIES})
	target_link_directories(functional-stale-source-uid-client PRIVATE ${LIBEDATASERVER_LIBRARY_DIRS})

	# The config-lookup client opens no `.source` keyfile, because a lookup
	# occurs before an account exists. It links evolution-shell-3.0, not a
	# libe* client library, because EConfigLookup is in e-util (the Evolution
	# library, not an EDS library). The top-level CMakeLists.txt always
	# requires evolution-shell-3.0 for module-jmap-configuration.so, so the
	# EVOLUTION_SHELL variables are already set here.
	add_executable(functional-config-lookup-client tests/functional/config-lookup-client.c)
	target_include_directories(functional-config-lookup-client PRIVATE ${EVOLUTION_SHELL_INCLUDE_DIRS})
	target_compile_options(functional-config-lookup-client PRIVATE ${EVOLUTION_SHELL_CFLAGS_OTHER})
	target_link_libraries(functional-config-lookup-client PRIVATE ${EVOLUTION_SHELL_LIBRARIES})
	target_link_directories(functional-config-lookup-client PRIVATE ${EVOLUTION_SHELL_LIBRARY_DIRS})

	# The mail client links no client library. There is no libecamel that
	# matches libebook and libecal, because Evolution loads a Camel provider
	# into its own process. This program is that process. Thus it links camel,
	# and libedataserver for the ESourceRegistry and the ESourceCamel code that
	# changes the keyfile settings into a configured CamelService.
	#
	# No connection-status.c: that is an EClient concept, and a CamelService
	# reports its connection status synchronously.
	pkg_check_modules(CAMEL_CLIENT REQUIRED camel-1.2>=${REQUIRE_EVOLUTION_VERSION})
	pkg_check_modules(LIBEDATASERVER REQUIRED libedataserver-1.2>=${REQUIRE_EVOLUTION_VERSION})

	# The send client is a separate program, not a mode of the receive client.
	# It opens no store, and its path (account to identity to transport,
	# through two uids) has nothing in common with opening a folder. It links
	# the same libraries, because it is the same kind of process: a libcamel
	# consumer that also hosts the provider.
	#
	# `mail-stale-token` is a third program of the same kind. It subclasses
	# `CamelSession`, because it needs a session that answers
	# `get_oauth2_access_token_sync`, and the base `CamelSession` does not.
	# See the client's header.
	#
	# `mail-live` is the receive client against a real server, not the mock.
	# It also subclasses `CamelSession`, for a related reason: a real server
	# requires a password, and only a session can give one to a `CamelService`.
	#
	# `transport-live` is the send client against a real server. It combines
	# the chain walk of transport-client.c, the password procedure of
	# mail-live, and a delivery check into a second account. The mock test has
	# no equivalent of that check.
	foreach(_client mail transport mail-stale-token mail-live transport-live)
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

	# The Rust side builds the scratch EDS installation, runs the client on a
	# private bus and checks what both ends report. Each test has its own
	# registration (`cargo test --test <name>`, not one run of the whole crate).
	# Thus a failure names the interface that broke, and each test gets only
	# the paths that it needs. Only CMake knows these paths: the client that
	# CMake built, and the backend that cargo built, which the test stages as
	# the one module of that factory.
	#
	# All these tests share a cargo target directory, so the cargo lock
	# serialises them in each CTest schedule.
	add_test(
		NAME functional-book
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test address-book
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-book PROPERTIES
		LABELS functional
		# A large limit, because it includes the activation of two daemons and a
		# first connect. The limit makes a blocked factory fail the run, not hang it.
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_BOOK_CLIENT=$<TARGET_FILE:functional-book-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so"
	)

	# Uses the binary and module of `functional-book-client` (the `list` phase
	# is a mode of that program). The new coverage is in the Rust test: two
	# separate connects that share one on-disk cache. They reach
	# `get_changes_sync` instead of `list_existing_sync` again.
	# See `tests/book-changes.rs`.
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

	# Three connects on one cache: list, edit offline, replay. EDS gives the
	# extra of the cached contact back on the replay as the base of the edit.
	# See `tests/book-offline-edit.rs`.
	add_test(
		NAME functional-book-offline-edit
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test book-offline-edit
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-book-offline-edit PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_BOOK_CLIENT=$<TARGET_FILE:functional-book-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so"
	)

	# The same `functional-book-client` binary and module, against a real
	# Stalwart instead of the in-process mock. The Rust source marks it
	# `#[ignore]`: it needs `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/
	# `_WRITE_PASSWORD`, which the operator sets by hand from
	# `live-server-env.sh` and a temporary `stw seed` account (see
	# docs/manual-test-live-server.md). Thus this registration reports
	# "ignored" in a usual functional run. It exists so that the CTest rule
	# "did not run" versus "ran" applies here too, as for each test in this file.
	add_test(
		NAME functional-book-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-book
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-book-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_BOOK_CLIENT=$<TARGET_FILE:functional-book-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so"
	)

	# The real-EDS test of the book-sync scale checkpoint. It seeds a
	# real-sized book directly over JMAP before EDS opens it, then measures the
	# first open by evolution-addressbook-factory. `#[ignore]` for the same
	# reason as functional-book-live-stalwart, so it reports "ignored" in a
	# usual functional run. The timeout is large: at the batch sizes for
	# manual runs, the seed step alone can take minutes.
	add_test(
		NAME functional-book-scale-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-book-scale
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-book-scale-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 1800
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_BOOK_CLIENT=$<TARGET_FILE:functional-book-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so"
	)

	# The two-connects-one-cache test of `functional-book-changes`, against a
	# real Stalwart instead of the in-process mock.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-book-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-book-changes-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-book-changes
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-book-changes-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_BOOK_CLIENT=$<TARGET_FILE:functional-book-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so"
	)

	# functional-cal-live-client against a real Stalwart instead of the
	# in-process mock.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-book-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-cal-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-calendar
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_LIVE_CLIENT=$<TARGET_FILE:functional-cal-live-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# functional-cal-attendees-client against a real Stalwart.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-cal-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-cal-attendees-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-calendar-attendees
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-attendees-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_ATTENDEES_CLIENT=$<TARGET_FILE:functional-cal-attendees-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# functional-cal-attendees-remote-client against a real Stalwart.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-cal-attendees-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-cal-attendees-remote-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-calendar-attendees-remote
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-attendees-remote-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_ATTENDEES_REMOTE_CLIENT=$<TARGET_FILE:functional-cal-attendees-remote-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# functional-cal-scheduling-live-client against a real Stalwart with two
	# accounts.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-cal-attendees-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-cal-scheduling-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-calendar-scheduling
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-scheduling-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_SCHEDULING_LIVE_CLIENT=$<TARGET_FILE:functional-cal-scheduling-live-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# The real-EDS test of the cal-sync scale checkpoint, the calendar twin of
	# functional-book-scale-live-stalwart. It seeds a real-sized calendar
	# directly over JMAP before EDS opens it, then measures the first open by
	# evolution-calendar-factory. It reuses the connect-and-list of
	# functional-cal-changes-client without changes. `#[ignore]` for the same
	# reason as functional-book-scale-live-stalwart, so it reports "ignored" in
	# a usual functional run. The timeout is large: at the batch sizes for
	# manual runs, the seed step alone can take minutes.
	add_test(
		NAME functional-cal-scale-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-calendar-scale
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-scale-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 1800
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT=$<TARGET_FILE:functional-cal-changes-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
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

	# The calendar twin of `functional-book-changes`: two connects that share
	# one on-disk cache reach `get_changes_sync` instead of
	# `list_existing_sync` again. See `tests/calendar-changes.rs`.
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

	# The two-connects-one-cache test of `functional-cal-changes`, against a
	# real Stalwart instead of the in-process mock.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-book-changes-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-cal-changes-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-calendar-changes
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-changes-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT=$<TARGET_FILE:functional-cal-changes-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# The D2 write half through a real, running backend instance: a colour edit
	# on the `ESourceSelectable` of the calendar. The test checks if the running
	# `source_changed` vfunc pushes the edit, not only if the pure decision
	# function (`jmap-backend-cal/tests/ops.rs`) pushes it.
	add_test(
		NAME functional-cal-color
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test calendar-color
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-color PROPERTIES
		LABELS functional
		# A large limit for the usual reason, plus the intentional
		# PUSH_SETTLE_SECONDS wait in the client.
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_COLOR_CLIENT=$<TARGET_FILE:functional-cal-color-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# Through a real, running backend instance: the scheduling capabilities
	# that a libecal consumer sees, with the account switch at its default and
	# off, and which saves and removals tell the mock to send iTIP.
	add_test(
		NAME functional-cal-scheduling
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test calendar-scheduling
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-scheduling PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_SCHEDULING_CLIENT=$<TARGET_FILE:functional-cal-scheduling-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# Received components, through a real, running backend instance: the
	# server sends no invitation in the user's name for them, and they cannot
	# change a meeting that the user organizes.
	add_test(
		NAME functional-cal-received
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test calendar-received
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-received PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_RECEIVED_CLIENT=$<TARGET_FILE:functional-cal-received-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# A dismissed reminder, through a real, running backend instance:
	# `discard_alarm_sync` writes ACKNOWLEDGED on the VALARM, and the mock
	# stores `acknowledged` on the alert. For a single event and for an
	# occurrence of a series.
	add_test(
		NAME functional-cal-discard-alarm
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test calendar-discard-alarm
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-discard-alarm PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_DISCARD_ALARM_CLIENT=$<TARGET_FILE:functional-cal-discard-alarm-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# Track E Path A `get_free_busy_sync` through a real, running backend
	# instance: a free/busy query for a seeded principal. The test checks if
	# the running vfunc sends it through `Principal/query` and
	# `Principal/getAvailability`, not only if the pure decision function
	# (`jmap-backend-cal/tests/ops.rs`) does.
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

	# The `get_free_busy_sync` test of `functional-cal-free-busy`, against a
	# real Stalwart instead of the in-process mock.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-cal-changes-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-cal-free-busy-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-calendar-free-busy
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-cal-free-busy-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CAL_FREE_BUSY_CLIENT=$<TARGET_FILE:functional-cal-free-busy-client>;JMAP_FUNCTIONAL_CAL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_cal_module.so"
	)

	# The populate/fan-out of the collection backend. A real
	# evolution-source-registry loads module-jmap-backend.so for the account
	# keyfile that the Rust side writes. The client waits until the
	# address-book and calendar children appear through the registry API, as
	# in the procedure of docs/manual-test-collection-backend.md.
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

	# Does an edit of the host (here, the port) of a broken, live account cause
	# a new authenticate, as the "changed" handler of `source_changed.rs` intends?
	add_test(
		NAME functional-collection-edit-host
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test collection-edit-host
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-collection-edit-host PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_COLLECTION_EDIT_CLIENT=$<TARGET_FILE:functional-collection-edit-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so"
	)

	# The same functional-collection-client binary and module, against a real
	# Stalwart instead of the in-process mock.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-book-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-collection-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-collection
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-collection-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_COLLECTION_CLIENT=$<TARGET_FILE:functional-collection-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so"
	)

	# Calendars and address books that a different real account shares with
	# the user. They appear as children of the user account through a real
	# registry, and they disappear when the share is withdrawn. Two Stalwart
	# accounts, so `#[ignore]` like the test above.
	add_test(
		NAME functional-collection-shared-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-collection-shared
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-collection-shared-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_COLLECTION_SHARED_CLIENT=$<TARGET_FILE:functional-collection-shared-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so"
	)

	# The write half: "New Address Book"/"Delete" through the same registry.
	# It proves create_resource_sync/delete_resource_sync end to end.
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

	# The calendar test of the same write half: "New Calendar"/"Delete" against
	# E_SOURCE_EXTENSION_CALENDAR. It proves that create_resource_sync/
	# delete_resource_sync select the correct `/set` call for the child kind.
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

	# module-jmap-configuration.so is not one of the four EDS/Camel backends.
	# The client loads it itself (see config-lookup-client.c), so it needs the
	# path of the built module, not the module-directory variable of a daemon.
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

	# The discovery test of `functional-config-lookup`, against a real
	# Stalwart instead of the in-process mock.
	# `#[ignore]` in the Rust source for the same reason as
	# functional-book-changes-live-stalwart, so it reports
	# "ignored" in a usual functional run.
	add_test(
		NAME functional-config-lookup-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-config-lookup
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-config-lookup-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_CONFIG_LOOKUP_CLIENT=$<TARGET_FILE:functional-config-lookup-client>;JMAP_FUNCTIONAL_CONFIG_LOOKUP_MODULE=${CARGO_TARGET_DIR}/release/libjmap_config_module.so"
	)

	# The mail test needs a third path that the book and calendar tests do not:
	# the `.urls` file. Camel opens the module only through this file, so the
	# test stages it from the source tree and does not write it.
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
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_MAIL_CLIENT=$<TARGET_FILE:functional-mail-client>;JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT=$<TARGET_FILE:functional-mail-live-client>;JMAP_FUNCTIONAL_MAIL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_mail.so;JMAP_FUNCTIONAL_MAIL_URLS=${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls"
	)

	# functional-mail-live-client against a real Stalwart instead of the
	# in-process mock. It stages the same module and `.urls` file as
	# functional-mail, with the same two variable names, for the reason that
	# functional-transport gives: it is one provider. `#[ignore]` in the Rust
	# source like the other live-server tests, so it reports "ignored" in a
	# usual functional run. The send client, functional-transport-live-client,
	# is in the same test file. One registration, because the five tests share
	# one temporary account and serialise on one mutex.
	add_test(
		NAME functional-mail-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-mail
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-mail-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT=$<TARGET_FILE:functional-mail-live-client>;JMAP_FUNCTIONAL_TRANSPORT_LIVE_CLIENT=$<TARGET_FILE:functional-transport-live-client>;JMAP_FUNCTIONAL_MAIL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_mail.so;JMAP_FUNCTIONAL_MAIL_URLS=${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls"
	)

	# The mail twin of functional-book-scale-live-stalwart and
	# functional-cal-scale-live-stalwart. It seeds a real-sized inbox directly
	# over JMAP, then measures the first open by the Camel provider through the
	# `scale` phase of functional-mail-live-client. It has its own registration,
	# separate from functional-mail-live-stalwart, because it needs its own
	# temporary account: it does not delete the messages that it seeds.
	# `#[ignore]` like the other live-server tests, with a large timeout for
	# the seed step.
	add_test(
		NAME functional-mail-scale-live-stalwart
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test live-stalwart-mail-scale
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-mail-scale-live-stalwart PROPERTIES
		LABELS functional
		TIMEOUT 1800
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT=$<TARGET_FILE:functional-mail-live-client>;JMAP_FUNCTIONAL_MAIL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_mail.so;JMAP_FUNCTIONAL_MAIL_URLS=${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls"
	)

	# The send test stages the same module and `.urls` file with the same two
	# variable names, because it is one provider. A test with its own module
	# can pass against a build that the receive test did not examine.
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
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_TRANSPORT_CLIENT=$<TARGET_FILE:functional-transport-client>;JMAP_FUNCTIONAL_TRANSPORT_LIVE_CLIENT=$<TARGET_FILE:functional-transport-live-client>;JMAP_FUNCTIONAL_MAIL_MODULE=${CARGO_TARGET_DIR}/release/libjmap_mail.so;JMAP_FUNCTIONAL_MAIL_URLS=${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls"
	)

	# The mail test of the hourly-re-consent acceptance test, without a GUI.
	# A real `CamelService` drives it, not an operator who keeps Evolution open
	# for an afternoon. It stages the same module and `.urls` file as the mail
	# tests above, with the same two variable names, for the same reason: it is
	# one provider.
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

	# The calendar test of the same acceptance test as
	# `functional-mail-stale-token`, for the backend that refreshes through an
	# `ESource`, not a `CamelSession`. It needs three modules, but each other
	# calendar test needs one: the calendar backend in the factory, and in the
	# registry `module-jmap-backend.so` (the "JMAP" EOAuth2Service and the
	# `[JMAP OAuth2]` extension type) and the EDS oauth2-services module (it
	# exports `Source.OAuth2Support`). See the test's header for why each
	# module is necessary.
	# `--test-threads=1`: each of the two tests starts its own registry,
	# factory and keyring daemon.
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

	# The address-book test of the same acceptance test as
	# `functional-cal-stale-token`, for the `with_connection`/
	# `retry_on_authentication_failure` wiring of `jmap-backend-book`. The same
	# three modules for the same reason, with the libebook factory and backend
	# in place of the libecal ones.
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

	# Does a real `EBookMetaBackend` connect through a cross-origin redirect
	# that removes credentials fail cleanly? It must not escalate to
	# interactive consent or retry repeatedly, unlike the endless OAuth2 popup
	# loop that the operator saw on 0.5.0. No OAuth2 modules: the account uses
	# plain Basic authentication, so only the book backend is staged.
	add_test(
		NAME functional-redirect-credentials-withheld
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test redirect-credentials-withheld
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-redirect-credentials-withheld PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_REDIRECT_CREDENTIALS_WITHHELD_CLIENT=$<TARGET_FILE:functional-redirect-credentials-withheld-client>;JMAP_FUNCTIONAL_BOOK_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_book_module.so"
	)

	# Checks if a source UID that only dconf/GSettings contains can make the
	# registry do a credential lookup and open a consent window. It cannot, so
	# the test also finds what causes the operator's `Failed to lookup password
	# for source <uid>` line for a UID with no keyfile in the config directory.
	# It uses the module of the collection backend because the cause is a
	# collection child, and EDS writes its keyfile to the cache directory.
	# `--test-threads=1`: each of the three tests starts its own registry and
	# keyring daemon, and two of them also start a mock server.
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

	# The stale Source.OAuth2Support proxy that changes a silent token fetch
	# into G_DBUS_ERROR_SERVICE_UNKNOWN and then a consent window. It reuses
	# the module of the collection backend (it registers the "JMAP"
	# EOAuth2Service that the `[Authentication] Method` of the account names)
	# and adds the EDS oauth2-services module.
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

	# Two accounts that share one [Authentication] User and use two
	# independent mock JMAP deployments collide on one secret-store slot.
	add_test(
		NAME functional-oauth2-token-collision
		COMMAND ${CARGO_EXECUTABLE} test --locked -p jmap-functional
			--test oauth2-token-collision
		WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}/rust"
	)
	set_tests_properties(functional-oauth2-token-collision PROPERTIES
		LABELS functional
		TIMEOUT 300
		ENVIRONMENT
			"CARGO_INCREMENTAL=0;JMAP_FUNCTIONAL_OAUTH2_TOKEN_COLLISION_CLIENT=$<TARGET_FILE:functional-oauth2-token-collision-client>;JMAP_FUNCTIONAL_COLLECTION_MODULE=${CARGO_TARGET_DIR}/release/libjmap_backend_collection_module.so;JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE=${MODULE_OAUTH2_SERVICES_LIBRARY}"
	)

	# `Session::run` (rust/crates/jmap-functional/src/lib.rs) unlocks a login
	# keyring on `gnome-keyring-daemon` before the client of each other test
	# runs. For each account with an `[Authentication]` extension, EDS asks an
	# `org.freedesktop.secrets` provider. This check is fatal for the same
	# reason as the three daemon checks above: without a secret store, a
	# functional test fails with a D-Bus activation error or a timeout that
	# does not name the missing program.
	find_program(GNOME_KEYRING_DAEMON_EXECUTABLE gnome-keyring-daemon
		PATHS /usr/bin /usr/libexec)
	if(NOT GNOME_KEYRING_DAEMON_EXECUTABLE)
		message(FATAL_ERROR
			"ENABLE_FUNCTIONAL_TESTS is ON but gnome-keyring-daemon was not "
			"found (Debian/Ubuntu: gnome-keyring). Every functional test's "
			"session needs a secret store: EDS D-Bus-activates "
			"org.freedesktop.secrets on the first credential lookup, and "
			"without a running keyring daemon to answer that activation, "
			"the lookup hangs until it times out.")
	endif()

	# The secret-store step of the jmap-functional crate, with no backend,
	# module or `ESource`. See rust/crates/jmap-functional/tests/secret-store.rs.
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

	# The other half of the secret-store tests: not the unlock step of the
	# jmap-functional crate, but the product that reads a real secret service.
	# It examines two things: the lock state, and if a service exists at all.
	# The consequences differ. A locked store still holds the token, so the
	# product tells the user to unlock it. An absent store cannot hold a
	# token, so the product shows no sign-in window.
	# It is registered here, not in `rust-test-eds`, because it needs
	# `gnome-keyring-daemon` and `dbus-run-session`, and only
	# ci/install-deps-functional.sh installs them. Thus each test in the file
	# has `#[ignore]`, and this command adds `--ignored`. `--test-threads=1`:
	# each test starts its own keyring daemon, and concurrent tests race for
	# the same `gnome-keyring-daemon` control socket discovery.
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
