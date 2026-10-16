/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * ROADMAP item 108, part (3): live confirmation that an event another client
 * created directly on the server, with JSCalendar-bis `calendarAddress`
 * participants, shows its ORGANIZER/ATTENDEE once a fresh `ECalClient`
 * connects and pulls it.
 *
 * The mirror of `cal-attendees-client.c`: that file writes through EDS and
 * reads EDS's own cache straight back; this one never writes anything —
 * the event already exists on the server, seeded by a raw `jmap_client`
 * before this program ever runs — and only connects and lists. A fresh
 * meta-backend cache's first connect is `list_existing_sync`
 * (`cal-changes-client.c`'s own header explains why), so no explicit
 * refresh is needed to pick up a write nothing here performed.
 *
 * `jmap_ical::event::drawn_participants` is the code path being confirmed:
 * it turns a `CalendarEvent.participants` JSON map into ORGANIZER/ATTENDEE
 * lines via `calendar_address`'s `calendarAddress`-with-`sendTo`-fallback
 * read. If that path is right, the component this program reads back
 * states both, despite EDS never having written either itself.
 *
 * Everything around this file — the scratch XDG tree, the `.source`
 * keyfile, the private D-Bus session, the proxy to the real server and
 * every assertion — belongs to `rust/crates/jmap-functional/tests/
 * live-stalwart-calendar-attendees-remote.rs`, which seeds the event over a
 * raw JMAP connection, runs this program and reads its output.
 *
 *   usage: functional-cal-attendees-remote-client <source-uid> <summary>
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

/* Every value a component states for one property, joined by commas, or the
 * empty string if it states none -- mirrors `cal-attendees-client.c`'s own
 * helper of the same name. */
static gchar *
joined_values (ICalComponent *component,
	       ICalPropertyKind kind)
{
	GString *values = g_string_new (NULL);
	ICalProperty *property;

	property = i_cal_component_get_first_property (component, kind);
	while (property) {
		ICalProperty *next;
		gchar *value = i_cal_property_get_value_as_string (property);

		if (values->len)
			g_string_append_c (values, ',');
		g_string_append (values, value ? value : "");
		g_free (value);

		next = i_cal_component_get_next_property (component, kind);
		g_object_unref (property);
		property = next;
	}

	return g_string_free (values, FALSE);
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
	ICalComponent *found = NULL;
	gchar *organizer;
	gchar *attendees;
	const gchar *source_uid;
	const gchar *summary;

	if (argc != 3) {
		g_printerr ("usage: %s <source-uid> <summary>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	summary = argv[2];

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	{
		const gchar *seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

		if (seed_password && *seed_password &&
		    !e_source_store_password_sync (source, seed_password, TRUE, NULL, &error))
			return fail ("store-password", error);
	}

	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	functional_report_connection_status (source, 10);

	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return fail ("retrieve-properties", error);

	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	/* "#t" is the S-expression that matches every object -- this connect's
	 * own list_existing_sync already pulled in the event seeded directly on
	 * the server before this process started. */
	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error))
		return fail ("query", error);

	for (link = components; link; link = link->next) {
		ICalComponent *component = link->data;
		const gchar *component_summary = i_cal_component_get_summary (component);

		if (component_summary && g_strcmp0 (component_summary, summary) == 0) {
			found = g_object_ref (component);
			break;
		}
	}

	g_print ("found=%d\n", found ? 1 : 0);

	if (found) {
		organizer = joined_values (found, I_CAL_ORGANIZER_PROPERTY);
		g_print ("read-back-organizer=%s\n", organizer);
		g_free (organizer);

		attendees = joined_values (found, I_CAL_ATTENDEE_PROPERTY);
		g_print ("read-back-attendees=%s\n", attendees);
		g_free (attendees);

		g_object_unref (found);
	}

	g_slist_free_full (components, g_object_unref);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
