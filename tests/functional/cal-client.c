/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the calendar functional test, and the equivalent of
 * book-client.c. It is an ordinary libecal consumer, as Evolution is. It
 * knows nothing about JMAP or the mock server. It opens a calendar by source
 * UID, reads it, writes events and occurrence edits to it, and reads them
 * back.
 *
 * `rust/crates/jmap-functional/tests/calendar.rs` owns the scratch XDG tree,
 * the `.source` keyfile, the private D-Bus session, the mock server and all
 * assertions. It runs this program and reads its output. This program has no
 * test framework and does not know the correct result. It writes each EDS
 * observation to stdout as one `key=value` line, and exits non-zero when a
 * call fails.
 *
 *   usage: functional-cal-client <source-uid> <summary> <all-day-summary>
 *                               <recurring-summary> <edited-occurrence-summary>
 *                               <split-summary> <zoned-summary>
 *                               <zoned-recurring-summary>
 */

#include <libecal/libecal.h>

#include "connection-status.h"
#include "event-start.h"
#include "test-util.h"

/* The start of the timed event. A UTC value, so that the scratch session
 * needs no timezone database, and both halves of the test check the same
 * string. */
#define TEST_DTSTART "20260115T130000Z"

/* The end of the timed event. It is a DTEND, not a DURATION, because the
 * Evolution appointment editor writes DTEND (e_cal_component_set_dtend).
 * RFC 5545 §3.6.1 makes the two mutually exclusive, so the backend must
 * understand this form to send the length of a user-created event. */
#define TEST_DTEND "20260115T143000Z"

/* The location of the timed event. On the server, a location is an entry in
 * the JSCalendar `locations` map, not a string. This value shows that a
 * LOCATION from a libecal consumer arrives there and is not dropped. Both
 * halves assert the same literal; see `LOCATION` in
 * `rust/crates/jmap-functional/tests/calendar.rs`. */
#define TEST_LOCATION "Room 42"

/* The categories of the timed event. The Evolution "Categories…" button
 * writes CATEGORIES; on the server, this is a JSCalendar `keywords` Set
 * (RFC 8984 §4.2.9). The two tags are on one line intentionally: libical
 * writes a multi-valued CATEGORIES again as one property for each value (see
 * `rust/crates/jmap-backend-cal/tests/marshal.rs`). Thus this value shows
 * that the full set, not only the first member, goes through the cache of
 * the meta backend. Both halves assert the same literals; see `KEYWORDS` in
 * `rust/crates/jmap-functional/tests/calendar.rs`. */
#define TEST_CATEGORIES "offsite,planning"

/* Whether the timed event blocks its time: the Evolution "Show Time as"
 * control. This is TRANSP on the component (RFC 5545 §3.8.2.7) and
 * `freeBusyStatus` on the server (RFC 8984 §4.4.2). The value is TRANSPARENT,
 * not the OPAQUE default of both formats, because a component without the
 * line is already in the default state. Both halves assert the same value;
 * see `FREE_BUSY_STATUS` in `rust/crates/jmap-functional/tests/calendar.rs`. */
#define TEST_TRANSP "TRANSPARENT"

/* The importance of the timed event: PRIORITY on the component (RFC 5545
 * §3.8.1.9) and `priority` on the server (RFC 8984 §4.4.1). Both formats use
 * the same integer, so this value tests the path through EDS, not a
 * translation. The mapping tests supply the property manually, so only real
 * EDS shows that the cache of the meta backend keeps a numeric property. The
 * value is 1, not 0, because a component without the line already has 0.
 * Both halves assert the same value; see `PRIORITY` in
 * `rust/crates/jmap-functional/tests/calendar.rs`. */
#define TEST_PRIORITY "1"

/* Who can see the timed event: CLASS on the component (RFC 5545 §3.8.1.3)
 * and `privacy` on the server (RFC 8984 §4.4.3). libical parses CLASS into
 * an ICalPropertyClass enum, not text, so it cannot write back a value that
 * it does not recognise. Thus the test asserts the path through the real
 * cache. The value is CONFIDENTIAL, not the PUBLIC default of both formats,
 * because a component without the line is already in the default state. Its
 * JSCalendar spelling (`secret`) is also different from the iCalendar
 * spelling, so it shows that the translation occurred. Both halves assert the
 * same value; see `CLASS` in `rust/crates/jmap-functional/tests/calendar.rs`. */
#define TEST_CLASS "CONFIDENTIAL"

/* The reminder of the timed event: the Evolution "Reminder" control. This is
 * a VALARM inside the VEVENT (RFC 5545 §3.6.6) and an entry in the JSCalendar
 * `alerts` map on the server (RFC 8984 §4.5.2). It is the first mapped
 * property that is a child component, not a content line. Only real EDS
 * shows whether the cache of `ECalMetaBackend` keeps a nested component. It
 * also shows whether the RFC 9074 §4 UID, which is the key of the entry,
 * stays beside the X-EVOLUTION-ALARM-UID that libecal adds.
 *
 * The offset is negative because a reminder is usually before the event. If
 * the sign is lost, the reminder fires after the event. Both halves assert
 * the same values; see `ALARM_TRIGGER` in
 * `rust/crates/jmap-functional/tests/calendar.rs`. */
#define TEST_ALARM_UID "k1"
#define TEST_ALARM_TRIGGER "-PT15M"

/* An all-day event, written as Evolution writes one: DATE values, not
 * DATE-TIMEs, for both the start and the end. This is the other event form
 * of RFC 5545 §3.6.1, and the only iCalendar way to say "a day, not a time".
 * The backend must recognise it to set JSCalendar showWithoutTime on the
 * server. */
#define TEST_ALL_DAY_DTSTART "20260201"
#define TEST_ALL_DAY_DTEND "20260202"

/* An event in a named zone. Like the zoned series below, it is intentionally
 * not written as text. The Evolution appointment editor sets the start from
 * an ICalTime that holds the zone object (e_cal_component_set_dtstart), and
 * libical then writes the TZID parameter. For a builtin zone, that TZID is
 * `/freeassociation.sourceforge.net/Europe/Berlin`: not an IANA name, and not
 * a name that a JMAP server can resolve. The setters give the component this
 * identifier without this file naming it, so the test always uses the value
 * that libical writes.
 *
 * The zone reaches the server only if the backend also sends the VTIMEZONE
 * that defines the identifier (RFC 5545 §3.2.19). Only real EDS can check
 * this: the mapping tests supply the identifier manually.
 *
 * The time is a wall-clock time, not UTC, because the test is about a zone.
 * The builtin zone data is compiled into libical, so the scratch session
 * needs no timezone database. */
#define TEST_ZONED_LOCATION "Europe/Berlin"
#define TEST_ZONED_DTSTART "20260115T160000"
#define TEST_ZONED_DTEND "20260115T173000"

/* A recurring event with one occurrence deleted. In the appointment list,
 * "Delete this occurrence" makes EDS keep the master component and add an
 * EXDATE to it. EXDATE (RFC 5545 §3.8.5.1) is the only iCalendar way to
 * exclude an occurrence; JSCalendar uses an entry in `recurrenceOverrides`.
 * If the backend drops an EXDATE, all other clients of the account still
 * show the appointment.
 *
 * The rule names a weekday, as the recurrence page of the appointment editor
 * writes for all frequencies except "every day". JSCalendar uses a `byDay` of
 * NDay objects (RFC 8984 §4.3.3). The mapping tests supply that form
 * manually, so only real EDS shows that the rule from libical keeps it. The
 * day is TH because TEST_DTSTART is a Thursday. Thus the rule, and all the
 * exclusions, detached instances and splits below, give the same occurrences
 * as without BYDAY. */
#define TEST_RECURRING_RRULE "FREQ=WEEKLY;COUNT=6;BYDAY=TH"
#define TEST_RECURRING_EXDATE "20260129T130000Z"

/* The occurrence that the user renames: "Edit this occurrence" in the same
 * menu. EDS keeps the master and adds a second VEVENT with the same UID and a
 * RECURRENCE-ID that identifies the replaced instance (RFC 5545 §3.8.4.4).
 * JSCalendar uses a patch in `recurrenceOverrides`, keyed by the start of the
 * instance. If the backend drops a detached instance, no other client sees
 * the change.
 *
 * This is the second occurrence of the weekly series above, one week before
 * the excluded occurrence, so the two exceptions are different. The start is
 * equal to the RECURRENCE-ID and the length is equal to the length of the
 * series, because the instance is renamed, not moved. Thus a title that did
 * not arrive is not confused with a start that did. */
#define TEST_RECURRING_EDITED_RECURRENCE_ID "20260122T130000Z"
#define TEST_RECURRING_EDITED_DTSTART "20260122T130000Z"
#define TEST_RECURRING_EDITED_DTEND "20260122T143000Z"

/* The categories of that one occurrence: the "Categories…" button for an
 * instance, not for the series. This is CATEGORIES on the detached component
 * and a `keywords` patch in `recurrenceOverrides`. The series has no tags, so
 * the full set is the difference. If the cache drops the tags, the instance
 * comes back untagged, as the series is. Two tags, so that libical also
 * splits the values on this component. Both halves assert the same literals;
 * see `RECURRING_EDITED_KEYWORDS` in
 * `rust/crates/jmap-functional/tests/calendar.rs`. */
#define TEST_RECURRING_EDITED_CATEGORIES "cancelled,offsite"

/* The reminder of that one occurrence: the "Reminder" control for an
 * instance, not for the series. This is a VALARM inside the detached
 * component and an `alerts` patch in `recurrenceOverrides`. When EDS puts the
 * two VEVENTs into one object, the alarm is a child of a child component, and
 * that is why the test measures it. The series has no reminder, so if the
 * cache drops this alarm, the occurrence looks as if the user removed it.
 *
 * The RFC 9074 §4 UID is written because it is the key of the `alerts`
 * entry. If the alarm loses it, the server gets the same reminder under a
 * different key. Both halves assert the same literals; see
 * `RECURRING_EDITED_ALERT_KEY` and `RECURRING_EDITED_ALARM_TRIGGER` in
 * `rust/crates/jmap-functional/tests/calendar.rs`. */
#define TEST_RECURRING_EDITED_ALARM_UID "k1"
#define TEST_RECURRING_EDITED_ALARM_TRIGGER "-PT1H"

/* An occurrence that is deleted as a user deletes one, not by an EXDATE in
 * the created component. "Delete this occurrence" is
 * e_cal_client_remove_object_sync with a RECURRENCE-ID and E_CAL_OBJ_MOD_THIS.
 * ECalMetaBackend changes this into a save of the master with one more
 * EXDATE; it does not call the removal vfunc. This translation is in EDS, not
 * in this project, and the created EXDATE above cannot test it. This is the
 * fourth occurrence of the weekly series, so it is not the excluded or the
 * edited occurrence. */
#define TEST_RECURRING_REMOVED_RECURRENCE_ID "20260205T130000Z"

/* The third item of that menu, "Edit this and future occurrences":
 * e_cal_client_modify_object_sync with E_CAL_OBJ_MOD_THIS_AND_FUTURE. It is
 * not an exception to a series: EDS splits the series in two. EDS truncates
 * the RRULE of the master to stop before the named instance. That instance
 * and all later ones become a second event with a UID from EDS, which goes to
 * the backend as a create (e_cal_util_split_at_instance_ex, then
 * ecmb_create_object_sync). Thus this is the only menu item that gives two
 * writes to the backend and makes a new event from an occurrence.
 * RANGE=THISANDFUTURE (RFC 5545 §3.2.13) does not occur, because EDS already
 * changed it into ordinary components.
 *
 * The component has the RRULE of the series, and this is necessary. EDS makes
 * an occurrence for a client as a clone of the master with a RECURRENCE-ID
 * (e_cal_util_construct_instance), so Evolution saves a component with the
 * rule on it. Also, e_cal_util_split_at_instance_ex returns NULL for a
 * component without recurrence, and ECalMetaBackend then reports success but
 * drops the edit.
 *
 * This is the fifth occurrence, after the three exceptions of the series. The
 * truncated master must keep all three, and the new event must have none. */
#define TEST_RECURRING_SPLIT_RECURRENCE_ID "20260212T130000Z"
#define TEST_RECURRING_SPLIT_DTSTART "20260212T130000Z"
#define TEST_RECURRING_SPLIT_DTEND "20260212T143000Z"

/* A series in one named zone with one occurrence moved into a different
 * zone: for example, a user who moves the weekly standup into the hours of a
 * journey. The zoned event above uses one zone for the full event, so it does
 * not test whether a second zone on a detached instance goes through EDS.
 *
 * It is a separate series, not one more exception to the weekly series above.
 * That series is in UTC and has three exceptions and a split, so a failure
 * there would not show which change caused it.
 *
 * Both zones come from the builtin table of libical and are set as zone
 * objects, for the reason that TEST_ZONED_LOCATION gives. The RECURRENCE-ID
 * is the exception: i_cal_component_set_recurrenceid writes a floating value
 * without TZID, so the code adds the parameter manually, from the zone
 * object. ECalComponent does the same for Evolution
 * (e_cal_component_set_recurid), so a component from the real editor has the
 * parameter. */
#define TEST_ZONED_RECURRING_RRULE "FREQ=WEEKLY;COUNT=3"
#define TEST_ZONED_RECURRING_DTSTART "20260305T100000"
#define TEST_ZONED_RECURRING_DTEND "20260305T110000"

/* The second occurrence, named on the clock of the series. RFC 5545
 * §3.8.4.4: a RECURRENCE-ID identifies the instance that the recurrence rules
 * generated, and the rules use the zone of the series. Thus it stays in
 * Berlin, but the instance moves to New York. On that date the two zones are
 * five hours apart, so a mapping that keeps the wall-clock time but drops the
 * zone gives a clear error. The length is equal to the length of the series,
 * so only the zone and the start are different. */
#define TEST_ZONED_MOVED_LOCATION "America/New_York"
#define TEST_ZONED_MOVED_RECURRENCE_ID "20260312T100000"
#define TEST_ZONED_MOVED_DTSTART "20260312T080000"
#define TEST_ZONED_MOVED_DTEND "20260312T090000"

/* Whether a component replaces one occurrence of a series and is not the
 * series. Only the RECURRENCE-ID (RFC 5545 §3.8.4.4) tells the two apart. */
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

/* All values of one property in a component, joined by commas: the EXDATEs
 * of RFC 5545 §3.8.5.1, or the CATEGORIES of §3.8.1.2.
 *
 * Each value is read as text, not as a typed value. libical can hold a list
 * as one property with `a,b` or as two properties with one value each; this
 * gives the same string for both. */
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

/* The TRIGGER (RFC 5545 §3.8.6.3) of the first VALARM in a component, as
 * text. An empty string if the event has no alarm, which is how a reminder
 * that the cache lost looks.
 *
 * Read as text for the same reason as CLASS and PRIORITY:
 * i_cal_property_get_trigger returns a struct whose zero value is a valid
 * "no offset", so it cannot tell a lost sign from a lost property. */
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

/* The RRULE (RFC 5545 §3.8.5.3) of a component, as text. The test reads both
 * the truncated master and the series that EDS split from it this way. An
 * empty string means that the component has no rule: a series that stopped. */
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

/* The value of the DTSTART of a component as text, and the TZID of its zone
 * (RFC 5545 §3.2.19). The TZID is an empty string for a floating time.
 *
 * Read from the property, not with i_cal_component_get_dtstart. That function
 * resolves the identifier against the VTIMEZONEs of the VCALENDAR and returns
 * a zone object, which shows what libical can find, not what EDS kept. Both
 * parts are returned because each alone can hide an error: a wall-clock start
 * without a zone and a start converted into the zone of the series both put
 * the appointment at the wrong hour.
 *
 * The TZID is reported exactly, with its prefix, because libical controls its
 * spelling. The test examines which zone it identifies, not its spelling. */
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

/* The component in a list that is a separate series with this SUMMARY: the
 * event that EDS made from an occurrence. The match uses the summary because
 * EDS, and then the server, assign the UID, so this side does not know it.
 * Detached instances are skipped because they belong to the original series,
 * whatever their title is. */
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
	gchar *categories;
	gchar *trigger;
	gchar *transp;
	gchar *priority;
	gchar *classification;
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

	/* Start evolution-source-registry on the session bus. It reads the
	 * scratch sources directory that the harness wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* Start evolution-calendar-factory, which dlopens libecalbackendjmap.so
	 * from EDS_CALENDAR_MODULES and selects the factory that matches the
	 * BackendName of the keyfile. A failure here is usually in one of those
	 * two steps, not in the backend logic.
	 *
	 * (guint32) -1 is the EDS value for "do not wait for connected"; see the
	 * same call in book-client.c for the reason. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return functional_fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	/* The EDS result of the connect, with a correct wait. It is more important
	 * here than for the address book: e_cal_client_connect_sync() succeeds
	 * also when connect_sync() of the backend failed, and this observation
	 * tells the two apart. */
	functional_report_connection_status (source, 10);

	/* Get the properties over the bus, not from the cached copy of the
	 * client. D-Bus notifications update that copy on a main context that
	 * this program does not run; see book-client.c. */
	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return functional_fail ("retrieve-properties", error);

	/* Whether the calendar accepts writes. EDS gets this from what the backend
	 * said during its connect. If the backend connects but does not say that
	 * it is writable, the calendar is read-only in the UI, and the write below
	 * reports only "Permission denied". This line lets the harness identify
	 * the cause, not the symptom. */
	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	/* "#t" is the S-expression that matches every object. */
	if (!e_cal_client_get_object_list_sync (cal, "#t", &components, NULL, &error))
		return functional_fail ("query", error);

	g_print ("events-before=%u\n", g_slist_length (components));
	g_slist_free_full (components, g_object_unref);
	components = NULL;

	/* Built from text, not with the libical-glib setters: the component that
	 * the test sends is exactly the text here, and one parse is simpler than
	 * many setters. The UID is a local name, as in Evolution. The server
	 * assigns the identifier of the event, so `added` below is not this
	 * string. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:jmap-functional-event\r\n"
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
		return functional_fail ("create", error);
	}

	g_object_unref (event);
	g_print ("added=%s\n", added_uid ? added_uid : "");

	/* Read from the cache of the meta backend, not from the server: EDS must
	 * keep what it wrote. */
	if (!e_cal_client_get_object_sync (cal, added_uid, NULL, &read_back, NULL, &error)) {
		g_free (added_uid);
		return functional_fail ("read-back", error);
	}

	read_back_event = functional_first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back: EDS returned an object with no VEVENT in it\n");
		g_free (added_uid);
		return 1;
	}

	g_print ("read-back-summary=%s\n", i_cal_component_get_summary (read_back_event));
	/* The location, from the same cached component. It goes to the server as
	 * a `locations` entry and must come back as the LOCATION line. If not, the
	 * appointment loses its room when EDS reads it again. */
	g_print ("read-back-location=%s\n", i_cal_component_get_location (read_back_event));
	/* The tags, from the same cached component. They are joined, not read as
	 * one line, because libical splits a multi-valued CATEGORIES into one
	 * property for each value. The output is the set, whatever its form. */
	categories = joined_values (read_back_event, I_CAL_CATEGORIES_PROPERTY);
	g_print ("read-back-categories=%s\n", categories);
	g_free (categories);
	/* Whether it blocks time, read as text and not with
	 * i_cal_property_get_transp. An empty string then means that the
	 * component has no TRANSP, which is how a lost value looks; the enum
	 * would report OPAQUE for that. */
	transp = joined_values (read_back_event, I_CAL_TRANSP_PROPERTY);
	g_print ("read-back-transp=%s\n", transp);
	g_free (transp);
	/* The importance, read as text for the same reason:
	 * i_cal_property_get_priority reports a missing property as 0, which is
	 * the state that a lost PRIORITY gives. */
	priority = joined_values (read_back_event, I_CAL_PRIORITY_PROPERTY);
	g_print ("read-back-priority=%s\n", priority);
	g_free (priority);
	/* The visibility, read as text for a stronger reason:
	 * i_cal_property_get_class reports a missing property as
	 * I_CAL_CLASS_NONE, and libical also parses an unrecognised value to that
	 * enum member. Thus the enum cannot tell a lost CLASS from a value that
	 * libical did not recognise, but the text can. */
	classification = joined_values (read_back_event, I_CAL_CLASS_PROPERTY);
	g_print ("read-back-class=%s\n", classification);
	g_free (classification);
	/* The reminder, from the same cached component. The alarm is a child
	 * component, so the cache must keep a full nested component, not a line.
	 * An empty string means that EDS returned the event without a reminder. */
	trigger = first_alarm_trigger (read_back_event);
	g_print ("read-back-alarm-trigger=%s\n", trigger);
	g_free (trigger);
	g_object_unref (read_back_event);

	/* The all-day event, through the same path. It is written second, so that
	 * a failure here is not confused with a failure of the timed event. */
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
		return functional_fail ("create-all-day", error);
	}

	g_object_unref (event);
	g_print ("added-all-day=%s\n", all_day_uid ? all_day_uid : "");
	g_free (all_day_uid);

	/* The zoned event, built with the setters, so that libical writes the
	 * TZID; see TEST_ZONED_LOCATION. */
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
		return functional_fail ("create-zoned", error);
	}

	g_object_unref (event);
	g_print ("added-zoned=%s\n", zoned_uid ? zoned_uid : "");
	g_free (zoned_uid);

	/* The recurring event, with an excluded occurrence. It is a separate
	 * event, so a failure here is not confused with the events above. */
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
		return functional_fail ("create-recurring", error);
	}

	g_object_unref (event);
	g_print ("added-recurring=%s\n", recurring_uid ? recurring_uid : "");

	/* Edit one occurrence, as "Edit this occurrence" does. The component has
	 * the UID of the series (from the server, not the local name), a
	 * RECURRENCE-ID that identifies the replaced instance, and its own
	 * SUMMARY. E_CAL_OBJ_MOD_THIS tells EDS that this is one instance; with
	 * E_CAL_OBJ_MOD_ALL, it would rename all occurrences. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:%s\r\n"
		"RECURRENCE-ID:%s\r\n"
		"DTSTART:%s\r\n"
		"DTEND:%s\r\n"
		"SUMMARY:%s\r\n"
		"CATEGORIES:%s\r\n"
		"BEGIN:VALARM\r\n"
		"UID:%s\r\n"
		"ACTION:DISPLAY\r\n"
		"DESCRIPTION:%s\r\n"
		"TRIGGER:%s\r\n"
		"END:VALARM\r\n"
		"END:VEVENT\r\n",
		recurring_uid, TEST_RECURRING_EDITED_RECURRENCE_ID,
		TEST_RECURRING_EDITED_DTSTART, TEST_RECURRING_EDITED_DTEND,
		edited_summary, TEST_RECURRING_EDITED_CATEGORIES,
		TEST_RECURRING_EDITED_ALARM_UID, edited_summary,
		TEST_RECURRING_EDITED_ALARM_TRIGGER);
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
		return functional_fail ("modify-occurrence", error);
	}

	g_object_unref (event);

	/* Read what EDS kept, by UID and RECURRENCE-ID. ECalMetaBackend stores a
	 * detached instance by that pair. A request by UID alone returns only the
	 * master, not a VCALENDAR with both. Thus only this request shows whether
	 * EDS kept or dropped the instance. */
	if (!e_cal_client_get_object_sync (cal, recurring_uid,
					   TEST_RECURRING_EDITED_RECURRENCE_ID,
					   &read_back, NULL, &error)) {
		g_free (recurring_uid);
		g_free (added_uid);
		return functional_fail ("read-back-occurrence", error);
	}

	read_back_event = functional_first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back-occurrence: EDS returned an object with no VEVENT in it\n");
		g_free (recurring_uid);
		g_free (added_uid);
		return 1;
	}

	/* Check, do not assume. An occurrence that EDS expanded from the RRULE has
	 * the summary of the series and no RECURRENCE-ID, so it is not the
	 * requested instance. */
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
	/* The tags of that instance, read as for the series: as text, joined.
	 * Only this instance holds this set. If the cache drops it, the next save
	 * sends the instance with the tags of the series, and the user loses the
	 * change. */
	categories = joined_values (read_back_event, I_CAL_CATEGORIES_PROPERTY);
	g_print ("edited-occurrence-categories=%s\n", categories);
	g_free (categories);
	/* The reminder of the instance, read as text for the same reason as the
	 * reminder of the series: the zero value of the struct cannot tell a lost
	 * sign from a kept one. The alarm is a component in a component in the
	 * returned object, so it is the deepest item that the cache must keep. */
	trigger = first_alarm_trigger (read_back_event);
	g_print ("edited-occurrence-alarm-trigger=%s\n", trigger);
	g_free (trigger);
	g_object_unref (read_back_event);

	/* "Delete this occurrence" on the fourth occurrence. Only the client sees
	 * a removal: EDS gives the backend the master with one more EXDATE. This
	 * occurs after the edit, so the series already has one exception of each
	 * type. A save that builds the overrides again from nothing loses them. */
	if (!e_cal_client_remove_object_sync (cal, recurring_uid,
					      TEST_RECURRING_REMOVED_RECURRENCE_ID,
					      E_CAL_OBJ_MOD_THIS,
					      E_CAL_OPERATION_FLAG_NONE, NULL, &error)) {
		g_free (recurring_uid);
		g_free (added_uid);
		return functional_fail ("remove-occurrence", error);
	}

	/* The series as EDS kept it. A request by UID without RECURRENCE-ID
	 * returns only the master, which holds the exclusions. */
	if (!e_cal_client_get_object_sync (cal, recurring_uid, NULL, &read_back, NULL, &error)) {
		g_free (recurring_uid);
		g_free (added_uid);
		return functional_fail ("read-back-series", error);
	}

	read_back_event = functional_first_vevent (read_back);
	g_object_unref (read_back);

	if (!read_back_event) {
		g_printerr ("read-back-series: EDS returned an object with no VEVENT in it\n");
		g_free (recurring_uid);
		g_free (added_uid);
		return 1;
	}

	exdates = joined_values (read_back_event, I_CAL_EXDATE_PROPERTY);
	g_object_unref (read_back_event);

	g_print ("recurring-exdates=%s\n", exdates);
	g_free (exdates);

	/* "Edit this and future occurrences" on the fifth occurrence. EDS changes
	 * it into a truncated series and a second event. It is the last of the
	 * three edits, so the series that EDS splits already has the exceptions
	 * of the other two. */
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
		return functional_fail ("modify-this-and-future", error);
	}

	g_object_unref (event);

	/* The remaining part of the series: the master again. EDS must have
	 * truncated its rule to stop before the split instance. If the save of
	 * the backend reverts the truncated rule, the series still recurs on the
	 * days of the new event, so the appointment shows twice with two titles. */
	if (!e_cal_client_get_object_sync (cal, recurring_uid, NULL, &read_back, NULL, &error)) {
		g_free (recurring_uid);
		g_free (added_uid);
		return functional_fail ("read-back-truncated", error);
	}

	g_free (recurring_uid);

	read_back_event = functional_first_vevent (read_back);
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

	/* The last event: a series in a named zone with one occurrence moved into
	 * a different zone. See TEST_ZONED_RECURRING_RRULE for why it is a
	 * separate series. Built with the setters, as the single zoned event is,
	 * for the same reason. */
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

	/* The rule comes from text, unlike the other parts of this component. An
	 * RRULE has no zone, so text here does not hardcode an identifier. */
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
		return functional_fail ("create-zoned-recurring", error);
	}

	g_object_unref (event);
	g_print ("added-zoned-recurring=%s\n",
		 zoned_recurring_uid ? zoned_recurring_uid : "");

	/* The move: "Edit this occurrence" again, which changes the zone, not the
	 * title. The RECURRENCE-ID stays in the zone of the series, because that
	 * identifies the generated instance. DTSTART and DTEND are in the new
	 * zone. */
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
		return functional_fail ("modify-zoned-occurrence", error);
	}

	g_object_unref (event);

	/* Read what EDS kept, by the pair again. The RECURRENCE-ID has no Z: it
	 * is a wall-clock time in the zone of the series, and ECalCache uses that
	 * string as the key of the detached instance. */
	if (!e_cal_client_get_object_sync (cal, zoned_recurring_uid,
					   TEST_ZONED_MOVED_RECURRENCE_ID,
					   &read_back, NULL, &error)) {
		g_free (zoned_recurring_uid);
		g_free (added_uid);
		return functional_fail ("read-back-zoned-occurrence", error);
	}

	g_free (zoned_recurring_uid);

	read_back_event = functional_first_vevent (read_back);
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
		return functional_fail ("query-after", error);
	}

	g_print ("events-after=%u\n", g_slist_length (components));

	/* The other half of the split, from the same list: the event that EDS
	 * made from the occurrence. This side cannot request it by UID, because
	 * EDS made a UID and then the server replaced it. Its start and rule show
	 * that the split occurred at the requested instance, not at the start of
	 * the series. */
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

	/* Make sure that it took none of the exceptions of the series. The two
	 * cancelled occurrences are both before the split, so an EXDATE here is
	 * an exclusion that EDS moved to the wrong days. */
	exdates = joined_values (split_event, I_CAL_EXDATE_PROPERTY);
	g_print ("split-exdates=%s\n", exdates);
	g_free (exdates);

	g_slist_free_full (components, g_object_unref);
	g_free (added_uid);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
