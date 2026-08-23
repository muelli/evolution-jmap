/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Track E Path A's `get_free_busy_sync`, proven through a real, running
 * `evolution-calendar-factory` rather than the in-process fixtures
 * `jmap-backend-cal/tests/{backend,ops}.rs` stop at: an ordinary libecal
 * consumer that opens a calendar (which is what makes the factory dlopen
 * libecalbackendjmap.so and keeps a live `ECalMetaBackend` instance
 * watching this source), then asks the same question the meeting
 * scheduler's free/busy panel asks — `e_cal_client_get_free_busy_sync()`
 * for one address over a window — the way `cal-color-client.c` is the
 * missing FFI link for D2's colour push. It knows nothing about JMAP or
 * the mock: what it drives is EDS's own `ECalBackendSyncClass::
 * get_free_busy_sync` slot, which `jmap-backend-cal/src/backend.rs`
 * installs and `jmap-cal-sync/src/freebusy.rs::CalSync::free_busy` answers
 * with a `Principal/query` + `Principal/getAvailability` round trip to the
 * server.
 *
 * Everything around this program — the scratch XDG tree, the `.source`
 * keyfile, the private D-Bus session, the mock server and every assertion —
 * belongs to `rust/crates/jmap-functional/tests/calendar-free-busy.rs`,
 * which runs it and reads its output. So this file has no test framework in
 * it and no notion of what "correct" is: it reports what EDS told it on
 * stdout, one `key=value` line per observation, and exits non-zero the
 * moment a call fails.
 *
 *   usage: functional-cal-free-busy-client <source-uid> <user-email> \
 *              <window-start> <window-end>
 *
 * <window-start>/<window-end> are iCalendar UTC `DATE-TIME`s
 * (e.g. "20260901T080000Z"), parsed with `i_cal_time_new_from_string()` and
 * converted to the `time_t`s `e_cal_client_get_free_busy_sync()` takes —
 * the Rust side states the window that way rather than as a raw `time_t`
 * so the one window literal it and this client share reads the same in
 * both languages.
 */

#include <libecal/libecal.h>

#include "connection-status.h"

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

static time_t
parse_utc (const gchar *text)
{
	ICalTime *time;
	time_t result;

	time = i_cal_time_new_from_string (text);
	if (!time)
		return (time_t) -1;

	result = i_cal_time_as_timet (time);
	g_object_unref (time);

	return result;
}

/* One line per FREEBUSY property the server's answer carried, in the order
 * libical stored them. `i_cal_property_get_value_as_string()` reads the
 * period text ("<start>/<end>") verbatim rather than through `ICalPeriod`
 * accessors, the same reasoning `cal-client.c::joined_values` gives for
 * every other property this tree reads as text: it reports what the
 * component states, not what a second layer of conversion made of it. */
static void
report_free_busy_periods (ICalComponent *component)
{
	ICalProperty *property;
	guint index = 0;

	property = i_cal_component_get_first_property (component, I_CAL_FREEBUSY_PROPERTY);
	while (property) {
		ICalProperty *next;
		ICalParameter *fbtype_param;
		gchar *value = i_cal_property_get_value_as_string (property);

		g_print ("free-busy-period-%u=%s\n", index, value ? value : "");
		g_free (value);

		fbtype_param = i_cal_property_get_first_parameter (property, I_CAL_FBTYPE_PARAMETER);
		if (fbtype_param) {
			ICalParameterFbtype fbtype = i_cal_parameter_get_fbtype (fbtype_param);

			g_print ("free-busy-fbtype-%u=%s\n", index,
				 fbtype == I_CAL_FBTYPE_BUSY ? "BUSY" :
				 fbtype == I_CAL_FBTYPE_FREE ? "FREE" :
				 fbtype == I_CAL_FBTYPE_BUSYUNAVAILABLE ? "BUSY-UNAVAILABLE" :
				 fbtype == I_CAL_FBTYPE_BUSYTENTATIVE ? "BUSY-TENTATIVE" : "OTHER");
			g_object_unref (fbtype_param);
		}

		next = i_cal_component_get_next_property (component, I_CAL_FREEBUSY_PROPERTY);
		g_object_unref (property);
		property = next;
		index++;
	}

	g_print ("free-busy-period-count=%u\n", index);
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	const gchar *source_uid;
	const gchar *user_email;
	time_t window_start;
	time_t window_end;
	GSList *users = NULL;
	GSList *out_freebusy = NULL;

	if (argc != 5) {
		g_printerr ("usage: %s <source-uid> <user-email> <window-start> <window-end>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	user_email = argv[2];
	window_start = parse_utc (argv[3]);
	window_end = parse_utc (argv[4]);
	if (window_start == (time_t) -1 || window_end == (time_t) -1) {
		g_printerr ("could not parse the free/busy window as an iCalendar UTC DATE-TIME\n");
		return 2;
	}

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

	/* Activates evolution-calendar-factory and keeps a live backend
	 * instance running for the rest of this process's life —
	 * `get_free_busy_sync` only ever fires on a backend that is connected
	 * and holding the JMAP client this test is actually exercising. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	functional_report_connection_status (source, 10);

	users = g_slist_append (users, (gpointer) user_email);
	if (!e_cal_client_get_free_busy_sync (E_CAL_CLIENT (client), window_start, window_end,
					      users, &out_freebusy, NULL, &error)) {
		g_slist_free (users);
		return fail ("get-free-busy", error);
	}
	g_slist_free (users);

	g_print ("free-busy-component-count=%u\n", g_slist_length (out_freebusy));

	if (out_freebusy) {
		ECalComponent *component = E_CAL_COMPONENT (out_freebusy->data);
		ICalComponent *icalcomp = e_cal_component_get_icalcomponent (component);
		ICalProperty *attendee_prop = i_cal_component_get_first_property (icalcomp, I_CAL_ATTENDEE_PROPERTY);

		if (attendee_prop) {
			g_print ("free-busy-attendee=%s\n", i_cal_property_get_attendee (attendee_prop));
			g_object_unref (attendee_prop);
		}

		report_free_busy_periods (icalcomp);
	}

	g_slist_free_full (out_freebusy, g_object_unref);

	g_print ("done=1\n");

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
