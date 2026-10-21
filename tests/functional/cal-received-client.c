/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Tests the scheduling rules for components that come in through
 * `e_cal_client_receive_objects`, with a real `evolution-calendar-factory`.
 * Evolution calls it when the user accepts an invitation in a mail, and when
 * the user imports a file.
 *
 * The program does these steps:
 * 1. It receives a REQUEST that names the user (Olga) as the organizer and a
 *    third address as a guest.
 * 2. It receives an invitation from Vera, accepted by Olga.
 * 3. It creates a meeting of Olga with Ida, and then receives a REQUEST with
 *    the same UID that adds a third address as a guest.
 *
 * `rust/crates/jmap-functional/tests/calendar-received.rs` runs this program
 * and does the assertions on the outbox of the mock. The program writes one
 * `key=value` line for each observation. It exits non-zero when a call fails
 * that must not fail.
 *
 *   usage: functional-cal-received-client <source-uid>
 */

#include <libecal/libecal.h>

#include "connection-status.h"

#define OLGA "mailto:olga@example.com"
#define VERA "mailto:vera@example.com"
#define IDA "mailto:ida@example.com"
/* Must match the addresses in calendar-received.rs. */
#define FIRST_STRANGER "mailto:first-stranger@example.net"
#define SECOND_STRANGER "mailto:second-stranger@example.net"

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* Returns an iTIP REQUEST with one event. */
static ICalComponent *
request (const gchar *uid,
	 const gchar *organizer,
	 const gchar *attendees)
{
	ICalComponent *component;
	gchar *icalendar;

	icalendar = g_strdup_printf (
		"BEGIN:VCALENDAR\r\n"
		"VERSION:2.0\r\n"
		"PRODID:-//functional//received//EN\r\n"
		"METHOD:REQUEST\r\n"
		"BEGIN:VEVENT\r\n"
		"UID:%s\r\n"
		"SUMMARY:Received\r\n"
		"DTSTART:20260922T110000Z\r\n"
		"DTEND:20260922T120000Z\r\n"
		"ORGANIZER:%s\r\n"
		"%s"
		"END:VEVENT\r\n"
		"END:VCALENDAR\r\n",
		uid, organizer, attendees);
	component = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	return component;
}

static gboolean
receive (ECalClient *client,
	 ICalComponent *component,
	 GError **error)
{
	gboolean ok;

	ok = e_cal_client_receive_objects_sync (client, component,
						E_CAL_OPERATION_FLAG_NONE, NULL, error);
	g_object_unref (component);

	return ok;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	ICalComponent *own;
	gchar *own_uid = NULL;

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

	/* Step 1. */
	if (!receive (E_CAL_CLIENT (client),
		      request ("forged@functional", OLGA,
			       "ATTENDEE;PARTSTAT=ACCEPTED:" OLGA "\r\n"
			       "ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:" FIRST_STRANGER "\r\n"),
		      &error))
		return fail ("receive-forged", error);
	g_print ("received-forged=1\n");

	/* Step 2. */
	if (!receive (E_CAL_CLIENT (client),
		      request ("invitation@functional", VERA,
			       "ATTENDEE;PARTSTAT=ACCEPTED:" VERA "\r\n"
			       "ATTENDEE;PARTSTAT=ACCEPTED;RSVP=TRUE:" OLGA "\r\n"),
		      &error))
		return fail ("receive-invitation", error);
	g_print ("received-invitation=1\n");

	/* Step 3. */
	own = i_cal_component_new_from_string (
		"BEGIN:VEVENT\r\n"
		"UID:own@functional\r\n"
		"SUMMARY:Own\r\n"
		"DTSTART:20260923T110000Z\r\n"
		"DTEND:20260923T120000Z\r\n"
		"ORGANIZER:" OLGA "\r\n"
		"ATTENDEE;PARTSTAT=ACCEPTED:" OLGA "\r\n"
		"ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:" IDA "\r\n"
		"END:VEVENT\r\n");
	if (!e_cal_client_create_object_sync (E_CAL_CLIENT (client), own,
					      E_CAL_OPERATION_FLAG_NONE, &own_uid, NULL, &error))
		return fail ("create-own", error);
	g_object_unref (own);
	g_print ("created-own=1\n");

	if (receive (E_CAL_CLIENT (client),
		     request ("own@functional", OLGA,
			      "ATTENDEE;PARTSTAT=ACCEPTED:" OLGA "\r\n"
			      "ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:" IDA "\r\n"
			      "ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:" SECOND_STRANGER "\r\n"),
		     &error)) {
		g_print ("own-changed=1\n");
	} else {
		g_print ("own-changed=0\n");
		g_print ("own-permission-denied=%d\n",
			 g_error_matches (error, E_CLIENT_ERROR, E_CLIENT_ERROR_PERMISSION_DENIED) ? 1 : 0);
		g_print ("own-message=%s\n", error ? error->message : "");
		g_clear_error (&error);
	}

	g_free (own_uid);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
