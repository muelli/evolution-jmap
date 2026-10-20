/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The calendar client against a real server. It is the calendar equivalent
 * of the `write` phase in `book-client.c`.
 *
 * `cal-client.c` is the client of the mock-based suite. It writes many
 * events in one run, so one difference on a real server makes the full run
 * fail. Thus this file writes only one event: SUMMARY, LOCATION, CATEGORIES,
 * TRANSP, PRIORITY, CLASS and one VALARM. It creates the event and reads it
 * back.
 *
 * `rust/crates/jmap-functional/tests/live-stalwart-calendar.rs` creates the
 * scratch XDG tree, the `.source` keyfile, the private D-Bus session and the
 * proxy to the server, and it does all the assertions. This program writes
 * one `key=value` line on stdout for each observation. It exits non-zero
 * when a call fails.
 *
 *   usage: functional-cal-live-client <source-uid> <summary>
 */

#include <libecal/libecal.h>

#include "connection-status.h"

/* The event that this test writes. A UTC time makes sure that the test does
 * not need a timezone database. */
#define TEST_DTSTART "20260115T130000Z"
#define TEST_DTEND "20260115T143000Z"

/* The LOCATION. It must become an entry in a JSCalendar `locations` map
 * (RFC 8984 §4.2.5). */
#define TEST_LOCATION "Room 42"

/* The CATEGORIES, a JSCalendar `keywords` Set (RFC 8984 §4.2.9). Two tags
 * on one line, because libical writes one property for each value. */
#define TEST_CATEGORIES "offsite,planning"

/* TRANSP (RFC 5545 §3.8.2.7), a JSCalendar `freeBusyStatus` (RFC 8984
 * §4.4.2). TRANSPARENT, because OPAQUE is the default in both formats. */
#define TEST_TRANSP "TRANSPARENT"

/* PRIORITY (RFC 5545 §3.8.1.9), a JSCalendar `priority` (RFC 8984 §4.4.1).
 * 1, because both formats read 0 as no value. */
#define TEST_PRIORITY "1"

/* CLASS (RFC 5545 §3.8.1.3), a JSCalendar `privacy` (RFC 8984 §4.4.3).
 * CONFIDENTIAL becomes `secret` in JSCalendar, so it also shows the
 * translation. */
#define TEST_CLASS "CONFIDENTIAL"

/* A VALARM, a JSCalendar `alerts` entry (RFC 8984 §4.5.2) with the RFC 9074
 * §4 UID of the alarm as its key. The offset is negative, because a reminder
 * usually comes before the event. */
#define TEST_ALARM_UID "k1"
#define TEST_ALARM_TRIGGER "-PT15M"

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* get_object gives a bare VEVENT or a VCALENDAR that contains it. Copy of
 * the helper in `cal-client.c`. */
static ICalComponent *
first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}

/* All values of one property, joined by commas. Copy of the helper in
 * `cal-client.c`. */
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

/* The TRIGGER of the first VALARM, as text. Copy of the helper in
 * `cal-client.c`. */
static gchar *
first_alarm_trigger (ICalComponent *event)
{
	ICalComponent *alarm;
	ICalProperty *property;
	gchar *value;

	alarm = i_cal_component_get_first_component (event, I_CAL_VALARM_COMPONENT);
	if (!alarm)
		return g_strdup ("");

	property = i_cal_component_get_first_property (alarm, I_CAL_TRIGGER_PROPERTY);
	if (!property) {
		g_object_unref (alarm);
		return g_strdup ("");
	}

	value = i_cal_property_get_value_as_string (property);
	g_object_unref (property);
	g_object_unref (alarm);

	return value ? value : g_strdup ("");
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
	GSList *components = NULL;
	ICalComponent *event;
	ICalComponent *read_back = NULL;
	ICalComponent *read_back_event;
	gchar *icalendar;
	gchar *categories;
	gchar *trigger;
	gchar *transp;
	gchar *priority;
	gchar *classification;
	gchar *added_uid = NULL;
	const gchar *source_uid;
	const gchar *summary;

	if (argc != 3) {
		g_printerr ("usage: %s <source-uid> <summary>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	summary = argv[2];

	/* Starts evolution-source-registry, which reads the scratch sources. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* A real server needs a password and no GUI prompt is available. Store
	 * it where EDS looks for Basic auth, as `book-client.c` does. */
	{
		const gchar *seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

		if (seed_password && *seed_password &&
		    !e_source_store_password_sync (source, seed_password, TRUE, NULL, &error))
			return fail ("store-password", error);
	}

	/* Starts evolution-calendar-factory, which loads libecalbackendjmap.so
	 * from EDS_CALENDAR_MODULES. (guint32) -1 tells EDS not to wait for
	 * "connected"; connection-status.c gives the reason. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	/* Wait for the connection status from EDS. */
	functional_report_connection_status (source, 10);

	/* Get the properties over D-Bus. The cached copy needs a main loop, and
	 * this program has none. */
	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return fail ("retrieve-properties", error);

	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	/* "#t" is the S-expression that matches all objects. */
	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error))
		return fail ("query", error);

	g_print ("events-before=%u\n", g_slist_length (components));
	g_slist_free_full (components, g_object_unref);
	components = NULL;

	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:jmap-functional-live-event\r\n"
		"DTSTART:%s\r\n"
		"DTEND:%s\r\n"
		"SUMMARY:%s\r\n"
		"LOCATION:%s\r\n"
		"CATEGORIES:%s\r\n"
		"TRANSP:%s\r\n"
		"PRIORITY:%s\r\n"
		"CLASS:%s\r\n"
		"BEGIN:VALARM\r\n"
		"UID:%s\r\n"
		"ACTION:DISPLAY\r\n"
		"DESCRIPTION:%s\r\n"
		"TRIGGER:%s\r\n"
		"END:VALARM\r\n"
		"END:VEVENT\r\n",
		TEST_DTSTART, TEST_DTEND, summary, TEST_LOCATION, TEST_CATEGORIES,
		TEST_TRANSP, TEST_PRIORITY, TEST_CLASS,
		TEST_ALARM_UID, summary, TEST_ALARM_TRIGGER);
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

	/* Read from the meta backend cache, not the server: EDS must keep what
	 * it wrote. */
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

	g_print ("read-back-summary=%s\n", i_cal_component_get_summary (read_back_event));
	g_print ("read-back-location=%s\n", i_cal_component_get_location (read_back_event));

	categories = joined_values (read_back_event, I_CAL_CATEGORIES_PROPERTY);
	g_print ("read-back-categories=%s\n", categories);
	g_free (categories);

	transp = joined_values (read_back_event, I_CAL_TRANSP_PROPERTY);
	g_print ("read-back-transp=%s\n", transp);
	g_free (transp);

	priority = joined_values (read_back_event, I_CAL_PRIORITY_PROPERTY);
	g_print ("read-back-priority=%s\n", priority);
	g_free (priority);

	classification = joined_values (read_back_event, I_CAL_CLASS_PROPERTY);
	g_print ("read-back-class=%s\n", classification);
	g_free (classification);

	trigger = first_alarm_trigger (read_back_event);
	g_print ("read-back-alarm-trigger=%s\n", trigger);
	g_free (trigger);
	g_object_unref (read_back_event);

	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error)) {
		g_free (added_uid);
		return fail ("query-after", error);
	}

	g_print ("events-after=%u\n", g_slist_length (components));

	g_slist_free_full (components, g_object_unref);
	g_free (added_uid);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
