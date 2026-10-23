/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The write path of the calendar colour, tested through a real, running
 * `evolution-calendar-factory`. The in-process tests in
 * `jmap-backend-cal/tests/{backend,ops}.rs` do not go this far.
 *
 * This program is an ordinary libecal consumer. It opens a calendar, which
 * makes the factory dlopen libecalbackendjmap.so and keeps a live
 * `ECalMetaBackend` instance that monitors this source. Then it changes the
 * `ESourceSelectable` "Color" as the colour picker of the calendar-properties
 * dialog does: `e_source_selectable_set_color()`, then
 * `e_source_write_sync()`. It knows nothing about JMAP or the mock.
 *
 * The push to the server is the `source_changed` vfunc of the backend. It
 * reacts to the "changed" signal of the `ESource` on its own worker thread,
 * on the other side of the registry from this process.
 *
 * This client cannot observe the push: it is a one-way write to the server.
 * The EDS doc comment for `ECalMetaBackendClass::source_changed` says that it
 * runs "from a dedicated thread", not synchronously with
 * `e_source_write_sync()`. Thus this program waits a fixed time before it
 * exits, and does not poll. The daemons stop with the private D-Bus session
 * when this process exits, so a poll from the Rust side after that is not
 * possible.
 *
 * `rust/crates/jmap-functional/tests/calendar-color.rs` owns the scratch XDG
 * tree, the `.source` keyfile, the private D-Bus session, the mock server and
 * all assertions. It runs this program and reads its output. This program
 * has no test framework and does not know the correct result. It writes each
 * EDS observation to stdout as one `key=value` line, and exits non-zero when a
 * call fails.
 *
 *   usage: functional-cal-color-client <source-uid> <color>
 */

#include <libecal/libecal.h>

#include "connection-status.h"
#include "test-util.h"

/* The time that the asynchronous push of the backend gets to reach the mock.
 * When this process exits, its private bus and the backend instance stop.
 * The value is large because it costs time only on a passing run, and the
 * push is one local HTTP round trip to an in-process mock. */
#define PUSH_SETTLE_SECONDS 3

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	ESourceSelectable *selectable;
	const gchar *source_uid;
	const gchar *color;
	const gchar *initial;

	if (argc != 3) {
		g_printerr ("usage: %s <source-uid> <color>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	color = argv[2];

	/* Start evolution-source-registry on the session bus. It reads the
	 * scratch sources directory that the harness wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* Start evolution-calendar-factory and keep a live backend instance until
	 * this process exits. `source_changed` fires only on a backend that is
	 * connected and monitors this `ESource`. (guint32) -1 is the EDS value
	 * for "do not wait for connected"; `functional_report_connection_status`
	 * below does the wait. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return functional_fail ("connect", error);

	functional_report_connection_status (source, 10);

	if (!e_source_has_extension (source, E_SOURCE_EXTENSION_CALENDAR)) {
		g_printerr ("source carries no Calendar extension\n");
		return 1;
	}
	selectable = e_source_get_extension (source, E_SOURCE_EXTENSION_CALENDAR);

	/* The EDS GParamSpec gives `ESourceSelectable:color` the default
	 * "#62a0ea", not NULL. This value is only a diagnostic for the Rust side. */
	initial = e_source_selectable_get_color (selectable);
	g_print ("initial-color=%s\n", initial ? initial : "");

	/* The edit of the colour picker in the calendar-properties dialog: change
	 * the property, then write the source. The read path is the colour write
	 * in `jmap-backend-collection/src/child_source.rs::apply`. */
	e_source_selectable_set_color (selectable, color);
	if (!e_source_write_sync (source, NULL, &error))
		return functional_fail ("write", error);
	g_print ("written=1\n");

	/* See PUSH_SETTLE_SECONDS. */
	g_usleep (PUSH_SETTLE_SECONDS * G_USEC_PER_SEC);

	g_print ("done=1\n");

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
