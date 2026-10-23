/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The second client half of the calendar functional test. A usual libecal
 * consumer opens a calendar and reads an event that the *server* already has.
 * It reports the start that EDS gives, changes the places, the document and
 * the picture of the event, and saves the event.
 *
 * This is a separate program, not a phase of cal-client.c, which creates all
 * its events. cal-client.c examines what the server gets when the user makes
 * an appointment. This program examines the *round trip*: a JSCalendar event
 * that the mapping can only partly show goes to iCalendar, through the
 * ECalMetaBackend cache, to a libecal consumer, and back through a save. Only
 * an event that the server made can test this, and the series-splitting code
 * of cal-client.c is not used on that path.
 *
 * Like cal-client.c, it does not judge: it writes one `key=value` line on
 * stdout for each observation, and exits non-zero when a call fails.
 * `rust/crates/jmap-functional/tests/calendar.rs` seeds the event, runs this
 * program and asserts on its output.
 *
 *   usage: functional-cal-edit-client <source-uid> <event-uid> <new-location>
 *                                    <new-conference-uri> <old-attach-uri>
 *                                    <new-attach-uri> <old-image-uri>
 *                                    <new-image-uri>
 *
 * The arguments identify the attachment and the picture to change by their
 * current address, not by position or by the mapping key. An event can have
 * many of each (RFC 5545 §3.8.1.1, RFC 7986 §5.10). A user selects them by
 * address, and this program needs no knowledge of the mapping.
 */

#define LIBICAL_GLIB_UNSTABLE_API 1

#include <libecal/libecal.h>

#include "connection-status.h"
#include "event-start.h"
#include "test-util.h"

/* The parameter (jmap-ical X_JMAP_KEY) that tells which entry of the server
 * map a line comes from. A save patches `locations/<key>/name`: if EDS loses
 * the key, the save cannot find the entry that the user changed. */
#define JMAP_KEY_PARAMETER "X-JMAP-KEY"

/* The tries and interval for the get of the seeded event, as for
 * EDIT_WAIT_TRIES in book-client.c. For an event that is not in its cache,
 * ECalMetaBackend asks the backend to load it, thus the first try usually
 * succeeds. But the connect also starts a refresh, and during the refresh the
 * cache can be incomplete. Polling prevents a random failure. The total is ten
 * seconds, less than the CTest timeout, thus a missing event fails the test. */
#define EDIT_WAIT_TRIES 200
#define EDIT_WAIT_INTERVAL_US 50000

/* Return the value of one non-standard parameter of a property, or the empty
 * string if there is none.
 *
 * libical keeps each parameter without an enum as an I_CAL_X_PARAMETER, with
 * the name from i_cal_parameter_get_xname. Thus this looks at all of them. The
 * comparison ignores case: RFC 5545 §3.1 makes parameter names
 * case-insensitive, and the meta backend cache wrote this value.
 *
 * The empty string (not NULL) gives an observation for a lost parameter, not
 * a missing line in the output. */
static gchar *
x_parameter (ICalProperty *property,
	     const gchar *name)
{
	ICalParameter *parameter;

	if (!property)
		return g_strdup ("");

	parameter = i_cal_property_get_first_parameter (property, I_CAL_X_PARAMETER);
	while (parameter) {
		ICalParameter *next;
		const gchar *xname = i_cal_parameter_get_xname (parameter);

		if (xname && g_ascii_strcasecmp (xname, name) == 0) {
			gchar *value = g_strdup (i_cal_parameter_get_xvalue (parameter));

			g_object_unref (parameter);

			return value ? value : g_strdup ("");
		}

		next = i_cal_property_get_next_parameter (property, I_CAL_X_PARAMETER);
		g_object_unref (parameter);
		parameter = next;
	}

	return g_strdup ("");
}

/* Count the properties of one kind. A line that the round trip *added* is a
 * different bug from a changed value. Only the count shows the difference. */
static guint
property_count (ICalComponent *component,
		ICalPropertyKind kind)
{
	ICalProperty *property;
	guint count = 0;

	property = i_cal_component_get_first_property (component, kind);
	while (property) {
		ICalProperty *next;

		count++;
		next = i_cal_component_get_next_property (component, kind);
		g_object_unref (property);
		property = next;
	}

	return count;
}

/* The CONFERENCE property kind, which the generated ICalPropertyKind of
 * libical-glib 3.0 does not name.
 *
 * The glib binding with EDS 3.52 has no I_CAL_CONFERENCE_PROPERTY (RFC 7986
 * §5.11). The libical C enum has ICAL_CONFERENCE_PROPERTY, and the parser
 * makes it. The cast changes only the name, not the value:
 * i-cal-derived-property.h defines each I_CAL_* as the ICAL_* member.
 *
 * Thus a libical-glib client has no named constant to *offer* a conference.
 * This is one reason that Evolution 3.52 has no UI for it. */
#define CONFERENCE_PROPERTY ((ICalPropertyKind) ICAL_CONFERENCE_PROPERTY)

/* The IMAGE property kind (RFC 7986 §5.10). The generated ICalPropertyKind
 * also does not name it. The same cast applies. */
#define IMAGE_PROPERTY ((ICalPropertyKind) ICAL_IMAGE_PROPERTY)

/* Report the LOCATION and the CONFERENCE that EDS has.
 *
 * Each place has more than one observation. The value shows that the place
 * arrived. The key shows that a save can find the server entry, which is
 * necessary for the patch design. The count shows that the round trip did not
 * remove or add a line. The LABEL of the conference shows that a standard
 * parameter also stayed. */
static void
report_places (const gchar *prefix,
	       ICalComponent *event)
{
	ICalProperty *location;
	ICalProperty *conference;
	gchar *key;
	gchar *value;

	location = i_cal_component_get_first_property (event, I_CAL_LOCATION_PROPERTY);
	value = location ? i_cal_property_get_value_as_string (location) : g_strdup ("");
	key = x_parameter (location, JMAP_KEY_PARAMETER);
	g_print ("%s-location=%s\n", prefix, value ? value : "");
	g_print ("%s-location-key=%s\n", prefix, key);
	g_print ("%s-locations=%u\n", prefix,
		 property_count (event, I_CAL_LOCATION_PROPERTY));
	g_free (value);
	g_free (key);
	g_clear_object (&location);

	conference = i_cal_component_get_first_property (event, CONFERENCE_PROPERTY);
	value = conference ? i_cal_property_get_value_as_string (conference) : g_strdup ("");
	key = x_parameter (conference, JMAP_KEY_PARAMETER);
	g_print ("%s-conference=%s\n", prefix, value ? value : "");
	g_print ("%s-conference-key=%s\n", prefix, key);
	g_print ("%s-conferences=%u\n", prefix,
		 property_count (event, CONFERENCE_PROPERTY));
	g_free (value);
	g_free (key);

	if (conference) {
		ICalParameter *label = i_cal_property_get_first_parameter (
			conference, I_CAL_LABEL_PARAMETER);

		g_print ("%s-conference-label=%s\n", prefix,
			 label ? i_cal_parameter_get_label (label) : "");
		g_clear_object (&label);
		g_object_unref (conference);
	} else {
		g_print ("%s-conference-label=\n", prefix);
	}
}

/* Report each ATTACH line that EDS has: one group of observations for each
 * line, numbered from one in component order.
 *
 * The value type makes this a different question. libical keeps a LOCATION
 * and a CONFERENCE as the string of the line. An ATTACH (RFC 5545 §3.8.1.1)
 * has its own value type, thus the parser makes an icalattach and the
 * parameters are next to a new value. This part examines if the X-JMAP-KEY
 * stays through that and through the ECalMetaBackend cache.
 *
 * Report all lines, not only the first. With one document, a save that lost
 * the key finds the only server entry. With two, it changes the entry that
 * the mapping guessed. The number in each name only separates the groups:
 * the harness finds the server entry by the key.
 *
 * Read the address with i_cal_property_get_attach, not
 * i_cal_property_get_value_as_string. This is the URL that libical parsed,
 * thus a value that the round trip wrote differently shows as a difference.
 *
 * Also report FMTTYPE and SIZE, which have a libical enum. A cache that keeps
 * only known parameters keeps these two and drops the key. Thus the three
 * values tell "unknown parameters lost" from "line lost".
 */
static void
report_resource (const gchar *prefix,
		 ICalComponent *event)
{
	ICalProperty *attach;
	guint index = 0;

	g_print ("%s-attaches=%u\n", prefix, property_count (event, I_CAL_ATTACH_PROPERTY));

	attach = i_cal_component_get_first_property (event, I_CAL_ATTACH_PROPERTY);
	while (attach) {
		ICalProperty *next;
		ICalAttach *value;
		ICalParameter *parameter;
		const gchar *url;
		gchar *key;

		index++;

		value = i_cal_property_get_attach (attach);
		url = value ? i_cal_attach_get_url (value) : NULL;
		g_print ("%s-attach-%u=%s\n", prefix, index, url ? url : "");
		g_clear_object (&value);

		key = x_parameter (attach, JMAP_KEY_PARAMETER);
		g_print ("%s-attach-%u-key=%s\n", prefix, index, key);
		g_free (key);

		parameter = i_cal_property_get_first_parameter (
			attach, I_CAL_FMTTYPE_PARAMETER);
		g_print ("%s-attach-%u-fmttype=%s\n", prefix, index,
			 parameter ? i_cal_parameter_get_fmttype (parameter) : "");
		g_clear_object (&parameter);

		parameter = i_cal_property_get_first_parameter (
			attach, I_CAL_SIZE_PARAMETER);
		g_print ("%s-attach-%u-size=%s\n", prefix, index,
			 parameter ? i_cal_parameter_get_size (parameter) : "");
		g_clear_object (&parameter);

		next = i_cal_component_get_next_property (event, I_CAL_ATTACH_PROPERTY);
		g_object_unref (attach);
		attach = next;
	}
}

/* Return the text after the `=` when libical writes the parameter, or the
 * empty string if there is no parameter.
 *
 * libical-glib has no function that changes an enum value of a parameter into
 * its iCalendar text. i_cal_parameter_get_display returns an
 * ICalParameterDisplay, and its number depends on the enum layout. The text
 * that libical writes is the text that a calendar reader sees.
 *
 * The empty string for no parameter has the same reason as in x_parameter. */
static gchar *
parameter_value (ICalParameter *parameter)
{
	gchar *written;
	const gchar *equals;
	gchar *value;

	if (!parameter)
		return g_strdup ("");

	written = i_cal_parameter_as_ical_string (parameter);
	if (!written)
		return g_strdup ("");

	equals = strchr (written, '=');
	value = g_strdup (equals ? equals + 1 : "");
	g_free (written);

	return value;
}

/* Report each IMAGE line that EDS has, numbered as in report_resource.
 *
 * RFC 8984 §4.2.7 keeps in one `links` map what iCalendar puts in two
 * properties: a document is an ATTACH, a picture is an IMAGE (RFC 7986
 * §5.10). jmap-ical uses the `icon` rel to separate them. A picture that came
 * back as an ATTACH passes all report_resource observations, thus report the
 * IMAGE lines separately.
 *
 * Read the address as the value string: here no other method is available.
 * The §5.10 grammar makes VALUE=URI REQUIRED for a URI, thus the mapping
 * writes it. Then libical parses an ICAL_URI_VALUE, not the ICAL_ATTACH_VALUE
 * of an ATTACH. i_cal_property_get_attach on this property does not return
 * NULL: it reads the URI as an icalattach and *crashes* (measured with
 * libical 3.0.17).
 *
 * Also report DISPLAY: it is the only JSCalendar Link member with its own
 * standard parameter. §6.1 tells a reader to show no image for an unknown
 * DISPLAY value, thus a changed value is worse than a lost one. Report
 * FMTTYPE too, but not SIZE: §5.10 does not allow SIZE on this property. */
static void
report_pictures (const gchar *prefix,
		 ICalComponent *event)
{
	ICalProperty *image;
	guint index = 0;

	g_print ("%s-images=%u\n", prefix, property_count (event, IMAGE_PROPERTY));

	image = i_cal_component_get_first_property (event, IMAGE_PROPERTY);
	while (image) {
		ICalProperty *next;
		ICalParameter *parameter;
		gchar *value;

		index++;

		value = i_cal_property_get_value_as_string (image);
		g_print ("%s-image-%u=%s\n", prefix, index, value ? value : "");
		g_free (value);

		value = x_parameter (image, JMAP_KEY_PARAMETER);
		g_print ("%s-image-%u-key=%s\n", prefix, index, value);
		g_free (value);

		parameter = i_cal_property_get_first_parameter (
			image, I_CAL_FMTTYPE_PARAMETER);
		g_print ("%s-image-%u-fmttype=%s\n", prefix, index,
			 parameter ? i_cal_parameter_get_fmttype (parameter) : "");
		g_clear_object (&parameter);

		parameter = i_cal_property_get_first_parameter (
			image, I_CAL_DISPLAY_PARAMETER);
		value = parameter_value (parameter);
		g_print ("%s-image-%u-display=%s\n", prefix, index, value);
		g_free (value);
		g_clear_object (&parameter);

		next = i_cal_component_get_next_property (event, IMAGE_PROPERTY);
		g_object_unref (image);
		image = next;
	}
}

/* Return the IMAGE line with the given address, or NULL if there is none.
 *
 * Like attach_pointing_at, but compare the value as a string: an icalattach
 * request on this property crashes (see report_pictures). Ignore the key. */
static ICalProperty *
image_pointing_at (ICalComponent *event,
		   const gchar *url)
{
	ICalProperty *image;

	image = i_cal_component_get_first_property (event, IMAGE_PROPERTY);
	while (image) {
		ICalProperty *next;
		gchar *value = i_cal_property_get_value_as_string (image);
		gboolean matches = g_strcmp0 (value, url) == 0;

		g_free (value);

		if (matches)
			return image;

		next = i_cal_component_get_next_property (event, IMAGE_PROPERTY);
		g_object_unref (image);
		image = next;
	}

	return NULL;
}

/* Return the ATTACH line with the given address, or NULL if there is none.
 *
 * Compare the URL that libical parsed (as report_resource prints it), as a
 * consumer does. Ignore the key: the *mapping* pairs lines with server
 * entries, and a selection by key cannot find a wrong pairing. */
static ICalProperty *
attach_pointing_at (ICalComponent *event,
		    const gchar *url)
{
	ICalProperty *attach;

	attach = i_cal_component_get_first_property (event, I_CAL_ATTACH_PROPERTY);
	while (attach) {
		ICalProperty *next;
		ICalAttach *value = i_cal_property_get_attach (attach);
		gboolean matches = value && g_strcmp0 (i_cal_attach_get_url (value), url) == 0;

		g_clear_object (&value);

		if (matches)
			return attach;

		next = i_cal_component_get_next_property (event, I_CAL_ATTACH_PROPERTY);
		g_object_unref (attach);
		attach = next;
	}

	return NULL;
}

/* Get one event until EDS has it, or return NULL after EDIT_WAIT_TRIES. Retry
 * after each error, also after E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND. The caller
 * reports the failure. */
static ICalComponent *
wait_for_event (ECalClient *cal,
		const gchar *uid)
{
	guint try;

	for (try = 0; try < EDIT_WAIT_TRIES; try++) {
		ICalComponent *component = NULL;
		GError *error = NULL;

		if (e_cal_client_get_object_sync (cal, uid, NULL, &component, NULL, &error)) {
			g_print ("waited-tries=%u\n", try);

			return component;
		}

		g_clear_error (&error);
		g_usleep (EDIT_WAIT_INTERVAL_US);
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
	ICalComponent *fetched;
	ICalComponent *event;
	ICalComponent *read_back = NULL;
	ICalComponent *read_back_event;
	ICalProperty *conference;
	ICalProperty *attach;
	ICalProperty *image;
	ICalAttach *attach_value;
	const gchar *source_uid;
	const gchar *event_uid;
	const gchar *new_location;
	const gchar *new_conference_uri;
	const gchar *old_attach_uri;
	const gchar *new_attach_uri;
	const gchar *old_image_uri;
	const gchar *new_image_uri;

	if (argc != 9) {
		g_printerr ("usage: %s <source-uid> <event-uid> <new-location> "
			    "<new-conference-uri> <old-attach-uri> "
			    "<new-attach-uri> <old-image-uri> "
			    "<new-image-uri>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	event_uid = argv[2];
	new_location = argv[3];
	new_conference_uri = argv[4];
	old_attach_uri = argv[5];
	new_attach_uri = argv[6];
	old_image_uri = argv[7];
	new_image_uri = argv[8];

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("source: no source with uid %s\n", source_uid);
		return 1;
	}

	/* (guint32) -1 tells EDS "do not wait for connected". The built-in wait
	 * blocks the only thread that iterates the context of the notification,
	 * thus it always expires. functional_report_connection_status below waits
	 * correctly. See "Why the clients pass 'do not wait for connected'" in
	 * docs/functional-tests.md. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return functional_fail ("connect", error);

	/* Report the status before the first request, as cal-client.c does.
	 * e_cal_client_connect_sync succeeds also when the backend connect_sync
	 * failed. Without the status, a calendar that the backend did not open
	 * looks like an open calendar that is read-only. */
	functional_report_connection_status (source, 10);

	cal = E_CAL_CLIENT (client);
	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	fetched = wait_for_event (cal, event_uid);
	if (!fetched) {
		g_printerr ("get-seeded: EDS never handed back the event %s\n", event_uid);
		return 1;
	}

	event = functional_first_vevent (fetched);
	if (!event) {
		g_printerr ("get-seeded: what EDS handed back holds no VEVENT\n");
		return 1;
	}

	g_print ("read-summary=%s\n", i_cal_component_get_summary (event));
	/* The VEVENT is sufficient: libical finds a TZID in the component *and its
	 * parents*. functional_first_vevent returns a child of `fetched` with its
	 * parent link, thus a VTIMEZONE in the VCALENDAR is available. */
	functional_report_start ("read", event);
	report_places ("read", event);
	report_resource ("read", event);
	report_pictures ("read", event);

	/* The first edit: the user changes the place in the Evolution appointment
	 * editor. i_cal_component_set_location replaces the LOCATION value and
	 * keeps its parameters (measured). */
	i_cal_component_set_location (event, new_location);

	/* The second edit: the conference address. Evolution 3.52 has no control
	 * for it (see CONFERENCE_PROPERTY), thus another client on the account
	 * makes this edit. This edit makes the X-JMAP-KEY necessary: RFC 7986
	 * §5.11 allows many CONFERENCE lines, thus only the key finds the server
	 * entry. RFC 5545 §3.6.1 allows one LOCATION, thus the save finds the
	 * only entry without the key.
	 *
	 * Set the value on the existing property, as with set_location above. The
	 * test examines if a save keeps the loaded parameters, and a new property
	 * has none. */
	conference = i_cal_component_get_first_property (event, CONFERENCE_PROPERTY);
	if (!conference) {
		g_printerr ("edit-conference: the event EDS handed back has no "
			    "CONFERENCE to retype\n");
		return 1;
	}
	i_cal_property_set_value_from_string (conference, new_conference_uri, "URI");
	g_object_unref (conference);

	/* The third edit: the address of one document. Another client makes this
	 * edit. The Evolution 3.52 appointment editor attaches local files, and the
	 * mapping intentionally ignores file: URIs. RFC 5545 §3.8.1.1 allows many
	 * ATTACH lines, thus this edit also makes the key necessary.
	 *
	 * Use the line with old_attach_uri: the user selects one attachment of
	 * many. A save that changes the first line changes the wrong document.
	 *
	 * Set the value on the existing property (a new property has no
	 * parameters), and with the icalattach API that a consumer uses. */
	attach = attach_pointing_at (event, old_attach_uri);
	if (!attach) {
		g_printerr ("edit-attach: the event EDS handed back has no ATTACH "
			    "pointing at %s to re-address\n", old_attach_uri);
		return 1;
	}
	attach_value = i_cal_attach_new_from_url (new_attach_uri);
	i_cal_property_set_attach (attach, attach_value);
	g_object_unref (attach_value);
	g_object_unref (attach);

	/* The fourth edit: the address of the picture, from the same `links` map
	 * but in its own property (see report_pictures). Change it in the same
	 * save as the attachment. Only with both kinds of line does a test find a
	 * mapping that pairs lines with entries by count, not by key.
	 *
	 * Use i_cal_property_set_value_from_string, not the icalattach API: with
	 * VALUE=URI, the value is a URI, not an icalattach. This keeps the
	 * parameters, as a consumer edit does. */
	image = image_pointing_at (event, old_image_uri);
	if (!image) {
		g_printerr ("edit-image: the event EDS handed back has no IMAGE "
			    "stating %s to re-address\n", old_image_uri);
		return 1;
	}
	i_cal_property_set_value_from_string (image, new_image_uri, "URI");
	g_object_unref (image);

	if (!e_cal_client_modify_object_sync (cal, event, E_CAL_OBJ_MOD_ALL,
					      E_CAL_OPERATION_FLAG_NONE, NULL, &error))
		return functional_fail ("modify", error);

	g_object_unref (event);
	g_object_unref (fetched);

	if (!e_cal_client_get_object_sync (cal, event_uid, NULL, &read_back, NULL, &error))
		return functional_fail ("get-after-modify", error);

	read_back_event = functional_first_vevent (read_back);
	if (!read_back_event) {
		g_printerr ("get-after-modify: what EDS handed back holds no VEVENT\n");
		return 1;
	}

	functional_report_start ("read-back", read_back_event);
	report_places ("read-back", read_back_event);
	report_resource ("read-back", read_back_event);
	report_pictures ("read-back", read_back_event);

	g_object_unref (read_back_event);
	g_object_unref (read_back);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
