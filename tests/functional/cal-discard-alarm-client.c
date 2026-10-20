/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Dismisses a reminder as the Evolution reminder watcher does, through a
 * real `evolution-calendar-factory`. The program finds the event by its
 * summary and gets the alarm uid of its reminder. Then it calls
 * `e_cal_client_discard_alarm_sync()` with the recurrence id of the
 * occurrence (or none) and no operation flags. In EDS 3.52 and later,
 * `e_reminder_watcher_dismiss_one_sync()` makes this call and ignores the
 * result. Evolution shows no failure, so this program reports it.
 *
 * `rust/crates/jmap-functional/tests/calendar-discard-alarm.rs` creates the
 * scratch XDG tree, the `.source` keyfile, the private D-Bus session and the
 * mock server, and it does all the assertions. The program writes one
 * `key=value` line on stdout for each observation. It exits non-zero only
 * when a step before the dismissal fails.
 *
 *   usage: functional-cal-discard-alarm-client <source-uid> <summary> [<rid>]
 */

#include <libecal/libecal.h>

#include "connection-status.h"

/* The maximum wait for the refresh of the connect to get the event from the
 * server. A long wait costs time only on a failed run. */
#define FIND_TIMEOUT_SECONDS 10

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* The cached event with the summary `summary`, or NULL after the timeout. */
static ECalComponent *
find_event (ECalClient *cal,
	    const gchar *summary,
	    GError **error)
{
	gint64 deadline = g_get_monotonic_time () + FIND_TIMEOUT_SECONDS * G_USEC_PER_SEC;

	do {
		GSList *components = NULL, *link;
		ECalComponent *found = NULL;

		if (!e_cal_client_get_object_list_as_comps_sync (cal, "#t", &components, NULL, error))
			return NULL;

		for (link = components; link && !found; link = link->next) {
			ECalComponent *component = link->data;
			ECalComponentText *text = e_cal_component_get_summary (component);

			if (text && g_strcmp0 (e_cal_component_text_get_value (text), summary) == 0)
				found = g_object_ref (component);
			e_cal_component_text_free (text);
		}

		g_slist_free_full (components, g_object_unref);
		if (found)
			return found;

		g_usleep (G_USEC_PER_SEC / 4);
	} while (g_get_monotonic_time () < deadline);

	return NULL;
}

/* The ACKNOWLEDGED of the first VALARM of `uid` in EDS now, or an empty
 * string. */
static gchar *
read_back_acknowledged (ECalClient *cal,
			const gchar *uid)
{
	ICalComponent *icomp = NULL;
	ICalComponent *valarm;
	ICalProperty *property;
	gchar *value = NULL;

	if (!e_cal_client_get_object_sync (cal, uid, NULL, &icomp, NULL, NULL))
		return g_strdup ("");

	valarm = i_cal_component_get_first_component (icomp, I_CAL_VALARM_COMPONENT);
	if (valarm) {
		property = i_cal_component_get_first_property (valarm, I_CAL_ACKNOWLEDGED_PROPERTY);
		if (property) {
			value = i_cal_property_get_value_as_string (property);
			g_object_unref (property);
		}
		g_object_unref (valarm);
	}
	g_object_unref (icomp);

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
	ECalComponent *event;
	GSList *alarm_uids;
	gchar *acknowledged;
	const gchar *source_uid;
	const gchar *summary;
	const gchar *rid;
	const gchar *uid;
	const gchar *auid;
	gboolean discarded;

	if (argc != 3 && argc != 4) {
		g_printerr ("usage: %s <source-uid> <summary> [<rid>]\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	summary = argv[2];
	rid = argc == 4 ? argv[3] : NULL;

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	functional_report_connection_status (source, 10);

	event = find_event (cal, summary, &error);
	if (!event)
		return fail ("find", error);

	uid = e_cal_component_get_uid (event);
	alarm_uids = e_cal_component_get_alarm_uids (event);
	g_print ("alarms=%u\n", g_slist_length (alarm_uids));
	if (!alarm_uids) {
		g_printerr ("find: the event carries no alarm\n");
		return 1;
	}
	auid = alarm_uids->data;

	discarded = e_cal_client_discard_alarm_sync (cal, uid, rid, auid,
						     E_CAL_OPERATION_FLAG_NONE, NULL, &error);
	g_print ("discarded=%d\n", discarded ? 1 : 0);
	if (error) {
		g_print ("discard-error=%s %d %s\n", g_quark_to_string (error->domain),
			 error->code, error->message);
		g_clear_error (&error);
	}

	acknowledged = read_back_acknowledged (cal, uid);
	g_print ("read-back-acknowledged=%s\n", acknowledged);
	g_free (acknowledged);

	g_slist_free_full (alarm_uids, g_free);
	g_object_unref (event);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
