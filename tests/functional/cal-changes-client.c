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
 *   usage: functional-cal-changes-client <source-uid>
 */

#include <libecal/libecal.h>

#include "connection-status.h"

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

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

	if (argc != 2) {
		g_printerr ("usage: %s <source-uid>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];

	/* Starts evolution-source-registry on the session bus. It reads the
	 * temporary sources directory that the test wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

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
			return fail ("store-password", error);
	}

	/* Starts evolution-calendar-factory, which dlopens
	 * libecalbackendjmap.so. cal-client.c tells why this does not wait for
	 * connected. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	/* The EDS result of the connect. With the mock it is immediate. With a
	 * real server, authenticate_sync completes only when this thread's main
	 * context iterates. connection-status.c tells why. */
	functional_report_connection_status (source, 10);

	/* Read over D-Bus, not from the client cache. cal-client.c and
	 * book-client.c tell why. */
	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return fail ("retrieve-properties", error);

	/* "#t" is the S-expression that matches every object. */
	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error))
		return fail ("query", error);

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
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
