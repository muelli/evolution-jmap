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
 *                               <split-summary> <zoned-summary>
 *                               <zoned-recurring-summary>
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

/* And an event in a named zone — the one thing in this file that is deliberately
 * not written as text. Evolution's appointment editor sets a start from an
 * ICalTime carrying the zone *object* (e_cal_component_set_dtstart with a time
 * whose zone is the one the user picked), and libical then writes the TZID
 * parameter itself: for a builtin zone that is its own
 * `/freeassociation.sourceforge.net/Europe/Berlin`, which is neither an IANA
 * name nor anything a JMAP server can resolve. Building the component through
 * the setters is what makes this test carry that identifier without naming it,
 * so it keeps holding whatever libical writes rather than a string copied from
 * libical once.
 *
 * The zone reaches the server only if the envelope the backend builds also
 * carries the VTIMEZONE that defines the identifier — RFC 5545 §3.2.19 — and
 * that is what nothing below real EDS can check: the mapping's own tests have
 * to supply the identifier by hand, so they cannot say whether the component
 * Evolution actually hands over travels with its zone.
 *
 * A wall-clock time, not a UTC one, because a zoned event is the point; the
 * builtin zone data is compiled into libical, so this still asks nothing of the
 * scratch session's environment. */
#define TEST_ZONED_LOCATION "Europe/Berlin"
#define TEST_ZONED_DTSTART "20260115T160000"
#define TEST_ZONED_DTEND "20260115T173000"

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

/* And the occurrence deleted the way a user deletes one, rather than by writing
 * the EXDATE above into the component before it is created. "Delete this
 * occurrence" is e_cal_client_remove_object_sync with a RECURRENCE-ID and
 * E_CAL_OBJ_MOD_THIS, which ECalMetaBackend turns into a *save of the master*
 * carrying one more EXDATE — the removal vfunc is never reached. That
 * translation is EDS's, not this project's, and it is what the created-with-an-
 * EXDATE case above cannot exercise. The fourth occurrence of the weekly
 * series, so that it is neither the excluded one nor the edited one. */
#define TEST_RECURRING_REMOVED_RECURRENCE_ID "20260205T130000Z"

/* And the third thing that menu offers, "Edit this and future occurrences":
 * e_cal_client_modify_object_sync with E_CAL_OBJ_MOD_THIS_AND_FUTURE. It is the
 * only one of the three that is not an exception to a series at all — EDS
 * answers it by *splitting the series in two*. The master's RRULE is truncated
 * to stop before the named instance, and that instance and everything after it
 * becomes a second event under a UID of EDS's own invention, handed to the
 * backend as a create (e_cal_util_split_at_instance_ex, then
 * ecmb_create_object_sync). So this is the only menu item that reaches the
 * backend as two writes, and the only one that makes a new event out of an
 * occurrence — RANGE=THISANDFUTURE (RFC 5545 §3.2.13) never appears, because
 * EDS has already resolved it into ordinary components by then.
 *
 * The component carries the series' RRULE, and that is not decoration. EDS
 * builds an occurrence for a client by cloning the master and adding a
 * RECURRENCE-ID (e_cal_util_construct_instance), so a component with the rule
 * on it is exactly what Evolution has in hand to save; and
 * e_cal_util_split_at_instance_ex answers a component with no recurrence in it
 * with NULL, which ECalMetaBackend reports as success while dropping the edit.
 *
 * The fifth occurrence, which is after all three exceptions the series carries
 * by now: the truncated master has to keep every one of them and the new event
 * has to carry none. */
#define TEST_RECURRING_SPLIT_RECURRENCE_ID "20260212T130000Z"
#define TEST_RECURRING_SPLIT_DTSTART "20260212T130000Z"
#define TEST_RECURRING_SPLIT_DTEND "20260212T143000Z"

/* And the one thing every case above leaves unsaid: a series in one named zone
 * with a single occurrence moved into another — the user who drags the week's
 * standup into the hours they are travelling. Every zoned case so far has one
 * zone for the whole event, so nothing has yet asked whether a *second* zone,
 * named by one detached instance, survives the trip through EDS.
 *
 * It is its own series rather than a sixth exception to the weekly one above,
 * because that one is in UTC and by now carries three exceptions and a split;
 * a zone question answered on top of all that would not say which of the two
 * broke it.
 *
 * Both zones are taken from libical's builtin table and set as zone *objects*,
 * for the reason TEST_ZONED_LOCATION gives: what reaches the backend is then
 * libical's own identifier rather than a string written here. The RECURRENCE-ID
 * is the exception that proves it — i_cal_component_set_recurrenceid writes a
 * floating value with no TZID on it, so the parameter is put on by hand, out of
 * the zone object rather than out of a literal. ECalComponent does the same
 * thing for Evolution (e_cal_component_set_recurid), which is why the component
 * a real editor saves has it and one built through the plain libical setters
 * does not. */
#define TEST_ZONED_RECURRING_RRULE "FREQ=WEEKLY;COUNT=3"
#define TEST_ZONED_RECURRING_DTSTART "20260305T100000"
#define TEST_ZONED_RECURRING_DTEND "20260305T110000"

/* The second occurrence, named on the series' own clock. RFC 5545 §3.8.4.4: a
 * RECURRENCE-ID names the instance the recurrence rules generated, and the rules
 * run in the series' zone — so this stays in Berlin even though the instance it
 * points at is being moved to New York. The instance's own start is the moved
 * one, and the two are five hours and a different clock apart, which is what
 * makes a mapping that carried the wall-clock time without the zone visible
 * rather than a rounding error. Its length is the series', so that what differs
 * about this occurrence is the zone and the start and nothing else. */
#define TEST_ZONED_MOVED_LOCATION "America/New_York"
#define TEST_ZONED_MOVED_RECURRENCE_ID "20260312T100000"
#define TEST_ZONED_MOVED_DTSTART "20260312T080000"
#define TEST_ZONED_MOVED_DTEND "20260312T090000"

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

/* Every instant a component says does not happen, joined by commas — the
 * EXDATEs of RFC 5545 §3.8.5.1, read back as text.
 *
 * Each property is asked for its value rather than for a time, which folds the
 * two shapes libical may hold a list in — one property carrying `a,b` or two
 * properties carrying one each — into the same string, so what this reports
 * depends on which instants are excluded and not on how they were written. */
static gchar *
exdate_values (ICalComponent *component)
{
	GString *values = g_string_new (NULL);
	ICalProperty *property;

	property = i_cal_component_get_first_property (component, I_CAL_EXDATE_PROPERTY);
	while (property) {
		ICalProperty *next;
		gchar *value = i_cal_property_get_value_as_string (property);

		if (values->len)
			g_string_append_c (values, ',');
		g_string_append (values, value ? value : "");
		g_free (value);

		next = i_cal_component_get_next_property (component, I_CAL_EXDATE_PROPERTY);
		g_object_unref (property);
		property = next;
	}

	return g_string_free (values, FALSE);
}

/* The recurrence rule a component carries, as text — the RRULE of RFC 5545
 * §3.8.5.3, read back the same way for the truncated master and for the series
 * EDS split off it. An empty string for a component with no rule, which is a
 * series that stopped recurring rather than one whose rule this cannot see. */
static gchar *
rrule_value (ICalComponent *component)
{
	ICalProperty *property;
	gchar *value;

	property = i_cal_component_get_first_property (component, I_CAL_RRULE_PROPERTY);
	if (!property)
		return g_strdup ("");

	value = i_cal_property_get_value_as_string (property);
	g_object_unref (property);

	return value ? value : g_strdup ("");
}

/* The two halves of a component's DTSTART: the value as text, and the TZID
 * naming the clock it is on (RFC 5545 §3.2.19) or an empty string for a
 * floating one.
 *
 * Read off the property rather than through i_cal_component_get_dtstart,
 * because that resolves the identifier against the enclosing VCALENDAR's
 * VTIMEZONEs and hands back a time carrying a zone *object* — which would
 * report what libical could look up rather than what EDS kept. Both are
 * returned together because either alone passes for the other going wrong: a
 * wall-clock start with no zone and a start silently converted into the
 * series' zone are the same appointment at the wrong hour, stated differently.
 *
 * The TZID is reported verbatim, prefix and all, because it is libical's to
 * spell — this test asks which zone it names, not how libical writes it. */
static void
dtstart_parts (ICalComponent *component,
	       gchar **value,
	       gchar **tzid)
{
	ICalProperty *property;
	ICalParameter *parameter;

	*value = NULL;
	*tzid = NULL;

	property = i_cal_component_get_first_property (component, I_CAL_DTSTART_PROPERTY);
	if (!property) {
		*value = g_strdup ("");
		*tzid = g_strdup ("");
		return;
	}

	*value = i_cal_property_get_value_as_string (property);

	parameter = i_cal_property_get_first_parameter (property, I_CAL_TZID_PARAMETER);
	if (parameter) {
		*tzid = g_strdup (i_cal_parameter_get_tzid (parameter));
		g_object_unref (parameter);
	}

	g_object_unref (property);

	if (!*value)
		*value = g_strdup ("");
	if (!*tzid)
		*tzid = g_strdup ("");
}

/* The one component in a list that is a series of its own with this SUMMARY:
 * the event EDS made out of an occurrence. Matched on the summary because the
 * UID is EDS's invention and then the server's, so nothing on this side knows
 * it; the detached instances are skipped because an instance of the original
 * series is not the new event however it is titled. */
static ICalComponent *
find_series_by_summary (GSList *components,
			const gchar *summary)
{
	GSList *link;

	for (link = components; link; link = g_slist_next (link)) {
		ICalComponent *component = link->data;
		const gchar *found = i_cal_component_get_summary (component);

		if (is_detached_instance (component))
			continue;

		if (found && g_strcmp0 (found, summary) == 0)
			return component;
	}

	return NULL;
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
	ICalComponent *split_event;
	ICalProperty *property;
	ICalTime *dtstart;
	ICalTime *zoned_time;
	ICalTimezone *zone;
	ICalTimezone *moved_zone;
	gchar *icalendar;
	gchar *exdates;
	gchar *rrule;
	gchar *tzid;
	gchar *added_uid = NULL;
	gchar *all_day_uid = NULL;
	gchar *recurring_uid = NULL;
	gchar *zoned_uid = NULL;
	gchar *zoned_recurring_uid = NULL;
	const gchar *source_uid;
	const gchar *summary;
	const gchar *all_day_summary;
	const gchar *recurring_summary;
	const gchar *edited_summary;
	const gchar *split_summary;
	const gchar *zoned_summary;
	const gchar *zoned_recurring_summary;

	if (argc != 9) {
		g_printerr ("usage: %s <source-uid> <summary> <all-day-summary> "
			    "<recurring-summary> <edited-occurrence-summary> "
			    "<split-summary> <zoned-summary> "
			    "<zoned-recurring-summary>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	summary = argv[2];
	all_day_summary = argv[3];
	recurring_summary = argv[4];
	edited_summary = argv[5];
	split_summary = argv[6];
	zoned_summary = argv[7];
	zoned_recurring_summary = argv[8];

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

	/* The zoned one, built through the setters so that the TZID on it is
	 * libical's own rather than one written here — see TEST_ZONED_LOCATION. */
	zone = i_cal_timezone_get_builtin_timezone (TEST_ZONED_LOCATION);

	if (!zone) {
		g_printerr ("build: libical has no builtin zone for %s\n",
			    TEST_ZONED_LOCATION);
		g_free (added_uid);
		return 1;
	}

	event = i_cal_component_new_vevent ();
	i_cal_component_set_uid (event, "jmap-functional-zoned-event");
	i_cal_component_set_summary (event, zoned_summary);

	zoned_time = i_cal_time_new_from_string (TEST_ZONED_DTSTART);
	i_cal_time_set_timezone (zoned_time, zone);
	i_cal_component_set_dtstart (event, zoned_time);
	g_object_unref (zoned_time);

	zoned_time = i_cal_time_new_from_string (TEST_ZONED_DTEND);
	i_cal_time_set_timezone (zoned_time, zone);
	i_cal_component_set_dtend (event, zoned_time);
	g_object_unref (zoned_time);

	if (!e_cal_client_create_object_sync (cal, event, E_CAL_OPERATION_FLAG_NONE,
					      &zoned_uid, NULL, &error)) {
		g_object_unref (event);
		g_free (added_uid);
		return fail ("create-zoned", error);
	}

	g_object_unref (event);
	g_print ("added-zoned=%s\n", zoned_uid ? zoned_uid : "");
	g_free (zoned_uid);

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

	read_back_event = first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back-occurrence: EDS returned an object with no VEVENT in it\n");
		g_free (recurring_uid);
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
		g_free (recurring_uid);
		g_free (added_uid);
		return 1;
	}

	g_print ("edited-occurrence-summary=%s\n",
		 i_cal_component_get_summary (read_back_event));
	g_object_unref (read_back_event);

	/* And "Delete this occurrence" on a fourth one, which is a removal only
	 * from the client's side: EDS answers it by handing the backend the
	 * master with one more EXDATE. Done after the edit so that the series
	 * already carries an exception of each kind when it happens, which is the
	 * state a save that rebuilt the overrides from scratch would flatten. */
	if (!e_cal_client_remove_object_sync (cal, recurring_uid,
					      TEST_RECURRING_REMOVED_RECURRENCE_ID,
					      E_CAL_OBJ_MOD_THIS,
					      E_CAL_OPERATION_FLAG_NONE, NULL, &error)) {
		g_free (recurring_uid);
		g_free (added_uid);
		return fail ("remove-occurrence", error);
	}

	/* The series as EDS kept it: asking by UID with no RECURRENCE-ID answers
	 * with the master alone, which is the component the exclusions live on. */
	if (!e_cal_client_get_object_sync (cal, recurring_uid, NULL, &read_back, NULL, &error)) {
		g_free (recurring_uid);
		g_free (added_uid);
		return fail ("read-back-series", error);
	}

	read_back_event = first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back-series: EDS returned an object with no VEVENT in it\n");
		g_free (recurring_uid);
		g_free (added_uid);
		return 1;
	}

	exdates = exdate_values (read_back_event);
	g_object_unref (read_back_event);

	g_print ("recurring-exdates=%s\n", exdates);
	g_free (exdates);

	/* And "Edit this and future occurrences" on the fifth one, which EDS turns
	 * into a truncated series plus a second event. Last of the three, so the
	 * series it cuts in two is the one the other two have already left their
	 * exceptions on. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:%s\r\n"
		"RECURRENCE-ID:%s\r\n"
		"DTSTART:%s\r\n"
		"DTEND:%s\r\n"
		"SUMMARY:%s\r\n"
		"RRULE:%s\r\n"
		"END:VEVENT\r\n",
		recurring_uid, TEST_RECURRING_SPLIT_RECURRENCE_ID,
		TEST_RECURRING_SPLIT_DTSTART, TEST_RECURRING_SPLIT_DTEND,
		split_summary, TEST_RECURRING_RRULE);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	if (!event) {
		g_printerr ("build: libical would not parse the split occurrence\n");
		g_free (recurring_uid);
		g_free (added_uid);
		return 1;
	}

	if (!e_cal_client_modify_object_sync (cal, event, E_CAL_OBJ_MOD_THIS_AND_FUTURE,
					      E_CAL_OPERATION_FLAG_NONE, NULL, &error)) {
		g_object_unref (event);
		g_free (recurring_uid);
		g_free (added_uid);
		return fail ("modify-this-and-future", error);
	}

	g_object_unref (event);

	/* What is left of the series EDS cut: the master again, whose rule has to
	 * have been shortened to stop before the instance the split began at. A
	 * rule EDS truncated and the backend's save undid is a series that still
	 * recurs over the days the new event now owns — the same appointment
	 * twice, under two titles. */
	if (!e_cal_client_get_object_sync (cal, recurring_uid, NULL, &read_back, NULL, &error)) {
		g_free (recurring_uid);
		g_free (added_uid);
		return fail ("read-back-truncated", error);
	}

	g_free (recurring_uid);

	read_back_event = first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back-truncated: EDS returned an object with no VEVENT in it\n");
		g_free (added_uid);
		return 1;
	}

	rrule = rrule_value (read_back_event);
	g_object_unref (read_back_event);

	g_print ("series-rrule=%s\n", rrule);
	g_free (rrule);

	/* And the last event: a series in a named zone with one occurrence moved
	 * into another — see TEST_ZONED_RECURRING_RRULE for why it is a series of
	 * its own rather than a sixth exception to the one above. Built through
	 * the setters, like the one-off zoned event, and for the same reason. */
	zone = i_cal_timezone_get_builtin_timezone (TEST_ZONED_LOCATION);
	moved_zone = i_cal_timezone_get_builtin_timezone (TEST_ZONED_MOVED_LOCATION);

	if (!moved_zone) {
		g_printerr ("build: libical has no builtin zone for %s\n",
			    TEST_ZONED_MOVED_LOCATION);
		g_free (added_uid);
		return 1;
	}

	event = i_cal_component_new_vevent ();
	i_cal_component_set_uid (event, "jmap-functional-zoned-recurring-event");
	i_cal_component_set_summary (event, zoned_recurring_summary);

	zoned_time = i_cal_time_new_from_string (TEST_ZONED_RECURRING_DTSTART);
	i_cal_time_set_timezone (zoned_time, zone);
	i_cal_component_set_dtstart (event, zoned_time);
	g_object_unref (zoned_time);

	zoned_time = i_cal_time_new_from_string (TEST_ZONED_RECURRING_DTEND);
	i_cal_time_set_timezone (zoned_time, zone);
	i_cal_component_set_dtend (event, zoned_time);
	g_object_unref (zoned_time);

	/* The rule from text, unlike everything else on this component: a RRULE
	 * carries no zone, so writing it out is not the shortcut that hardcoding
	 * an identifier would be. */
	property = i_cal_property_new_from_string ("RRULE:" TEST_ZONED_RECURRING_RRULE);

	if (!property) {
		g_printerr ("build: libical would not parse the zoned series' rule\n");
		g_object_unref (event);
		g_free (added_uid);
		return 1;
	}

	i_cal_component_take_property (event, property);

	if (!e_cal_client_create_object_sync (cal, event, E_CAL_OPERATION_FLAG_NONE,
					      &zoned_recurring_uid, NULL, &error)) {
		g_object_unref (event);
		g_free (added_uid);
		return fail ("create-zoned-recurring", error);
	}

	g_object_unref (event);
	g_print ("added-zoned-recurring=%s\n",
		 zoned_recurring_uid ? zoned_recurring_uid : "");

	/* And the move itself: "Edit this occurrence" a second time, changing the
	 * clock rather than the title. The RECURRENCE-ID stays on the series'
	 * zone because that is the instance the rules generated; the DTSTART and
	 * DTEND are on the zone the user moved it to. */
	event = i_cal_component_new_vevent ();
	i_cal_component_set_uid (event, zoned_recurring_uid);
	i_cal_component_set_summary (event, zoned_recurring_summary);

	zoned_time = i_cal_time_new_from_string (TEST_ZONED_MOVED_RECURRENCE_ID);
	i_cal_time_set_timezone (zoned_time, zone);
	property = i_cal_property_new_recurrenceid (zoned_time);
	i_cal_property_take_parameter (
		property, i_cal_parameter_new_tzid (i_cal_timezone_get_tzid (zone)));
	i_cal_component_take_property (event, property);
	g_object_unref (zoned_time);

	zoned_time = i_cal_time_new_from_string (TEST_ZONED_MOVED_DTSTART);
	i_cal_time_set_timezone (zoned_time, moved_zone);
	i_cal_component_set_dtstart (event, zoned_time);
	g_object_unref (zoned_time);

	zoned_time = i_cal_time_new_from_string (TEST_ZONED_MOVED_DTEND);
	i_cal_time_set_timezone (zoned_time, moved_zone);
	i_cal_component_set_dtend (event, zoned_time);
	g_object_unref (zoned_time);

	if (!e_cal_client_modify_object_sync (cal, event, E_CAL_OBJ_MOD_THIS,
					      E_CAL_OPERATION_FLAG_NONE, NULL, &error)) {
		g_object_unref (event);
		g_free (zoned_recurring_uid);
		g_free (added_uid);
		return fail ("modify-zoned-occurrence", error);
	}

	g_object_unref (event);

	/* What EDS kept of it, asked for by the pair again. The RECURRENCE-ID is
	 * spelled without a Z: it is a wall-clock time on the series' zone, and
	 * that is the string ECalCache keys the detached instance on. */
	if (!e_cal_client_get_object_sync (cal, zoned_recurring_uid,
					   TEST_ZONED_MOVED_RECURRENCE_ID,
					   &read_back, NULL, &error)) {
		g_free (zoned_recurring_uid);
		g_free (added_uid);
		return fail ("read-back-zoned-occurrence", error);
	}

	g_free (zoned_recurring_uid);

	read_back_event = first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back-zoned-occurrence: EDS returned an object with no "
			    "VEVENT in it\n");
		g_free (added_uid);
		return 1;
	}

	if (!is_detached_instance (read_back_event)) {
		g_printerr ("read-back-zoned-occurrence: EDS answered with a component "
			    "that replaces no occurrence\n");
		g_object_unref (read_back_event);
		g_free (added_uid);
		return 1;
	}

	dtstart_parts (read_back_event, &icalendar, &tzid);
	g_object_unref (read_back_event);

	g_print ("zoned-occurrence-dtstart=%s\n", icalendar);
	g_print ("zoned-occurrence-tzid=%s\n", tzid);
	g_free (icalendar);
	g_free (tzid);

	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error)) {
		g_free (added_uid);
		return fail ("query-after", error);
	}

	g_print ("events-after=%u\n", g_slist_length (components));

	/* And the other half of the split, found in the same listing: the event
	 * EDS made out of the occurrence, which nothing on this side can ask for
	 * by UID because EDS invented one and the server then replaced it. Its own
	 * start and its own rule are what say the split happened where it was
	 * asked for rather than at the head of the series. */
	split_event = find_series_by_summary (components, split_summary);

	if (!split_event) {
		g_printerr ("split: no series titled '%s' is in the calendar\n", split_summary);
		g_slist_free_full (components, g_object_unref);
		g_free (added_uid);
		return 1;
	}

	dtstart = i_cal_component_get_dtstart (split_event);
	icalendar = dtstart ? i_cal_time_as_ical_string (dtstart) : NULL;
	g_clear_object (&dtstart);

	g_print ("split-dtstart=%s\n", icalendar ? icalendar : "");
	g_free (icalendar);

	rrule = rrule_value (split_event);
	g_print ("split-rrule=%s\n", rrule);
	g_free (rrule);

	/* And that it took none of the series' exceptions with it. The two
	 * cancelled occurrences are both before the split, so an EXDATE here is an
	 * exclusion EDS moved onto days it does not belong to. */
	exdates = exdate_values (split_event);
	g_print ("split-exdates=%s\n", exdates);
	g_free (exdates);

	g_slist_free_full (components, g_object_unref);
	g_free (added_uid);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
