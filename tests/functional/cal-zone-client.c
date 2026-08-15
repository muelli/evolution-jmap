/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The third client half of the calendar functional test: an ordinary libecal
 * consumer that opens a calendar, reads events the *server* already held, and
 * reports the one thing it came for — what instant EDS and libical between them
 * say each event starts at, asked twice: of libical alone, and of the calendar
 * itself, which is the path Evolution takes. Then it saves the first of them
 * twice and looks again after each, which asks the same question of the same
 * event after an edit.
 *
 * The two edits are the two kinds there are, and they ask opposite things of one
 * save path. The first retypes the title: an edit with nothing to do with the
 * clock, so anything that happens to the clock is something the save did on its
 * own. The second drags the appointment to another wall clock and states its new
 * length the way Evolution's editor does — as a `DTEND`, in place of the
 * `DURATION` the mapping wrote — which is an edit about nothing *but* the clock,
 * and so the one that would show up a save that resends the zone whenever a
 * date-time is in the patch. Neither may cost the user the zone, and the zone is
 * the one thing here nothing outside the server can name.
 *
 * A program of its own rather than more arguments to cal-edit-client.c, which
 * asks what survives a save of the members no iCalendar line has room for:
 * handing it an event with no place, no conference and no attachment to retype
 * would mean either weakening what it insists on finding or seeding an event
 * with members this question has no use for. And the edits are made here rather
 * than in a second run because reading and saving are two halves of one question:
 * an appointment whose zone is resolvable until the first time the user touches
 * it is not usable, and the half that could not be measured without the other
 * half in the same run is exactly this one — the cache the read filled is the
 * cache the saves are made against.
 *
 * Several events in one run rather than one per run, because what is being
 * measured is a *contrast*: a zone the server defined against one it only named.
 * One run answers both from one cache filled by one refresh, so a difference
 * between the two answers is a difference between the events. Two runs would
 * share the session's XDG directories and so the meta backend's cache, which is
 * the one thing the harness deliberately starts empty.
 *
 * Like its twins it has no notion of what "correct" is: it reports what EDS told
 * it on stdout, one `key=value` line per observation, and exits non-zero the
 * moment a call fails. Every assertion belongs to
 * `rust/crates/jmap-functional/tests/calendar.rs`, which seeds the events, runs
 * this program and reads its output.
 *
 *   usage: functional-cal-zone-client <source-uid> <new-summary>
 *              <new-dtstart> <new-dtend> <event-uid>...
 *
 * The observations of the n-th event on the command line are prefixed `event-n`,
 * counting from one. Positional rather than keyed by the UID because the UID is
 * whatever the mock filed the event under, and a key beginning with a digit
 * makes for a poor observation name. What the first event looks like after the
 * rename is prefixed `saved`, and after the move `moved`.
 *
 * The two clocks are iCalendar `DATE-TIME` values with no zone on them — the
 * `TZID` the moved event keeps is the one already on its `DTSTART`, because the
 * whole question is whether an appointment that moves stays in the zone it was
 * in, and a client that named the zone itself would be answering it.
 */

#include <libecal/libecal.h>

#include "connection-status.h"
#include "event-start.h"

/* How long to wait for a seeded event to become gettable, and how often to ask.
 * cal-edit-client.c's EDIT_WAIT_TRIES, for the reason given there: the backend
 * schedules a refresh during the connect, and a get that arrives while it is
 * still running can be answered out of a cache the refresh has not filled yet. */
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

/* What EDS hands back from get_object: a bare VEVENT for an event with one
 * instance, or a VCALENDAR wrapping the instances when there are several.
 * cal-edit-client.c's first_vevent, and here it matters for a second reason —
 * a VTIMEZONE can only stand in the wrapper, so which shape EDS chose decides
 * whether a definition could have travelled with the event at all. */
static ICalComponent *
first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}

/* How many VTIMEZONEs stand beside the event EDS handed back.
 *
 * Reported rather than inferred from the instant, because the two failures it
 * tells apart are repaired in different places. An event whose start resolves
 * *and* carries no definition resolved out of libical's builtin table, which is
 * the bet an IANA name rests on; one that carries none and does not resolve is a
 * definition lost between the mapping and EDS's cache, which is this
 * repository's bug; and one that carries a definition and still does not resolve
 * is a VTIMEZONE libical refused, which is the mapping's drawing. The instant
 * alone cannot distinguish the last two, and they are not the same work. */
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

/* Ask EDS itself for the zone the event's DTSTART names, and report the instant
 * the start means once it is resolved against what came back.
 *
 * The measurement that matters, and the one functional_report_start cannot make.
 * A `TZID` is resolved by libical against the enclosing component's own
 * `VTIMEZONE`s and its builtin table, and neither holds a zone only a server can
 * name: EDS does not hand the definition back beside the event, it keeps it in the
 * calendar's own timezone store, which is what `e_cal_client_get_timezone_sync`
 * exists to read. That is how Evolution resolves a `TZID` too — its recurrence and
 * alarm machinery takes a lookup callback and this is what the callback calls — so
 * this is the consumer's path, and the bare libical answer beside it is a fact
 * about libical's table rather than about the account.
 *
 * Two observations. Whether the zone came back at all says the definition survived
 * the mapping, the marshalling, `ECalMetaBackend`'s gathering and the D-Bus hop;
 * the instant says the zone that came back is the zone the server described, which
 * a definition kept in part would not be.
 *
 * The time is rebuilt from the value on the line rather than taken from
 * i_cal_component_get_dtstart, because that call has already resolved the `TZID` —
 * to nothing — and a floating time carries no clock to reinterpret. Parsing the
 * value gives the wall clock the line states, which is what a zone applies to.
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
		 * zone answers with an error, and that answer is the observation. */
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

/* How long EDS says the event lasts, reported as both of the lines that can say
 * so.
 *
 * Two observations rather than one resolved length, because which line carries it
 * is itself the measurement: the mapping writes a `DURATION`, Evolution's editor
 * writes a `DTEND`, and RFC 5545 §3.6.1 allows only one of them on a component. So
 * the pair says which of the two shapes came back — and an event that came back
 * with neither, or with both, is a different bug from one that came back the wrong
 * length. */
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

/* Move the appointment: retype the wall clock its `DTSTART` states, and state how
 * long it now lasts as an end rather than as a length.
 *
 * The start is edited *in place* — the value of the property already there, with
 * every parameter on it left alone — which is the same content line Evolution
 * produces by a different route. Its editor builds an ECalComponentDateTime and
 * calls e_cal_component_set_dtstart, which writes the value and puts back the
 * `TZID` it was given; what reaches the backend either way is the same
 * `DTSTART;TZID=…:…`. Doing it in place here is not a shortcut but the point: this
 * program must not name the zone, or it would be supplying the answer the leg is
 * asking for.
 *
 * The end is a new property carrying the `TZID` read off that same `DTSTART`,
 * because an event whose two clocks are in different zones is a shape neither
 * Evolution writes nor this mapping can resolve. And the `DURATION` the mapping
 * drew is removed beside it: RFC 5545 §3.6.1 makes the two mutually exclusive, so
 * leaving it would hand the backend a component no reader has to make sense of —
 * and would leave it open which of the two the length came from.
 *
 * FALSE when the component states no start, or when libical would not read the
 * end back: either is this program failing to make the edit, not an observation. */
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

/* Ask for one event until EDS has it, or give up. cal-edit-client.c's
 * wait_for_event: a miss arrives as E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND rather
 * than as a failure of the call, so it is cleared and retried. */
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

/* Send an edited component back and report what EDS hands out for the event
 * afterwards, under `prefix`.
 *
 * The read-back is a plain get rather than a wait: the event is certainly in the
 * cache by now — a save the backend accepted is a save it has already stored — so
 * an answer of "not found" here is a finding rather than a race to sleep through.
 *
 * The same observations as the read loop makes, and in the same order, because
 * what the leg compares is exactly a before and an after; a shorter report after a
 * save would leave the assertions with nothing to hold the save to. 0 on success,
 * and the program's exit status otherwise. */
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

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	ECalClient *cal;
	ICalComponent *fetched;
	ICalComponent *event;
	const gchar *source_uid;
	const gchar *new_summary;
	const gchar *new_start;
	const gchar *new_end;
	const gchar *saved_uid;
	int status;
	int first;
	int index;

	if (argc < 6) {
		g_printerr ("usage: %s <source-uid> <new-summary> <new-dtstart> "
			    "<new-dtend> <event-uid>...\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	new_summary = argv[2];
	new_start = argv[3];
	new_end = argv[4];
	/* The first event on the command line, and the one the saves are made to. */
	first = 5;
	saved_uid = argv[first];

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("source: no source with uid %s\n", source_uid);
		return 1;
	}

	/* (guint32) -1 is EDS's "do not wait for connected". A program shaped like
	 * this one cannot use the built-in wait — it blocks the only thread that
	 * would iterate the context the notification arrives on, so it always
	 * expires — and waiting properly is what functional_report_connection_status
	 * below is for. See "Why the clients pass 'do not wait for connected'" in
	 * docs/functional-tests.md. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	/* Reported before anything is asked of the calendar, for the reason its
	 * twins give: e_cal_client_connect_sync succeeds even when the backend's
	 * connect_sync failed, so a calendar the backend never opened looks from
	 * here exactly like one it opened and forgot to claim writable. */
	functional_report_connection_status (source, 10);

	cal = E_CAL_CLIENT (client);
	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

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
		/* The VEVENT rather than what wrapped it, which is safe because libical
		 * resolves a TZID against the component *and its parents*: first_vevent
		 * hands back a child of `fetched` with its parent link intact, so a
		 * VTIMEZONE standing in the VCALENDAR is still in reach from here. */
		functional_report_start (prefix, event);
		report_zone_lookup (prefix, cal, event);
		report_length (prefix, event);

		g_free (prefix);
		g_object_unref (event);
		g_object_unref (fetched);
	}

	/* And the first save. The event is asked for again rather than kept from the
	 * loop above, which costs one call to a cache that certainly holds it by now
	 * and keeps the loop's job to reporting.
	 *
	 * The title is retyped through i_cal_component_set_summary, which replaces the
	 * value of the SUMMARY already there and leaves everything around it alone —
	 * so what goes back is the component EDS handed over with one line rewritten,
	 * which is what Evolution's appointment editor sends. Nothing here touches the
	 * DTSTART: the point of this save is that an edit which has nothing to do with
	 * the zone does not cost the user the zone, and an edit that restated the
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

	/* And the second, which is the edit about the clock and nothing else. Made to
	 * the component EDS hands back *now* rather than to the one edited above,
	 * because that is the component a user has in front of them: the save the
	 * backend just made redrew the event from what the server holds, and an edit
	 * built on the older copy would be sending back a line the first save may
	 * already have changed. */
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

	if (status != 0)
		return status;

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
