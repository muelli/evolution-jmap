/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Tests against a real server that an event with JSCalendar-bis
 * `calendarAddress` participants, created on the server by a different
 * client, shows ORGANIZER and ATTENDEE when a new `ECalClient` connects.
 *
 * This is the opposite of `cal-attendees-client.c`. This program writes
 * nothing: a raw `jmap_client` creates the event before the program starts.
 * The first connect of a new meta backend cache calls `list_existing_sync`
 * (see `cal-changes-client.c`), so no refresh is necessary.
 *
 * The code under test is `jmap_ical::event::drawn_participants`. It turns
 * `CalendarEvent.participants` into ORGANIZER and ATTENDEE lines with
 * `calendar_address`, which reads `calendarAddress` or else `sendTo`.
 *
 * `rust/crates/jmap-functional/tests/live-stalwart-calendar-attendees-remote.rs`
 * creates the scratch XDG tree, the `.source` keyfile, the private D-Bus
 * session and the proxy to the server. It creates the event over JMAP, runs
 * this program and does all the assertions.
 *
 *   usage: functional-cal-attendees-remote-client <source-uid> <summary>
 */

#include <libecal/libecal.h>

#include "connection-status.h"
#include "test-util.h"

/* All values of one property, joined by commas, or an empty string. Copy of
 * the helper in `cal-attendees-client.c`. */
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
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	{
		const gchar *seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

		if (seed_password && *seed_password &&
		    !e_source_store_password_sync (source, seed_password, TRUE, NULL, &error))
			return functional_fail ("store-password", error);
	}

	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return functional_fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	functional_report_connection_status (source, 10);

	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return functional_fail ("retrieve-properties", error);

	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	/* "#t" matches all objects. The list_existing_sync of the connect
	 * already got the event from the server. */
	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error))
		return functional_fail ("query", error);

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
