/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the calendar functional test, and the twin of
 * book-client.c one file over: an ordinary libecal consumer, the way
 * Evolution is one. It knows nothing about JMAP and nothing about the mock
 * server — it opens a calendar by source UID, reads it, writes one event to
 * it and reads that back.
 *
 * Everything around it — the scratch XDG tree, the `.source` keyfile, the
 * private D-Bus session, the mock server and every assertion — belongs to
 * `rust/crates/jmap-functional/tests/calendar.rs`, which runs this program
 * and reads its output. So this file has no test framework in it and no
 * notion of what "correct" is: it reports what EDS told it on stdout, one
 * `key=value` line per observation, and exits non-zero the moment a call
 * fails.
 *
 *   usage: functional-cal-client <source-uid> <summary> <all-day-summary>
 *                               <recurring-summary> <edited-occurrence-summary>
 */

#include <libecal/libecal.h>

#include "connection-status.h"

/* When the event this test writes starts. A UTC instant, so that nothing
 * here depends on a timezone database being reachable from the scratch
 * session, and the value the mock is checked for is the same string in
 * both halves of the test. */
#define TEST_DTSTART "20260115T130000Z"

/* And when it ends. Stated as DTEND rather than DURATION because that is
 * what Evolution's appointment editor writes — e_cal_component_set_dtend —
 * and RFC 5545 §3.6.1 makes the two mutually exclusive, so this is the form
 * the backend has to understand for a user-created event to reach the server
 * with a length at all. */
#define TEST_DTEND "20260115T143000Z"

/* And an all-day event, written the way Evolution writes one: DATE values
 * rather than DATE-TIMEs, on both ends. RFC 5545 §3.6.1's other form of an
 * event, and the only thing in iCalendar that says "this is a day, not a time
 * of day" — so it is what the backend has to recognise for the server to be
 * told, in JSCalendar's showWithoutTime, that the user made a day of it. */
#define TEST_ALL_DAY_DTSTART "20260201"
#define TEST_ALL_DAY_DTEND "20260202"

/* And a recurring event with one occurrence deleted, which is what a user
 * does with "Delete this occurrence" in the appointment list: EDS keeps the
 * master component and adds an EXDATE to it. RFC 5545 §3.8.5.1 is the only way
 * iCalendar says "not that one", and JSCalendar says it with an entry in
 * `recurrenceOverrides` — so an EXDATE the backend drops is an appointment
 * every other client reading the account still sees. */
#define TEST_RECURRING_RRULE "FREQ=WEEKLY;COUNT=6"
#define TEST_RECURRING_EXDATE "20260129T130000Z"

/* And the occurrence the user renames instead of deleting, which is "Edit this
 * occurrence" in the same menu: EDS keeps the master and adds a second VEVENT
 * with the same UID and a RECURRENCE-ID naming the instance it replaces
 * (RFC 5545 §3.8.4.4). That is the only way iCalendar says "that one, but
 * different", and JSCalendar says it with a patch under the instance's start in
 * `recurrenceOverrides` — so a detached instance the backend drops is a change
 * the user made and nobody else ever sees. The second occurrence of the weekly
 * series above, a week before the excluded one, so that neither exception can
 * be mistaken for the other. The start equals the RECURRENCE-ID and the length
 * the series', because this instance is renamed and not moved: an override that
 * changed those too would not tell a title that failed to arrive from a start
 * that did. */
#define TEST_RECURRING_EDITED_RECURRENCE_ID "20260122T130000Z"
#define TEST_RECURRING_EDITED_DTSTART "20260122T130000Z"
#define TEST_RECURRING_EDITED_DTEND "20260122T143000Z"

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* What EDS hands back from get_object: a bare VEVENT for an event with one
 * instance, or a VCALENDAR wrapping the instances when there are several.
 * This test writes one, but reading the wrapper's first VEVENT rather than
 * insisting on the bare form keeps a future recurrence from turning into a
 * failure that says nothing about recurrence. */
static ICalComponent *
first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}

/* Whether a component replaces one occurrence of a series rather than being
 * the series — RFC 5545 §3.8.4.4's RECURRENCE-ID, and the only thing that
 * tells the two apart. */
static gboolean
is_detached_instance (ICalComponent *component)
{
	ICalProperty *recurrence_id;

	recurrence_id = i_cal_component_get_first_property (
		component, I_CAL_RECURRENCEID_PROPERTY);
	if (!recurrence_id)
		return FALSE;

	g_object_unref (recurrence_id);

	return TRUE;
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
	gchar *added_uid = NULL;
	gchar *all_day_uid = NULL;
	gchar *recurring_uid = NULL;
	const gchar *source_uid;
	const gchar *summary;
	const gchar *all_day_summary;
	const gchar *recurring_summary;
	const gchar *edited_summary;

	if (argc != 6) {
		g_printerr ("usage: %s <source-uid> <summary> <all-day-summary> "
			    "<recurring-summary> <edited-occurrence-summary>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	summary = argv[2];
	all_day_summary = argv[3];
	recurring_summary = argv[4];
	edited_summary = argv[5];

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
	 * libecalbackendjmap.so out of EDS_CALENDAR_MODULES and picks the
	 * factory matching the keyfile's BackendName. A failure here is
	 * usually one of those two steps, not the backend's logic.
	 *
	 * (guint32) -1 is EDS's "do not wait for connected"; see the same call
	 * in book-client.c for why this test does not wait. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	/* EDS's own verdict on the connect, waited for properly. It says more
	 * here than it does for the address book: e_cal_client_connect_sync()
	 * succeeds even when the backend's connect_sync() failed, and this is
	 * the observation that tells those two apart. */
	functional_report_connection_status (source, 10);

	/* Over the bus rather than out of the client's cached copy, which is
	 * updated from D-Bus notifications on a main context this program
	 * never runs — see book-client.c. */
	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return fail ("retrieve-properties", error);

	/* Whether the calendar accepts writes at all. EDS derives this from
	 * what the backend said during its connect, so a backend that connects
	 * happily and never says it can be written to gives a calendar that is
	 * silently read-only in the UI — a state the write below would report
	 * only as "Permission denied". Reported separately so the harness can
	 * name the cause rather than the symptom. */
	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	/* "#t" is the S-expression that matches every object. */
	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error))
		return fail ("query", error);

	g_print ("events-before=%u\n", g_slist_length (components));
	g_slist_free_full (components, g_object_unref);
	components = NULL;

	/* Built from text rather than through the libical-glib setters: the
	 * component this test wants to send is exactly the one written here,
	 * and a parse is one step where a chain of setters is several. The UID
	 * is a name invented locally, which is what Evolution does too — the
	 * server assigns the identifier the calendar is keyed on afterwards,
	 * so `added` below is not this string. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:jmap-functional-event\r\n"
		"DTSTART:%s\r\n"
		"DTEND:%s\r\n"
		"SUMMARY:%s\r\n"
		"END:VEVENT\r\n",
		TEST_DTSTART, TEST_DTEND, summary);
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
	g_object_unref (read_back_event);

	/* The all-day one, through the same path. Written second so that a
	 * failure here cannot be mistaken for the timed event's. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:jmap-functional-all-day-event\r\n"
		"DTSTART;VALUE=DATE:%s\r\n"
		"DTEND;VALUE=DATE:%s\r\n"
		"SUMMARY:%s\r\n"
		"END:VEVENT\r\n",
		TEST_ALL_DAY_DTSTART, TEST_ALL_DAY_DTEND, all_day_summary);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	if (!event) {
		g_printerr ("build: libical would not parse the all-day event\n");
		g_free (added_uid);
		return 1;
	}

	if (!e_cal_client_create_object_sync (cal, event, E_CAL_OPERATION_FLAG_NONE,
					      &all_day_uid, NULL, &error)) {
		g_object_unref (event);
		g_free (added_uid);
		return fail ("create-all-day", error);
	}

	g_object_unref (event);
	g_print ("added-all-day=%s\n", all_day_uid ? all_day_uid : "");
	g_free (all_day_uid);

	/* The recurring one, with an occurrence excluded. Written last for the
	 * same reason as the all-day one. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:jmap-functional-recurring-event\r\n"
		"DTSTART:%s\r\n"
		"DTEND:%s\r\n"
		"SUMMARY:%s\r\n"
		"RRULE:%s\r\n"
		"EXDATE:%s\r\n"
		"END:VEVENT\r\n",
		TEST_DTSTART, TEST_DTEND, recurring_summary,
		TEST_RECURRING_RRULE, TEST_RECURRING_EXDATE);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	if (!event) {
		g_printerr ("build: libical would not parse the recurring event\n");
		g_free (added_uid);
		return 1;
	}

	if (!e_cal_client_create_object_sync (cal, event, E_CAL_OPERATION_FLAG_NONE,
					      &recurring_uid, NULL, &error)) {
		g_object_unref (event);
		g_free (added_uid);
		return fail ("create-recurring", error);
	}

	g_object_unref (event);
	g_print ("added-recurring=%s\n", recurring_uid ? recurring_uid : "");

	/* And one occurrence of it edited, which is what "Edit this occurrence"
	 * does: a component with the series' UID — the one the server assigned,
	 * not the name this program invented — a RECURRENCE-ID naming the
	 * instance it replaces, and its own SUMMARY. E_CAL_OBJ_MOD_THIS is what
	 * tells EDS this is that instance and not the series; with
	 * E_CAL_OBJ_MOD_ALL it would rename every occurrence instead. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:%s\r\n"
		"RECURRENCE-ID:%s\r\n"
		"DTSTART:%s\r\n"
		"DTEND:%s\r\n"
		"SUMMARY:%s\r\n"
		"END:VEVENT\r\n",
		recurring_uid, TEST_RECURRING_EDITED_RECURRENCE_ID,
		TEST_RECURRING_EDITED_DTSTART, TEST_RECURRING_EDITED_DTEND,
		edited_summary);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	if (!event) {
		g_printerr ("build: libical would not parse the edited occurrence\n");
		g_free (recurring_uid);
		g_free (added_uid);
		return 1;
	}

	if (!e_cal_client_modify_object_sync (cal, event, E_CAL_OBJ_MOD_THIS,
					      E_CAL_OPERATION_FLAG_NONE, NULL, &error)) {
		g_object_unref (event);
		g_free (recurring_uid);
		g_free (added_uid);
		return fail ("modify-occurrence", error);
	}

	g_object_unref (event);

	/* What EDS kept of it, asked for by UID *and* RECURRENCE-ID: that pair
	 * is how ECalMetaBackend stores a detached instance, and asking for the
	 * UID alone answers with the series — the master alone, not a VCALENDAR
	 * holding both. So this is the only question that distinguishes an
	 * instance EDS kept from one it dropped. */
	if (!e_cal_client_get_object_sync (cal, recurring_uid,
					   TEST_RECURRING_EDITED_RECURRENCE_ID,
					   &read_back, NULL, &error)) {
		g_free (recurring_uid);
		g_free (added_uid);
		return fail ("read-back-occurrence", error);
	}

	g_free (recurring_uid);

	read_back_event = first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back-occurrence: EDS returned an object with no VEVENT in it\n");
		g_free (added_uid);
		return 1;
	}

	/* Checked rather than assumed, because it is what stops this from
	 * passing on an occurrence EDS expanded out of the RRULE: an expansion
	 * carries the series' own summary, and a component without a
	 * RECURRENCE-ID is not the instance this asked for at all. */
	if (!is_detached_instance (read_back_event)) {
		g_printerr ("read-back-occurrence: EDS answered with a component that "
			    "replaces no occurrence\n");
		g_object_unref (read_back_event);
		g_free (added_uid);
		return 1;
	}

	g_print ("edited-occurrence-summary=%s\n",
		 i_cal_component_get_summary (read_back_event));
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
