/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Tests `get_free_busy_sync` through a real `evolution-calendar-factory`.
 * The tests in `jmap-backend-cal/tests/{backend,ops}.rs` use only in-process
 * fixtures. This libecal client opens a calendar and calls
 * `e_cal_client_get_free_busy_sync()` for one address over a time window,
 * as the free/busy panel of the meeting editor does. It has no JMAP code.
 * `jmap-backend-cal/src/backend.rs` installs the
 * `ECalBackendSyncClass::get_free_busy_sync` slot, and
 * `jmap-cal-sync/src/freebusy.rs::CalSync::free_busy` sends
 * `Principal/query` and `Principal/getAvailability` to the server.
 *
 * `rust/crates/jmap-functional/tests/calendar-free-busy.rs` creates the
 * scratch XDG tree, the `.source` keyfile, the private D-Bus session and the
 * mock server, and it does all the assertions. This program writes one
 * `key=value` line on stdout for each observation. It exits non-zero when a
 * call fails.
 *
 *   usage: functional-cal-free-busy-client <source-uid> <user-email> \
 *              <window-start> <window-end>
 *
 * <window-start> and <window-end> are iCalendar UTC `DATE-TIME`s, for
 * example "20260901T080000Z". The client converts them to `time_t`.
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

/* Prints one line for each FREEBUSY property, in libical order. The period
 * text ("<start>/<end>") is read as text, so that no conversion changes it. */
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
	 * it where EDS looks for Basic auth. The mock tests do not set it. */
	{
		const gchar *seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

		if (seed_password && *seed_password &&
		    !e_source_store_password_sync (source, seed_password, TRUE, NULL, &error))
			return fail ("store-password", error);
	}

	/* Starts evolution-calendar-factory and keeps a backend alive.
	 * `get_free_busy_sync` runs only on a connected backend. */
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
