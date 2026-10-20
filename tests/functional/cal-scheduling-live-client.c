/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The organizer side of a meeting against a real server with two accounts.
 * It makes the calls of the Evolution comp editor through a real
 * `evolution-calendar-factory`.
 *
 * `cal-scheduling-client.c` makes the same calls against the mock, which
 * only records what it must send. Here the guest is a second account on the
 * same server.
 * `rust/crates/jmap-functional/tests/live-stalwart-calendar-scheduling.rs`
 * reads the calendar and mailbox of the guest to see the delivery. It runs
 * this program two times and does all the assertions:
 *
 *   create: reports the three capabilities. Then creates two meetings with
 *   <organizer> as organizer and <guest> as attendee: one is sent, one has
 *   "do not send" (`E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`).
 *
 *   settle: runs in a new scratch tree, so its first connect lists the full
 *   calendar again (see `cal-changes-client.c`) and sees the changes of the
 *   guest. Reports the PARTSTAT of the guest on the sent meeting. Then
 *   removes the two meetings with the same flags as at creation.
 *
 * The program writes one `key=value` line for each observation. It exits
 * non-zero when a call fails.
 *
 *   usage: functional-cal-scheduling-live-client create <source-uid>
 *              <organizer> <guest> <sent-summary> <quiet-summary>
 *          functional-cal-scheduling-live-client settle <source-uid>
 *              <guest> <sent-summary> <quiet-summary>
 */

#include <string.h>

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

static void
report_capability (EClient *client,
		   const gchar *capability)
{
	g_print ("%s=%d\n", capability, e_client_check_capability (client, capability) ? 1 : 0);
}

/* Creates a meeting of `organizer` with `guest`. Returns the uid from the
 * backend, or NULL with `error` set. */
static gchar *
create_meeting (ECalClient *client,
		const gchar *summary,
		const gchar *organizer,
		const gchar *guest,
		ECalOperationFlags opflags,
		GError **error)
{
	ICalComponent *component;
	gchar *icalendar;
	gchar *uid = NULL;

	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:%s@functional\r\n"
		"SUMMARY:%s\r\n"
		"DTSTART:20261022T110000Z\r\n"
		"DTEND:20261022T120000Z\r\n"
		"ORGANIZER;CN=Organizer:mailto:%s\r\n"
		"ATTENDEE;CN=Organizer;PARTSTAT=ACCEPTED:mailto:%s\r\n"
		"ATTENDEE;CN=Guest;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:%s\r\n"
		"END:VEVENT\r\n",
		summary, summary, organizer, organizer, guest);
	component = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	if (!e_cal_client_create_object_sync (client, component, opflags, &uid, NULL, error))
		g_clear_pointer (&uid, g_free);

	g_object_unref (component);

	return uid;
}

/* The component with the title `summary` in `components`, or NULL. Not a
 * new reference. */
static ICalComponent *
find_by_summary (GSList *components,
		 const gchar *summary)
{
	GSList *link;

	for (link = components; link; link = link->next) {
		ICalComponent *component = link->data;

		if (g_strcmp0 (i_cal_component_get_summary (component), summary) == 0)
			return component;
	}

	return NULL;
}

/* The PARTSTAT of the attendee `mailto:<guest>` in iCalendar text, or "none"
 * when `component` has no such attendee. */
static gchar *
guest_partstat (ICalComponent *component,
		const gchar *guest)
{
	gchar *wanted = g_strconcat ("mailto:", guest, NULL);
	gchar *partstat = NULL;
	ICalProperty *property;

	property = i_cal_component_get_first_property (component, I_CAL_ATTENDEE_PROPERTY);
	while (property && !partstat) {
		ICalProperty *next;
		const gchar *address = i_cal_property_get_attendee (property);

		if (address && g_ascii_strcasecmp (address, wanted) == 0) {
			ICalParameter *parameter;

			/* Read the text "PARTSTAT=ACCEPTED" and keep the value. */
			parameter = i_cal_property_get_first_parameter (property, I_CAL_PARTSTAT_PARAMETER);
			if (parameter) {
				gchar *written = i_cal_parameter_as_ical_string (parameter);
				const gchar *value = written ? strchr (written, '=') : NULL;

				partstat = g_strdup (value ? value + 1 : "");
				g_free (written);
				g_object_unref (parameter);
			} else {
				partstat = g_strdup ("NEEDS-ACTION");
			}
		}

		next = i_cal_component_get_next_property (component, I_CAL_ATTENDEE_PROPERTY);
		g_object_unref (property);
		property = next;
	}
	g_clear_object (&property);
	g_free (wanted);

	return partstat ? partstat : g_strdup ("none");
}

static int
create (ECalClient *client,
	char **argv)
{
	GError *error = NULL;
	const gchar *organizer = argv[0];
	const gchar *guest = argv[1];
	gchar *uid;

	report_capability (E_CLIENT (client), E_CAL_STATIC_CAPABILITY_CREATE_MESSAGES);
	report_capability (E_CLIENT (client), E_CAL_STATIC_CAPABILITY_SAVE_SCHEDULES);
	report_capability (E_CLIENT (client), E_CAL_STATIC_CAPABILITY_ITIP_SUPPRESS_ON_REMOVE_SUPPORTED);

	uid = create_meeting (client, argv[2], organizer, guest, E_CAL_OPERATION_FLAG_NONE, &error);
	if (!uid)
		return fail ("create-sent", error);
	g_print ("created-sent=1\n");
	g_free (uid);

	uid = create_meeting (client, argv[3], organizer, guest,
			      E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE, &error);
	if (!uid)
		return fail ("create-quiet", error);
	g_print ("created-quiet=1\n");
	g_free (uid);

	return 0;
}

static int
remove_meeting (ECalClient *client,
		ICalComponent *component,
		ECalOperationFlags opflags,
		const gchar *step)
{
	GError *error = NULL;

	if (!e_cal_client_remove_object_sync (client, i_cal_component_get_uid (component), NULL,
					      E_CAL_OBJ_MOD_ALL, opflags, NULL, &error))
		return fail (step, error);
	g_print ("%s=1\n", step);

	return 0;
}

static int
settle (ECalClient *client,
	char **argv)
{
	GError *error = NULL;
	const gchar *guest = argv[0];
	GSList *components = NULL;
	ICalComponent *sent;
	ICalComponent *quiet;
	gchar *partstat;
	int status = 0;

	if (!e_cal_client_get_object_list_sync (client, "#t", &components, NULL, &error))
		return fail ("query", error);

	sent = find_by_summary (components, argv[1]);
	quiet = find_by_summary (components, argv[2]);
	g_print ("found-sent=%d\nfound-quiet=%d\n", sent ? 1 : 0, quiet ? 1 : 0);

	if (sent) {
		partstat = guest_partstat (sent, guest);
		g_print ("guest-partstat=%s\n", partstat);
		g_free (partstat);
	}

	if (quiet)
		status = remove_meeting (client, quiet, E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE,
					 "removed-quiet");
	if (sent && !status)
		status = remove_meeting (client, sent, E_CAL_OPERATION_FLAG_NONE, "removed-sent");

	g_slist_free_full (components, g_object_unref);

	return status;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	const gchar *seed_password;
	gboolean creating;
	int status;

	creating = argc == 7 && g_strcmp0 (argv[1], "create") == 0;
	if (!creating && !(argc == 6 && g_strcmp0 (argv[1], "settle") == 0)) {
		g_printerr ("usage: %s create <source-uid> <organizer> <guest> <sent-summary> <quiet-summary>\n"
			    "       %s settle <source-uid> <guest> <sent-summary> <quiet-summary>\n",
			    argv[0], argv[0]);
		return 2;
	}

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, argv[2]);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", argv[2]);
		return 1;
	}

	seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");
	if (seed_password && *seed_password &&
	    !e_source_store_password_sync (source, seed_password, TRUE, NULL, &error))
		return fail ("store-password", error);

	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	functional_report_connection_status (source, 10);

	if (creating)
		status = create (E_CAL_CLIENT (client), argv + 3);
	else
		status = settle (E_CAL_CLIENT (client), argv + 3);

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return status;
}
