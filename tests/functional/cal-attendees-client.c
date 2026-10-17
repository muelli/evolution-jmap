/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * ROADMAP item 112, step (a): live confirmation that a meeting created
 * locally through `ECalClient` keeps its attendees after EDS reads it back.
 *
 * The twin of `cal-live-client.c`, narrowed to one question: whether
 * `ORGANIZER`/`ATTENDEE` survive a create. `jmap-cal-sync::CalSync::
 * save_component` re-reads the event from the server rather than trusting
 * the component it was handed (see that function's own comment), so EDS's
 * meta-backend cache ends up with whatever the server echoes back on the
 * very same create, no second connect or explicit refresh needed. Item 108
 * part (1) used this program to observe the attendees being lost; item 112
 * made them go out as `participants`, so it now observes them surviving.
 *
 * Everything around this file -- the scratch XDG tree, the `.source`
 * keyfile, the private D-Bus session, the proxy to the real server and
 * every assertion -- belongs to `rust/crates/jmap-functional/tests/
 * live-stalwart-calendar-attendees.rs`, which runs this program and reads
 * its output, and also checks the server's own copy directly over JMAP.
 *
 *   usage: functional-cal-attendees-client <source-uid> <summary>
 */

#include <libecal/libecal.h>

#include "connection-status.h"

#define TEST_DTSTART "20260220T100000Z"
#define TEST_DTEND "20260220T110000Z"
#define TEST_ORGANIZER "mailto:organizer@example.com"
#define TEST_ATTENDEE_1 "mailto:attendee1@example.com"
#define TEST_ATTENDEE_2 "mailto:attendee2@example.com"

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

static ICalComponent *
first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}

/* Every value a component states for one property, joined by commas, or the
 * empty string if it states none -- mirrors `cal-live-client.c`'s own
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
		return fail ("create", error);
	}

	g_object_unref (event);
	g_print ("added=%s\n", added_uid ? added_uid : "");

	/* Out of the meta backend's cache, same as `cal-live-client.c`'s own
	 * read-back -- see this file's header for why that is already enough
	 * to observe whether the organizer/attendees made the round trip. */
	if (!e_cal_client_get_object_sync (cal, added_uid, NULL, &read_back, NULL, &error)) {
		g_free (added_uid);
		return fail ("read-back", error);
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
