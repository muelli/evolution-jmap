/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Item 80 stage 2 batch 2: the calendar leg's own client against a real
 * server, the twin of `book-client.c`'s `write` phase one file over.
 *
 * `cal-client.c` beside this file is the mock-based suite's client, and it
 * deliberately stays untouched: it creates an all-day event, a zoned event, a
 * six-occurrence recurring series with an excluded, an edited and a removed
 * occurrence, a THISANDFUTURE split, and a zoned recurring series with one
 * occurrence moved into a second zone — all in one run, and a real server
 * genuinely differing on any one of those (a split's two writes, say) would
 * fail the whole run rather than just mismeasure one field. This file is the
 * one event at the head of that list only: SUMMARY, LOCATION, CATEGORIES,
 * TRANSP, PRIORITY, CLASS, one VALARM, created and read back — the same scope
 * `book-client.c`'s `write` phase covers for the address book, and the same
 * bet item 80 stage 2's own plan names: the simplest connectivity leg first,
 * the richer ones as a later batch.
 *
 * Everything around it — the scratch XDG tree, the `.source` keyfile, the
 * private D-Bus session, the proxy to the real server and every assertion —
 * belongs to `rust/crates/jmap-functional/tests/live-stalwart-calendar.rs`,
 * which runs this program and reads its output. So this file has no test
 * framework in it and no notion of what "correct" is: it reports what EDS
 * told it on stdout, one `key=value` line per observation, and exits non-zero
 * the moment a call fails.
 *
 *   usage: functional-cal-live-client <source-uid> <summary>
 */

#include <libecal/libecal.h>

#include "connection-status.h"

/* The event this test writes. A UTC instant, so that nothing here depends on
 * a timezone database being reachable from the scratch session. */
#define TEST_DTSTART "20260115T130000Z"
#define TEST_DTEND "20260115T143000Z"

/* Where it happens — the LOCATION, which has to reach the server as an entry
 * in a JSCalendar `locations` map (RFC 8984 §4.2.5) rather than being
 * dropped. */
#define TEST_LOCATION "Room 42"

/* What it is filed under — the CATEGORIES, a JSCalendar `keywords` Set
 * (RFC 8984 §4.2.9). Two tags on one line, since libical re-renders a
 * multi-valued CATEGORIES as one property per value. */
#define TEST_CATEGORIES "offsite,planning"

/* Whether it blocks the time it occupies — TRANSP (RFC 5545 §3.8.2.7), a
 * JSCalendar `freeBusyStatus` (RFC 8984 §4.4.2). TRANSPARENT rather than the
 * OPAQUE both formats default to, since the default is the state a component
 * with no line on it is already in. */
#define TEST_TRANSP "TRANSPARENT"

/* How important it is — PRIORITY (RFC 5545 §3.8.1.9), a JSCalendar
 * `priority` (RFC 8984 §4.4.1). 1 rather than 0, the value both formats treat
 * as no value at all. */
#define TEST_PRIORITY "1"

/* Who may see it — CLASS (RFC 5545 §3.8.1.3), a JSCalendar `privacy`
 * (RFC 8984 §4.4.3). CONFIDENTIAL because its JSCalendar spelling (`secret`)
 * differs from its iCalendar one, so this also says the translation
 * happened. */
#define TEST_CLASS "CONFIDENTIAL"

/* When the user is reminded of it — a VALARM, a JSCalendar `alerts` entry
 * (RFC 8984 §4.5.2) keyed by the alarm's RFC 9074 §6 UID. A negative offset,
 * since a reminder is normally before the event. */
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

/* What EDS hands back from get_object: a bare VEVENT for an event with one
 * instance, or a VCALENDAR wrapping the instance. Mirrors `cal-client.c`'s
 * own helper of the same name. */
static ICalComponent *
first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}

/* Every value a component states for one property, joined by commas.
 * Mirrors `cal-client.c`'s own helper of the same name. */
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

/* When the first reminder of a component fires, as text. Mirrors
 * `cal-client.c`'s own helper of the same name. */
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

	/* A real server needs a password the mock never checks, and nothing
	 * here has a GUI to be prompted through. Stored the ordinary way EDS
	 * looks one up for Basic auth, mirroring `book-client.c`'s own step. */
	{
		const gchar *seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

		if (seed_password && *seed_password &&
		    !e_source_store_password_sync (source, seed_password, TRUE, NULL, &error))
			return fail ("store-password", error);
	}

	/* Activates evolution-calendar-factory, which is what dlopens
	 * libecalbackendjmap.so out of EDS_CALENDAR_MODULES and picks the
	 * factory matching the keyfile's BackendName. (guint32) -1 is EDS's
	 * "do not wait for connected"; see cal-client.c for why this test does
	 * not wait either. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	/* EDS's own verdict on the connect, waited for properly. */
	functional_report_connection_status (source, 10);

	/* Over the bus rather than out of the client's cached copy, which is
	 * updated from D-Bus notifications on a main context this program
	 * never runs. */
	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return fail ("retrieve-properties", error);

	/* Whether the calendar accepts writes at all. */
	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	/* "#t" is the S-expression that matches every object. */
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

	/* Out of the meta backend's cache rather than off the server, which is
	 * the point: EDS is meant to have kept what it just wrote. */
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
