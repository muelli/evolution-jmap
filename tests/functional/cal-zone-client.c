/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The third client half of the calendar functional test. It is an ordinary
 * libecal consumer that opens a calendar and reads events that the server
 * already held. For each event, it reports the instant at which the event
 * starts, in two ways: from libical alone, and from the calendar, which is
 * the path of Evolution. Then it saves the first event two times and reports
 * again after each save.
 *
 * The two edits are the two types of edit, and they test the save path in
 * opposite ways. The first changes the title, which has no relation to the
 * clock, so any change to the clock comes from the save. The second moves the
 * appointment to a different wall-clock time and gives its new length as a
 * `DTEND`, as the Evolution editor does, in place of the `DURATION` that the
 * mapping wrote. This edit is only about the clock, so it shows a save that
 * sends the zone again when a date-time is in the patch. Neither edit may
 * lose the zone, and only the server can identify the zone.
 *
 * This is a separate program, not more arguments to cal-edit-client.c. That
 * program examines what a save keeps of the members that have no iCalendar
 * line. To give it an event without a location, conference and attachment,
 * it would have to check less, or the event would need members that this
 * test does not use. The edits occur in the same run as the reads because
 * the saves must use the cache that the read filled. An appointment whose
 * zone resolves only until the first edit is not usable.
 *
 * One run reads several events because the test measures a contrast: a zone
 * that the server defined against a zone that it only named. One cache from
 * one refresh answers both, so a difference in the answers is a difference
 * between the events. Two runs would share the XDG directories of the
 * session, and thus the cache of the meta backend, which the harness starts
 * empty.
 *
 * The `create` mode tests the opposite direction. The read mode starts from a
 * zone that the server names; `create` starts from a zone that the client
 * names. It gives the calendar a `VTIMEZONE` through
 * `e_cal_client_add_timezone_sync`, then creates an event whose `DTSTART`
 * refers to it. Evolution does this when the user accepts an invitation with
 * a zone that no database holds. EDS keeps such a definition in the timezone
 * store of the calendar and leaves the `TZID` on the component. Thus the save
 * reaches the backend of this repository with a `TZID` and no definition.
 * This mode measures whether the backend gets the definition from that store
 * and sends it. It is a separate mode, not a phase of the read run, because
 * the only source of the zone in the store must be the client.
 *
 * The `series` mode tests the same question for a save that `create` cannot
 * do. It gives the calendar two definitions, creates a recurring event in the
 * first zone, and moves one occurrence into the second zone with
 * `E_CAL_OBJ_MOD_THIS`. This is a user who moves one day of a weekly meeting
 * into the hours of a journey. It reaches the backend as an update to an
 * event that the server holds, not as a create. A create sends the full
 * `timeZones` map, but an update can only add an entry to the current map.
 * Thus the second zone is the subject of this mode, and the first zone must
 * stay unchanged.
 *
 * The two zones come from two files, not from one file with two zones. The
 * test asserts the definition of each zone, and one file cannot tell a save
 * that sent the wrong zone from a save that sent both.
 *
 * Like the other calendar clients, it does not know the correct result. It
 * writes each EDS observation to stdout as one `key=value` line, and exits
 * non-zero when a call fails. `rust/crates/jmap-functional/tests/calendar.rs`
 * owns all assertions. It seeds the events, runs this program and reads its
 * output.
 *
 *   usage: functional-cal-zone-client read <source-uid> <new-summary>
 *              <new-dtstart> <new-dtend> <event-uid>...
 *          functional-cal-zone-client create <source-uid> <zone-file>
 *              <summary> <dtstart> <duration>
 *          functional-cal-zone-client series <source-uid> <zone-file>
 *              <moved-zone-file> <summary> <dtstart> <duration> <rrule>
 *              <recurrence-id> <moved-dtstart>
 *
 * The observations of the n-th event on the command line have the prefix
 * `event-n`, counting from one. The prefix uses the position, not the UID,
 * because the mock chooses the UID, and a key that starts with a digit is a
 * bad observation name. After the rename, the first event has the prefix
 * `saved`, and after the move, `moved`. The event that `create` made has the
 * prefix `created`, and the occurrence that `series` moved has `occurrence`.
 *
 * The two clocks are iCalendar `DATE-TIME` values without a zone. The moved
 * event keeps the `TZID` that is already on its `DTSTART`, because the test
 * examines whether a moved appointment stays in its zone. A client that named
 * the zone would supply the answer. The clock of `create` is also without a
 * zone and gets its `TZID` from the zone file, so this program never writes
 * the identifier. `series` takes three such clocks: the start of the series,
 * the `RECURRENCE-ID` of the moved occurrence, and its new start. It puts the
 * first two in the zone from the first file and the last in the zone from
 * the second file. This is the only arrangement that RFC 5545 §3.2.19
 * permits. The recurrence id identifies an instance that the rules
 * generated, so it is on the clock of the series, wherever the instance moved.
 */

#include <libecal/libecal.h>

#include "connection-status.h"
#include "event-start.h"

/* How long to wait until a seeded event is available, and how frequently to
 * ask. The same as EDIT_WAIT_TRIES in cal-edit-client.c, for the reason given
 * there: the backend starts a refresh during the connect, and a get before
 * the refresh is complete can get an answer from a cache that is not full. */
#define WAIT_TRIES 200
#define WAIT_INTERVAL_US 50000

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* Return the first VEVENT of what get_object returns: a bare VEVENT for an
 * event with one instance, or a VCALENDAR that contains the instances. This
 * is first_vevent of cal-edit-client.c. Here it is also important because a
 * VTIMEZONE can only be in the VCALENDAR, so the form that EDS selects
 * decides whether a definition can come with the event. */
static ICalComponent *
first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}

/* The number of VTIMEZONEs beside the event that EDS returned.
 *
 * Reported, not inferred from the instant, because it separates failures that
 * have different repairs. If the start resolves without a definition, it
 * resolved from the builtin table of libical, as an IANA name expects. If
 * there is no definition and the start does not resolve, the definition was
 * lost between the mapping and the cache of EDS: a bug in this repository. If
 * there is a definition and the start does not resolve, libical refused the
 * VTIMEZONE that the mapping made. The instant alone cannot tell the last two
 * apart, and their repairs are different. */
static guint
definition_count (ICalComponent *component)
{
	ICalComponent *child;
	guint count = 0;

	child = i_cal_component_get_first_component (component, I_CAL_VTIMEZONE_COMPONENT);
	while (child) {
		ICalComponent *next;

		count++;
		next = i_cal_component_get_next_component (component, I_CAL_VTIMEZONE_COMPONENT);
		g_object_unref (child);
		child = next;
	}

	return count;
}

/* Ask EDS for the zone that the DTSTART of the event names, and report the
 * instant of the start when it is resolved against that zone.
 *
 * This is the necessary measurement, and functional_report_start cannot do
 * it. libical resolves a `TZID` against the `VTIMEZONE`s of the enclosing
 * component and its builtin table, and neither holds a zone that only a
 * server can name. EDS does not return the definition beside the event: it
 * keeps it in the timezone store of the calendar, which
 * `e_cal_client_get_timezone_sync` reads. Evolution resolves a `TZID` in the
 * same way: its recurrence and alarm code uses a lookup callback that calls
 * this function. Thus this is the path of the consumer, and the libical-only
 * answer is a fact about the libical table, not about the account.
 *
 * Two observations. Whether the zone came back shows that the definition went
 * through the mapping, the marshalling, `ECalMetaBackend` and the D-Bus hop.
 * The instant shows that the returned zone is the zone that the server
 * described; a partial definition gives a different instant.
 *
 * The time is made again from the value on the line, not taken from
 * i_cal_component_get_dtstart. That function already resolved the `TZID` (to
 * nothing), and a floating time has no clock to interpret again. The parsed
 * value gives the wall-clock time of the line, to which a zone applies.
 */
static void
report_zone_lookup (const gchar *prefix,
		    ECalClient *cal,
		    ICalComponent *event)
{
	ICalProperty *property;
	ICalParameter *parameter;
	ICalTimezone *zone = NULL;
	ICalTime *start;
	ICalTime *utc;
	GError *error = NULL;
	gchar *value;

	property = i_cal_component_get_first_property (event, I_CAL_DTSTART_PROPERTY);
	parameter = property ? i_cal_property_get_first_parameter (
		property, I_CAL_TZID_PARAMETER) : NULL;

	if (parameter && !e_cal_client_get_timezone_sync (
			cal, i_cal_parameter_get_tzid (parameter), &zone, NULL, &error)) {
		/* Not a failure of this program: a calendar that does not know the
		 * zone returns an error, and that error is the observation. */
		g_clear_error (&error);
		zone = NULL;
	}

	g_print ("%s-zone-known=%d\n", prefix, zone ? 1 : 0);

	value = property ? i_cal_property_get_value_as_string (property) : NULL;
	start = value ? i_cal_time_new_from_string (value) : NULL;
	if (start && zone)
		i_cal_time_set_timezone (start, zone);
	utc = start ? i_cal_time_convert_to_zone (
		start, i_cal_timezone_get_utc_timezone ()) : NULL;
	g_free (value);
	value = utc ? i_cal_time_as_ical_string (utc) : NULL;
	g_print ("%s-zone-utc=%s\n", prefix, value ? value : "");
	g_free (value);

	g_clear_object (&utc);
	g_clear_object (&start);
	g_clear_object (&parameter);
	g_clear_object (&property);
}

/* Report the length of the event from both of the lines that can give it.
 *
 * Two observations, not one resolved length, because the line that has the
 * length is part of the measurement. The mapping writes a `DURATION`, the
 * Evolution editor writes a `DTEND`, and RFC 5545 §3.6.1 permits only one of
 * them on a component. Thus the pair shows which form came back. An event
 * with neither line, or with both, is a different bug from an event with the
 * wrong length. */
static void
report_length (const gchar *prefix,
	       ICalComponent *event)
{
	const struct {
		ICalPropertyKind kind;
		const gchar *name;
	} lines[] = {
		{ I_CAL_DURATION_PROPERTY, "duration" },
		{ I_CAL_DTEND_PROPERTY, "dtend" },
	};
	gsize i;

	for (i = 0; i < G_N_ELEMENTS (lines); i++) {
		ICalProperty *property;
		gchar *value;

		property = i_cal_component_get_first_property (event, lines[i].kind);
		value = property ? i_cal_property_get_value_as_string (property) : NULL;
		g_print ("%s-%s=%s\n", prefix, lines[i].name, value ? value : "");
		g_free (value);
		g_clear_object (&property);
	}
}

/* Move the appointment: change the wall-clock time of its `DTSTART`, and give
 * its new length as an end, not as a duration.
 *
 * The start is changed in place: the value of the current property changes,
 * and all its parameters stay. Evolution makes the same content line by a
 * different path. Its editor makes an ECalComponentDateTime and calls
 * e_cal_component_set_dtstart, which writes the value and the `TZID` that it
 * got. In both cases, the backend gets the same `DTSTART;TZID=…:…`. The
 * in-place edit is necessary: this program must not name the zone, because
 * then it would supply the answer that the test examines.
 *
 * The end is a new property with the `TZID` of that `DTSTART`, because
 * Evolution does not write an event with two clocks in different zones, and
 * this mapping cannot resolve one. The `DURATION` from the mapping is
 * removed. RFC 5545 §3.6.1 makes the two mutually exclusive, so a component
 * with both is not valid, and the source of the length would be unclear.
 *
 * FALSE when the component has no start, or when libical cannot read the end
 * back. Each is a failure of this program to make the edit, not an
 * observation. */
static gboolean
move_event (ICalComponent *event,
	    const gchar *new_start,
	    const gchar *new_end)
{
	ICalProperty *start;
	ICalProperty *end;
	ICalProperty *duration;
	ICalParameter *parameter;
	const gchar *tzid;
	gchar *line;

	start = i_cal_component_get_first_property (event, I_CAL_DTSTART_PROPERTY);
	if (!start)
		return FALSE;

	i_cal_property_set_value_from_string (start, new_start, "DATE-TIME");

	parameter = i_cal_property_get_first_parameter (start, I_CAL_TZID_PARAMETER);
	tzid = parameter ? i_cal_parameter_get_tzid (parameter) : NULL;
	line = tzid ? g_strdup_printf ("DTEND;TZID=%s:%s", tzid, new_end)
		    : g_strdup_printf ("DTEND:%s", new_end);
	end = i_cal_property_new_from_string (line);
	g_free (line);
	g_clear_object (&parameter);
	g_clear_object (&start);

	if (!end)
		return FALSE;

	duration = i_cal_component_get_first_property (event, I_CAL_DURATION_PROPERTY);
	if (duration) {
		i_cal_component_remove_property (event, duration);
		g_object_unref (duration);
	}

	i_cal_component_take_property (event, end);

	return TRUE;
}

/* Ask for one event until EDS has it, or stop. This is wait_for_event of
 * cal-edit-client.c: a miss is E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, not a
 * failure of the call, so the error is cleared and the request is sent
 * again. */
static ICalComponent *
wait_for_event (ECalClient *cal,
		const gchar *uid)
{
	guint try;

	for (try = 0; try < WAIT_TRIES; try++) {
		ICalComponent *component = NULL;
		GError *error = NULL;

		if (e_cal_client_get_object_sync (cal, uid, NULL, &component, NULL, &error))
			return component;

		g_clear_error (&error);
		g_usleep (WAIT_INTERVAL_US);
	}

	return NULL;
}

/* Send an edited component back and report, with `prefix`, what EDS returns
 * for the event after the save.
 *
 * The read-back is a plain get, not a wait. The event is in the cache: the
 * backend stores a save before it accepts it. Thus "not found" here is a
 * finding, not a race.
 *
 * The observations and their order are the same as in the read loop, because
 * the test compares a before and an after. A shorter report after a save
 * would give the assertions nothing to compare. 0 on success, else the exit
 * status of the program. */
static int
save_and_report (ECalClient *cal,
		 ICalComponent *event,
		 const gchar *uid,
		 const gchar *prefix)
{
	GError *error = NULL;
	ICalComponent *read_back = NULL;
	ICalComponent *read_back_event;

	if (!e_cal_client_modify_object_sync (cal, event, E_CAL_OBJ_MOD_ALL,
					      E_CAL_OPERATION_FLAG_NONE, NULL, &error))
		return fail ("modify", error);

	if (!e_cal_client_get_object_sync (cal, uid, NULL, &read_back, NULL, &error))
		return fail ("get-after-modify", error);

	read_back_event = first_vevent (read_back);
	if (!read_back_event) {
		g_printerr ("get-after-modify: what EDS handed back for %s holds no "
			    "VEVENT\n", uid);
		g_object_unref (read_back);
		return 1;
	}

	g_print ("%s-summary=%s\n", prefix, i_cal_component_get_summary (read_back_event));
	g_print ("%s-definitions=%u\n", prefix, definition_count (read_back));
	functional_report_start (prefix, read_back_event);
	report_zone_lookup (prefix, cal, read_back_event);
	report_length (prefix, read_back_event);

	g_object_unref (read_back_event);
	g_object_unref (read_back);

	return 0;
}

/* The read mode: report the seeded events, then do the two saves to the
 * first event. The event UIDs start at `argv[first]`.
 *
 * 0 on success, else the exit status of the program. */
static int
run_read (ECalClient *cal,
	  const gchar *new_summary,
	  const gchar *new_start,
	  const gchar *new_end,
	  int argc,
	  char **argv,
	  int first)
{
	ICalComponent *fetched;
	ICalComponent *event;
	const gchar *saved_uid;
	int status;
	int index;

	/* The first event on the command line gets the saves. */
	saved_uid = argv[first];

	for (index = first; index < argc; index++) {
		gchar *prefix = g_strdup_printf ("event-%d", index - first + 1);

		fetched = wait_for_event (cal, argv[index]);
		if (!fetched) {
			g_printerr ("get-seeded: EDS never handed back the event %s\n",
				    argv[index]);
			return 1;
		}

		event = first_vevent (fetched);
		if (!event) {
			g_printerr ("get-seeded: what EDS handed back for %s holds no "
				    "VEVENT\n", argv[index]);
			return 1;
		}

		g_print ("%s-summary=%s\n", prefix, i_cal_component_get_summary (event));
		g_print ("%s-definitions=%u\n", prefix, definition_count (fetched));
		/* Use the VEVENT, not its wrapper. This is safe because libical resolves
		 * a TZID against the component and its parents. first_vevent returns a
		 * child of `fetched` with its parent link, so a VTIMEZONE in the
		 * VCALENDAR is still available from here. */
		functional_report_start (prefix, event);
		report_zone_lookup (prefix, cal, event);
		report_length (prefix, event);

		g_free (prefix);
		g_object_unref (event);
		g_object_unref (fetched);
	}

	/* The first save. The code gets the event again, not from the loop
	 * above. This costs one call to a cache that holds the event, and the
	 * loop only reports.
	 *
	 * i_cal_component_set_summary changes the value of the current SUMMARY
	 * and leaves all other lines. Thus the component that goes back is the
	 * component from EDS with one changed line, as the Evolution appointment
	 * editor sends. This save does not change the DTSTART: an edit that is not
	 * about the zone must not lose the zone, and an edit that also stated the
	 * clock could not tell the two apart. */
	fetched = wait_for_event (cal, saved_uid);
	if (!fetched) {
		g_printerr ("get-before-modify: EDS no longer has the event %s\n", saved_uid);
		return 1;
	}

	event = first_vevent (fetched);
	if (!event) {
		g_printerr ("get-before-modify: what EDS handed back for %s holds no "
			    "VEVENT\n", saved_uid);
		return 1;
	}

	i_cal_component_set_summary (event, new_summary);
	status = save_and_report (cal, event, saved_uid, "saved");

	g_object_unref (event);
	g_object_unref (fetched);

	if (status != 0)
		return status;

	/* The second save, which changes only the clock. It edits the component
	 * that EDS returns now, not the component edited above, because a user
	 * sees the current component. The last save of the backend made the event
	 * again from the server data, so an edit of the older copy could send a
	 * line that the first save changed. */
	fetched = wait_for_event (cal, saved_uid);
	if (!fetched) {
		g_printerr ("get-before-move: EDS no longer has the event %s\n", saved_uid);
		return 1;
	}

	event = first_vevent (fetched);
	if (!event) {
		g_printerr ("get-before-move: what EDS handed back for %s holds no "
			    "VEVENT\n", saved_uid);
		return 1;
	}

	if (!move_event (event, new_start, new_end)) {
		g_printerr ("move: the event %s states no start, or libical would not "
			    "read the end back\n", saved_uid);
		return 1;
	}

	status = save_and_report (cal, event, saved_uid, "moved");

	g_object_unref (event);
	g_object_unref (fetched);

	return status;
}

/* Whether the component replaces an occurrence of a series and is not the
 * series: the `RECURRENCE-ID` of RFC 5545 §3.8.4.4. This is
 * is_detached_instance of cal-client.c.
 *
 * Necessary because `e_cal_client_get_object_sync` returns the master when
 * EDS has no detached copy of the instance. The master that `series` makes
 * has the correct UID, the correct summary and the old clock. Without this
 * check, a move that EDS did not keep looks like an event in the zone of the
 * series. A save that lost the moved zone looks the same, but it is a
 * different bug. */
static gboolean
is_detached_instance (ICalComponent *event)
{
	ICalProperty *recurrence_id;

	recurrence_id = i_cal_component_get_first_property (
		event, I_CAL_RECURRENCEID_PROPERTY);
	if (!recurrence_id)
		return FALSE;

	g_object_unref (recurrence_id);

	return TRUE;
}

/* Give the calendar the `VTIMEZONE` in `path`, report the identifier that
 * libical read from it as `report_key`, and return that identifier.
 *
 * The zone comes from a file, not from this program, so that the definition
 * and the assertions use the same text: the test writes the file and checks
 * the mock against it. The `TZID` comes from the zone that libical made, not
 * from a separate argument, so this program cannot name a zone that the file
 * does not. It is reported because, if libical used a different name, all
 * later `TZID`s would name a zone that nobody wrote.
 *
 * Evolution calls `e_cal_client_add_timezone_sync` before it saves an
 * appointment whose zone came from an invitation, not from a database. It is
 * the only path for such a definition to a backend: EDS removes the
 * `VTIMEZONE` from the component and keeps it in the timezone store of the
 * calendar.
 *
 * The identifier is a copy, so that the caller can use it after this function
 * releases its reference to the zone. NULL on failure, after a message on
 * stderr. */
static gchar *
add_zone_from_file (ECalClient *cal,
		    const gchar *path,
		    const gchar *report_key)
{
	GError *error = NULL;
	gchar *text = NULL;
	ICalComponent *definition;
	ICalTimezone *zone;
	const gchar *tzid;
	gchar *owned;

	if (!g_file_get_contents (path, &text, NULL, &error)) {
		fail ("zone-file", error);
		return NULL;
	}

	definition = i_cal_component_new_from_string (text);
	g_free (text);
	if (!definition) {
		g_printerr ("zone-file: libical would not parse %s\n", path);
		return NULL;
	}

	/* i_cal_timezone_set_component takes ownership of the component (its
	 * documentation: "the zone assumes ownership of the comp"), so the
	 * definition is not unreffed here. On the failure path the zone also keeps
	 * it, because its state is not known. */
	zone = i_cal_timezone_new ();
	if (!zone || !i_cal_timezone_set_component (zone, definition)) {
		g_printerr ("zone-file: libical would not make a zone of %s, which "
			    "means it states no TZID\n", path);
		g_clear_object (&zone);
		return NULL;
	}

	tzid = i_cal_timezone_get_tzid (zone);
	g_print ("%s=%s\n", report_key, tzid ? tzid : "");

	if (!tzid) {
		g_printerr ("zone-file: the zone libical made of %s names itself "
			    "nothing\n", path);
		g_object_unref (zone);
		return NULL;
	}

	owned = g_strdup (tzid);

	if (!e_cal_client_add_timezone_sync (cal, zone, NULL, &error)) {
		g_object_unref (zone);
		g_free (owned);
		fail ("add-timezone", error);
		return NULL;
	}

	g_object_unref (zone);

	return owned;
}

/* The create mode: give the calendar the `VTIMEZONE` in `zone_path`, create
 * an event that refers to it, and report what EDS returns for that event.
 *
 * add_zone_from_file describes the path of the zone to the calendar.
 *
 * 0 on success, else the exit status of the program. */
static int
run_create (ECalClient *cal,
	    const gchar *zone_path,
	    const gchar *summary,
	    const gchar *dtstart,
	    const gchar *duration)
{
	GError *error = NULL;
	gchar *icalendar;
	gchar *created_uid = NULL;
	ICalComponent *event;
	ICalComponent *fetched;
	ICalComponent *created;
	gchar *tzid;

	tzid = add_zone_from_file (cal, zone_path, "zone-tzid");
	if (!tzid)
		return 1;

	/* The UID is a local name of this program, as Evolution uses its own. The
	 * server chooses the UID of the event, which create_object returns. The
	 * length is a DURATION because the mapping writes that form, so a read-back
	 * with a DURATION went through the round trip. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:jmap-functional-client-zone\r\n"
		"DTSTART;TZID=%s:%s\r\n"
		"DURATION:%s\r\n"
		"SUMMARY:%s\r\n"
		"END:VEVENT\r\n",
		tzid, dtstart, duration, summary);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);
	g_free (tzid);

	if (!event) {
		g_printerr ("build: libical would not parse the event this test "
			    "writes\n");
		return 1;
	}

	if (!e_cal_client_create_object_sync (cal, event, E_CAL_OPERATION_FLAG_NONE,
					      &created_uid, NULL, &error)) {
		g_object_unref (event);
		return fail ("create", error);
	}

	g_object_unref (event);
	g_print ("created-uid=%s\n", created_uid ? created_uid : "");

	if (!created_uid) {
		g_printerr ("create: EDS accepted the event and named no uid for it\n");
		return 1;
	}

	fetched = wait_for_event (cal, created_uid);
	if (!fetched) {
		g_printerr ("get-created: EDS never handed back the event it made, %s\n",
			    created_uid);
		g_free (created_uid);
		return 1;
	}

	created = first_vevent (fetched);
	if (!created) {
		g_printerr ("get-created: what EDS handed back for %s holds no VEVENT\n",
			    created_uid);
		g_free (created_uid);
		g_object_unref (fetched);
		return 1;
	}

	g_print ("created-summary=%s\n", i_cal_component_get_summary (created));
	g_print ("created-definitions=%u\n", definition_count (fetched));
	functional_report_start ("created", created);
	report_zone_lookup ("created", cal, created);
	report_length ("created", created);

	g_free (created_uid);
	g_object_unref (created);
	g_object_unref (fetched);

	return 0;
}

/* The series mode: give the calendar both `VTIMEZONE`s, create a recurring
 * event in the first zone, move one occurrence into the second zone, and
 * report what EDS returns for that occurrence.
 *
 * Two saves; the second is the subject of this mode. The create sends the
 * full event, so the definition of the series zone goes as in the create
 * mode. The move is an update to an event that the server holds. There, the
 * `timeZones` map belongs to the server and the mapping can only add to it.
 * Only real EDS can do the two saves in that order: the second save must find
 * the map of the first on the server.
 *
 * The move is a full component with the UID of the series, a `RECURRENCE-ID`
 * that identifies the replaced instance, and its own `DTSTART`. This is what
 * "Edit this occurrence" in Evolution sends. `E_CAL_OBJ_MOD_THIS` tells EDS
 * that it replaces one instance, not the series. Its length is equal to the
 * length of the series, so only the clock and the zone are different from
 * the rule. A different duration would go in the same patch, and it would be
 * unclear which member caused the answer of the server.
 *
 * 0 on success, else the exit status of the program. */
static int
run_series (ECalClient *cal,
	    const gchar *zone_path,
	    const gchar *moved_zone_path,
	    const gchar *summary,
	    const gchar *dtstart,
	    const gchar *duration,
	    const gchar *rrule,
	    const gchar *recurrence_id,
	    const gchar *moved_dtstart)
{
	GError *error = NULL;
	gchar *icalendar;
	gchar *series_uid = NULL;
	gchar *tzid;
	gchar *moved_tzid;
	ICalComponent *event;
	ICalComponent *fetched;
	ICalComponent *occurrence;

	tzid = add_zone_from_file (cal, zone_path, "zone-tzid");
	if (!tzid)
		return 1;

	moved_tzid = add_zone_from_file (cal, moved_zone_path, "moved-zone-tzid");
	if (!moved_tzid) {
		g_free (tzid);
		return 1;
	}

	/* The rule is in the text with all other lines, unlike the series in
	 * cal-client.c. An RRULE has no zone, so text here does not hardcode an
	 * identifier. */
	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:jmap-functional-client-zone-series\r\n"
		"DTSTART;TZID=%s:%s\r\n"
		"DURATION:%s\r\n"
		"RRULE:%s\r\n"
		"SUMMARY:%s\r\n"
		"END:VEVENT\r\n",
		tzid, dtstart, duration, rrule, summary);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	if (!event) {
		g_printerr ("build: libical would not parse the series this test "
			    "writes\n");
		g_free (tzid);
		g_free (moved_tzid);
		return 1;
	}

	if (!e_cal_client_create_object_sync (cal, event, E_CAL_OPERATION_FLAG_NONE,
					      &series_uid, NULL, &error)) {
		g_object_unref (event);
		g_free (tzid);
		g_free (moved_tzid);
		return fail ("create-series", error);
	}

	g_object_unref (event);
	g_print ("series-uid=%s\n", series_uid ? series_uid : "");

	if (!series_uid) {
		g_printerr ("create-series: EDS accepted the series and named no uid "
			    "for it\n");
		g_free (tzid);
		g_free (moved_tzid);
		return 1;
	}

	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:%s\r\n"
		"RECURRENCE-ID;TZID=%s:%s\r\n"
		"DTSTART;TZID=%s:%s\r\n"
		"DURATION:%s\r\n"
		"SUMMARY:%s\r\n"
		"END:VEVENT\r\n",
		series_uid, tzid, recurrence_id, moved_tzid, moved_dtstart, duration,
		summary);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);
	g_free (tzid);
	g_free (moved_tzid);

	if (!event) {
		g_printerr ("build: libical would not parse the moved occurrence this "
			    "test writes\n");
		g_free (series_uid);
		return 1;
	}

	if (!e_cal_client_modify_object_sync (cal, event, E_CAL_OBJ_MOD_THIS,
					      E_CAL_OPERATION_FLAG_NONE, NULL, &error)) {
		g_object_unref (event);
		g_free (series_uid);
		return fail ("modify-occurrence", error);
	}

	g_object_unref (event);

	/* Request it by the pair, UID and RECURRENCE-ID. The id is a wall-clock
	 * time in the zone of the series, and ECalCache uses that string as the
	 * key of a detached instance. A plain get is sufficient: the backend
	 * stored the save before it accepted it, so "not found" is a finding, not
	 * a race. */
	if (!e_cal_client_get_object_sync (cal, series_uid, recurrence_id, &fetched,
					   NULL, &error)) {
		g_free (series_uid);
		return fail ("get-occurrence", error);
	}

	occurrence = first_vevent (fetched);
	if (!occurrence) {
		g_printerr ("get-occurrence: what EDS handed back for %s holds no "
			    "VEVENT\n", series_uid);
		g_free (series_uid);
		g_object_unref (fetched);
		return 1;
	}

	g_free (series_uid);

	g_print ("occurrence-detached=%d\n", is_detached_instance (occurrence) ? 1 : 0);
	g_print ("occurrence-summary=%s\n", i_cal_component_get_summary (occurrence));
	g_print ("occurrence-definitions=%u\n", definition_count (fetched));
	functional_report_start ("occurrence", occurrence);
	report_zone_lookup ("occurrence", cal, occurrence);
	report_length ("occurrence", occurrence);

	g_object_unref (occurrence);
	g_object_unref (fetched);

	return 0;
}

static void
usage (const gchar *program)
{
	g_printerr ("usage: %s read <source-uid> <new-summary> <new-dtstart> "
		    "<new-dtend> <event-uid>...\n"
		    "       %s create <source-uid> <zone-file> <summary> "
		    "<dtstart> <duration>\n"
		    "       %s series <source-uid> <zone-file> <moved-zone-file> "
		    "<summary> <dtstart> <duration> <rrule> <recurrence-id> "
		    "<moved-dtstart>\n", program, program, program);
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
	const gchar *source_uid;
	const gchar *mode;
	gsize index;

	int status;

	/* The number of arguments of each mode. The check is here, not in each
	 * mode, because too few arguments would cause a read past the end of argv.
	 * `read` takes one or more event UIDs, so its count is a minimum. */
	static const struct {
		const gchar *name;
		int arguments;
		gboolean variadic;
	} modes[] = {
		{ "read", 7, TRUE },
		{ "create", 7, FALSE },
		{ "series", 11, FALSE },
	};

	if (argc < 3) {
		usage (argv[0]);
		return 2;
	}

	mode = argv[1];
	for (index = 0; index < G_N_ELEMENTS (modes); index++) {
		if (g_strcmp0 (mode, modes[index].name) == 0)
			break;
	}

	if (index == G_N_ELEMENTS (modes) ||
	    (modes[index].variadic ? argc < modes[index].arguments
				   : argc != modes[index].arguments)) {
		usage (argv[0]);
		return 2;
	}


	source_uid = argv[2];

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("source: no source with uid %s\n", source_uid);
		return 1;
	}

	/* (guint32) -1 is the EDS value for "do not wait for connected". This
	 * program cannot use the built-in wait: it blocks the only thread that
	 * would iterate the context that gets the notification, so the wait always
	 * expires. functional_report_connection_status below does the wait. See
	 * "Why the clients pass 'do not wait for connected'" in
	 * docs/functional-tests.md. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	/* Reported before the first request to the calendar, for the reason that
	 * the other calendar clients give: e_cal_client_connect_sync succeeds also
	 * when connect_sync of the backend failed. Without this, a calendar that
	 * the backend did not open looks the same as one that it did not mark
	 * writable. */
	functional_report_connection_status (source, 10);

	cal = E_CAL_CLIENT (client);
	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	if (g_strcmp0 (mode, "create") == 0)
		status = run_create (cal, argv[3], argv[4], argv[5], argv[6]);
	else if (g_strcmp0 (mode, "series") == 0)
		status = run_series (cal, argv[3], argv[4], argv[5], argv[6], argv[7],
				     argv[8], argv[9], argv[10]);
	else
		/* The first event UID is argv[6]; the rest follow it. */
		status = run_read (cal, argv[3], argv[4], argv[5], argc, argv, 6);

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return status;
}
