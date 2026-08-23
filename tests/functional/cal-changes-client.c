/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * M9 layer 1, calendar: the twin of `book-client.c`'s `list` phase, for
 * `rust/crates/jmap-functional/tests/calendar-changes.rs` — whether
 * `ECalMetaBackendClass::get_changes_sync` is reached by a real, running
 * `evolution-calendar-factory`, the calendar-side half of the same gap
 * `book-changes.rs` closed for the address book (see that file's own header
 * and NIGHT-LOG's "Delivered: EBookMetaBackendClass::get_changes_sync proven
 * through a real, restarted evolution-addressbook-factory").
 *
 * `cal-client.c` one file over never reaches `get_changes_sync` either — it
 * opens its calendar exactly once per process, so every connect there hits
 * `list_existing_sync` (a fresh meta-backend cache has no stored sync tag).
 * Rather than growing that file's already-large, phase-free `main` with a
 * mode argument its ten scenarios have no use for, this is a small, separate
 * client of its own: connect, list every event, print the summaries sorted —
 * the calendar analogue of `book-client.c`'s `list_phase`.
 *
 *   usage: functional-cal-changes-client <source-uid>
 */

#include <libecal/libecal.h>

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

	/* Activates evolution-source-registry on the session bus, which reads
	 * the scratch sources directory the harness wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* Activates evolution-calendar-factory, which is what dlopens
	 * libecalbackendjmap.so; see the same call in cal-client.c for why
	 * this does not wait for connected. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	/* Over the bus rather than out of the client's cached copy; see the
	 * same call in cal-client.c/book-client.c for why. */
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
