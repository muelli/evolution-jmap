/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Tests who tells the guests of a meeting, with a libecal client and a real
 * `evolution-calendar-factory`. The Evolution `itip-utils.c` writes no
 * invitation mail for a client with `create-messages`. Thus the reported
 * capabilities decide if Evolution or the server sends mail to the guests.
 *
 * Then the program does what the comp editor does for a meeting of the
 * user. It creates two meetings: one is sent, one has "do not send"
 * (`E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`). It removes the two meetings
 * with the same flags. Each meeting has a different guest, so the scheduling
 * outbox of the mock shows which change made the server send.
 *
 * Last, it creates a meeting with a guest that the server cannot reach. It
 * reports if the create failed, if the error is
 * `E_CLIENT_ERROR_INVALID_ARG`, and the message that Evolution shows. If the
 * create succeeds, the program removes the meeting, so the store is empty.
 *
 * `rust/crates/jmap-functional/tests/calendar-scheduling.rs` runs this
 * program and does the assertions. The program writes one `key=value` line
 * for each observation. It exits non-zero when a call fails.
 *
 *   usage: functional-cal-scheduling-client <source-uid>
 */

#include <libecal/libecal.h>

#include "connection-status.h"

#define ORGANIZER "mailto:olga@example.com"
#define SENT_GUEST "mailto:vera@example.com"
#define QUIET_GUEST "mailto:ida@example.com"
/* Must match UNREACHABLE_GUEST in calendar-scheduling.rs. */
#define UNREACHABLE_GUEST "urn:uuid:5f0c9a4e-0000-4000-8000-0000000000d1"

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

/* Creates a meeting of Olga with `guest`. Returns the uid from the backend,
 * or NULL with `error` set. */
static gchar *
create_meeting (ECalClient *client,
		const gchar *summary,
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
		"DTSTART:20260922T110000Z\r\n"
		"DTEND:20260922T120000Z\r\n"
		"ORGANIZER;CN=Olga:" ORGANIZER "\r\n"
		"ATTENDEE;CN=Olga;PARTSTAT=ACCEPTED:" ORGANIZER "\r\n"
		"ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:%s\r\n"
		"END:VEVENT\r\n",
		summary, summary, guest);
	component = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	if (!e_cal_client_create_object_sync (client, component, opflags, &uid, NULL, error))
		g_clear_pointer (&uid, g_free);

	g_object_unref (component);

	return uid;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	gchar *sent_uid;
	gchar *quiet_uid;
	gchar *unreachable_uid;

	if (argc != 2) {
		g_printerr ("usage: %s <source-uid>\n", argv[0]);
		return 2;
	}

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, argv[1]);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", argv[1]);
		return 1;
	}

	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	functional_report_connection_status (source, 10);

	report_capability (client, E_CAL_STATIC_CAPABILITY_CREATE_MESSAGES);
	report_capability (client, E_CAL_STATIC_CAPABILITY_SAVE_SCHEDULES);
	report_capability (client, E_CAL_STATIC_CAPABILITY_ITIP_SUPPRESS_ON_REMOVE_SUPPORTED);

	sent_uid = create_meeting (E_CAL_CLIENT (client), "Sent", SENT_GUEST,
				   E_CAL_OPERATION_FLAG_NONE, &error);
	if (!sent_uid)
		return fail ("create-sent", error);
	g_print ("created-sent=1\n");

	quiet_uid = create_meeting (E_CAL_CLIENT (client), "Quiet", QUIET_GUEST,
				    E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE, &error);
	if (!quiet_uid)
		return fail ("create-quiet", error);
	g_print ("created-quiet=1\n");

	if (!e_cal_client_remove_object_sync (E_CAL_CLIENT (client), quiet_uid, NULL,
					      E_CAL_OBJ_MOD_ALL,
					      E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE,
					      NULL, &error))
		return fail ("remove-quiet", error);
	g_print ("removed-quiet=1\n");

	if (!e_cal_client_remove_object_sync (E_CAL_CLIENT (client), sent_uid, NULL,
					      E_CAL_OBJ_MOD_ALL, E_CAL_OPERATION_FLAG_NONE,
					      NULL, &error))
		return fail ("remove-sent", error);
	g_print ("removed-sent=1\n");

	unreachable_uid = create_meeting (E_CAL_CLIENT (client), "Unreachable", UNREACHABLE_GUEST,
					  E_CAL_OPERATION_FLAG_NONE, &error);
	g_print ("unreachable-created=%d\n", unreachable_uid ? 1 : 0);
	if (unreachable_uid) {
		if (!e_cal_client_remove_object_sync (E_CAL_CLIENT (client), unreachable_uid, NULL,
						      E_CAL_OBJ_MOD_ALL, E_CAL_OPERATION_FLAG_NONE,
						      NULL, &error))
			return fail ("remove-unreachable", error);
	} else {
		g_print ("unreachable-invalid-arg=%d\n",
			 g_error_matches (error, E_CLIENT_ERROR, E_CLIENT_ERROR_INVALID_ARG) ? 1 : 0);
		g_print ("unreachable-message=%s\n", error ? error->message : "");
		g_clear_error (&error);
	}

	g_free (sent_uid);
	g_free (quiet_uid);
	g_free (unreachable_uid);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
