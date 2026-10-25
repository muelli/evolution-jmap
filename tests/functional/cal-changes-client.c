/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The calendar equivalent of the `list` phase in `book-client.c`, for
 * `rust/crates/jmap-functional/tests/calendar-changes.rs`. The test checks
 * that a real `evolution-calendar-factory` calls
 * `ECalMetaBackendClass::get_changes_sync`. `book-changes.rs` does the same
 * check for the address book.
 *
 * `cal-client.c` does not reach `get_changes_sync`. It opens its calendar
 * once per process, so each connect calls `list_existing_sync` (a new
 * meta-backend cache has no stored sync tag). That file has a large `main`
 * with no phases, thus this separate client connects, lists all events and
 * prints the sorted summaries, as `list_phase` in `book-client.c` does.
 *
 * With an event UID and a summary, it then sets the summary of that event
 * and saves it. `rust/crates/jmap-functional/tests/calendar-offline-edit.rs`
 * uses this edit while the server is out of reach.
 *
 * With `--save`, it writes the event as EDS gives it to `<path>`, as an editor
 * keeps it open. With `--modify-from`, it waits until a refresh replaces the
 * cached event, then sets the summary of the old copy in `<path>` and saves it.
 * `rust/crates/jmap-functional/tests/calendar-stale-edit.rs` uses the two runs.
 *
 *   usage: functional-cal-changes-client <source-uid> [<event-uid> <summary>]
 *          functional-cal-changes-client <source-uid> --save <event-uid> <path>
 *          functional-cal-changes-client <source-uid> --modify-from <path> <summary>
 */

#include <libecal/libecal.h>

#include "connection-status.h"
#include "test-util.h"

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	ECalClient *cal;
	GSList *components = NULL, *link;
	GList *summaries = NULL, *summary_link;
	guint index = 0;
	const gchar *source_uid;

	if (argc != 2 && argc != 4 && argc != 5) {
		g_printerr ("usage: %s <source-uid> [<event-uid> <summary>]\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];

	/* Starts evolution-source-registry on the session bus. It reads the
	 * temporary sources directory that the test wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* A real server needs a password, and this program has no GUI prompt.
	 * Store it where EDS looks for Basic auth, as `book-client.c` and
	 * `cal-live-client.c` do. Tests against the mock do not set it. */
	{
		const gchar *seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

		if (seed_password && *seed_password &&
		    !e_source_store_password_sync (source, seed_password, TRUE, NULL, &error))
			return functional_fail ("store-password", error);
	}

	/* Starts evolution-calendar-factory, which dlopens
	 * libecalbackendjmap.so. cal-client.c tells why this does not wait for
	 * connected. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return functional_fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	/* The EDS result of the connect. With the mock it is immediate. With a
	 * real server, authenticate_sync completes only when this thread's main
	 * context iterates. connection-status.c tells why. */
	functional_report_connection_status (source, 10);

	/* Read over D-Bus, not from the client cache. cal-client.c and
	 * book-client.c tell why. */
	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return functional_fail ("retrieve-properties", error);

	/* "#t" is the S-expression that matches every object. */
	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error))
		return functional_fail ("query", error);

	for (link = components; link; link = link->next) {
		ICalComponent *component = link->data;
		const gchar *summary = i_cal_component_get_summary (component);

		summaries = g_list_insert_sorted (summaries, g_strdup (summary ? summary : ""),
						  (GCompareFunc) g_strcmp0);
	}

	g_print ("events=%u\n", g_slist_length (components));
	for (summary_link = summaries; summary_link; summary_link = summary_link->next, index++)
		g_print ("event-%u=%s\n", index, (const gchar *) summary_link->data);

	g_list_free_full (summaries, g_free);
	g_slist_free_full (components, g_object_unref);

	if (argc == 5 && g_str_equal (argv[2], "--save")) {
		ICalComponent *event = NULL;
		gchar *text;

		if (!e_cal_client_get_object_sync (cal, argv[3], NULL, &event, NULL, &error))
			return functional_fail ("get", error);

		text = i_cal_component_as_ical_string (event);
		if (!g_file_set_contents (argv[4], text, -1, &error))
			return functional_fail ("save", error);
		g_free (text);
		g_object_unref (event);
	} else if (argc == 5 && g_str_equal (argv[2], "--modify-from")) {
		ICalComponent *event;
		gchar *old = NULL;
		gboolean replaced = FALSE;
		guint attempt;

		if (!g_file_get_contents (argv[3], &old, NULL, &error))
			return functional_fail ("load", error);
		event = i_cal_component_new_from_string (old);
		if (!event) {
			g_printerr ("load: '%s' is not an iCalendar component\n", argv[3]);
			return 1;
		}

		/* The connect starts a refresh. Wait until it replaces the cached event. */
		for (attempt = 0; attempt < 80 && !replaced; attempt++) {
			ICalComponent *cached = NULL;

			if (e_cal_client_get_object_sync (cal, i_cal_component_get_uid (event), NULL,
							  &cached, NULL, NULL)) {
				gchar *text = i_cal_component_as_ical_string (cached);

				replaced = g_strcmp0 (text, old) != 0;
				g_free (text);
				g_object_unref (cached);
			}
			if (!replaced)
				g_usleep (G_USEC_PER_SEC / 4);
		}
		if (!replaced) {
			g_printerr ("refresh: the cached event did not change\n");
			return 1;
		}

		i_cal_component_set_summary (event, argv[4]);
		if (!e_cal_client_modify_object_sync (cal, event, E_CAL_OBJ_MOD_ALL,
						      E_CAL_OPERATION_FLAG_NONE, NULL, &error)) {
			g_object_unref (event);
			return functional_fail ("modify", error);
		}
		g_object_unref (event);
		g_free (old);
	} else if (argc == 4) {
		ICalComponent *event = NULL;

		if (!e_cal_client_get_object_sync (cal, argv[2], NULL, &event, NULL, &error))
			return functional_fail ("get", error);

		i_cal_component_set_summary (event, argv[3]);
		if (!e_cal_client_modify_object_sync (cal, event, E_CAL_OBJ_MOD_ALL,
						      E_CAL_OPERATION_FLAG_NONE, NULL, &error)) {
			g_object_unref (event);
			return functional_fail ("modify", error);
		}
		g_object_unref (event);
	}

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
