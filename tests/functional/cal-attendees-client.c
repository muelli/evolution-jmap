/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Tests against a real server that a meeting created through `ECalClient`
 * keeps its attendees when EDS reads it back.
 *
 * This is `cal-live-client.c` with one question: do `ORGANIZER` and
 * `ATTENDEE` stay after a create? `jmap-cal-sync::CalSync::save_component`
 * reads the event from the server again after a create (see
 * `save_component_with`). Thus the meta backend cache holds the server copy
 * after the create, without a second connect or a refresh.
 *
 * `rust/crates/jmap-functional/tests/live-stalwart-calendar-attendees.rs`
 * creates the scratch XDG tree, the `.source` keyfile, the private D-Bus
 * session and the proxy to the server. It does all the assertions, and also
 * reads the server copy directly over JMAP.
 *
 *   usage: functional-cal-attendees-client <source-uid> <summary>
 */

#include <libecal/libecal.h>

#include "connection-status.h"
#include "test-util.h"

#define TEST_DTSTART "20260220T100000Z"
#define TEST_DTEND "20260220T110000Z"
#define TEST_ORGANIZER "mailto:organizer@example.com"
#define TEST_ATTENDEE_1 "mailto:attendee1@example.com"
#define TEST_ATTENDEE_2 "mailto:attendee2@example.com"

static ICalComponent *
first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}

/* All values of one property, joined by commas, or an empty string. Copy of
 * the helper in `cal-live-client.c`. */
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
	ICalComponent *event;
	ICalComponent *read_back = NULL;
	ICalComponent *read_back_event;
	gchar *icalendar;
	gchar *organizer;
	gchar *attendees;
	gchar *added_uid = NULL;
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

	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:%s\r\n"
		"DTSTART:%s\r\n"
		"DTEND:%s\r\n"
		"SUMMARY:%s\r\n"
		"ORGANIZER;CN=Organizer:%s\r\n"
		"ATTENDEE;CN=Attendee One;ROLE=REQ-PARTICIPANT:%s\r\n"
		"ATTENDEE;CN=Attendee Two;ROLE=OPT-PARTICIPANT:%s\r\n"
		"END:VEVENT\r\n",
		summary, TEST_DTSTART, TEST_DTEND, summary,
		TEST_ORGANIZER, TEST_ATTENDEE_1, TEST_ATTENDEE_2);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	if (!event) {
		g_printerr ("build: libical would not parse the event this test writes\n");
		return 1;
	}

	if (!e_cal_client_create_object_sync (cal, event, E_CAL_OPERATION_FLAG_NONE,
					      &added_uid, NULL, &error)) {
		g_object_unref (event);
		return functional_fail ("create", error);
	}

	g_object_unref (event);
	g_print ("added=%s\n", added_uid ? added_uid : "");

	/* Read from the meta backend cache, which holds the server copy (see
	 * the file header). */
	if (!e_cal_client_get_object_sync (cal, added_uid, NULL, &read_back, NULL, &error)) {
		g_free (added_uid);
		return functional_fail ("read-back", error);
	}

	read_back_event = first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back: EDS returned an object with no VEVENT in it\n");
		g_free (added_uid);
		return 1;
	}

	organizer = joined_values (read_back_event, I_CAL_ORGANIZER_PROPERTY);
	g_print ("read-back-organizer=%s\n", organizer);
	g_free (organizer);

	attendees = joined_values (read_back_event, I_CAL_ATTENDEE_PROPERTY);
	g_print ("read-back-attendees=%s\n", attendees);
	g_free (attendees);

	g_object_unref (read_back_event);
	g_free (added_uid);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
