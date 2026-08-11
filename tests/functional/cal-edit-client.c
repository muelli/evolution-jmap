/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The second client half of the calendar functional test: an ordinary libecal
 * consumer that opens a calendar, reads an event the *server* already held,
 * retypes the places it happens at, the document it points at and the picture
 * of it, and writes them back.
 *
 * A program of its own rather than a phase of cal-client.c, which creates
 * every event it looks at. The two ask opposite questions. cal-client.c asks
 * what reaches the server when the user makes an appointment, so its whole
 * story is a client writing components and EDS's recurrence machinery
 * answering; this one asks what survives the *round trip* — a JSCalendar event
 * the mapping could only draw part of, out to iCalendar, through
 * ECalMetaBackend's cache, into a libecal consumer, back through a save. Only
 * an event nobody here created can ask that, and none of cal-client.c's
 * six hundred lines of series-splitting would be reached on the way.
 *
 * Like its twin it has no notion of what "correct" is: it reports what EDS
 * told it on stdout, one `key=value` line per observation, and exits non-zero
 * the moment a call fails. Every assertion belongs to
 * `rust/crates/jmap-functional/tests/calendar.rs`, which seeds the event,
 * runs this program and reads its output.
 *
 *   usage: functional-cal-edit-client <source-uid> <event-uid> <new-location>
 *                                    <new-conference-uri> <old-attach-uri>
 *                                    <new-attach-uri> <old-image-uri>
 *                                    <new-image-uri>
 *
 * The attachment and the picture to re-address are named by the address they
 * already carry rather than by their position among the lines or by the key the
 * mapping put on them: an event holds any number of both (RFC 5545 §3.8.1.1,
 * RFC 7986 §5.10), that is how a user picks the one they meant, and it keeps
 * this program with no notion of what the mapping writes on a line.
 */

#define LIBICAL_GLIB_UNSTABLE_API 1

#include <libecal/libecal.h>

#include "connection-status.h"

/* The parameter the mapping writes on a LOCATION and on a CONFERENCE to say
 * which entry of the server's map the line was drawn from — jmap-ical's
 * X_JMAP_KEY. Reading it back is the whole point of this program: the save
 * path patches `locations/<key>/name`, so a key EDS did not carry is a save
 * that cannot reach the entry the user edited. */
#define JMAP_KEY_PARAMETER "X-JMAP-KEY"

/* How long to wait for the seeded event to become gettable, and how often to
 * ask. The same reasoning as book-client.c's EDIT_WAIT_TRIES: ECalMetaBackend
 * answers a get for an event its cache has never heard of by asking the
 * backend to load it, so the first try is expected to succeed — but the
 * backend also schedules a refresh during the connect, and a get that arrives
 * while that is still running can be answered out of a cache the refresh has
 * not filled yet. Polling turns that ordering into a wait rather than a flake.
 * Ten seconds in total, far beyond what a local mock needs and well inside the
 * CTest timeout, so a genuinely absent event fails the test rather than
 * hanging it. */
#define EDIT_WAIT_TRIES 200
#define EDIT_WAIT_INTERVAL_US 50000

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* What EDS hands back from get_object: a bare VEVENT for an event with one
 * instance, or a VCALENDAR wrapping the instances when there are several. The
 * event this program reads has one, but taking the wrapper's first VEVENT
 * rather than insisting on the bare form keeps the shape of EDS's answer from
 * becoming a failure that says nothing about what is being tested. */
static ICalComponent *
first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}

/* The value of one non-standard parameter of a property, or the empty string
 * for a property that carries none.
 *
 * libical holds every parameter it has no enum for as an I_CAL_X_PARAMETER
 * whose name is readable through i_cal_parameter_get_xname, so this walks
 * them rather than asking for one by kind. The name is compared
 * case-insensitively because RFC 5545 §3.1 makes parameter names
 * case-insensitive, and this program is reading back what another
 * implementation — the meta backend's cache — chose to write.
 *
 * The empty string rather than NULL for absent, so that a parameter EDS
 * dropped is an observation the harness can assert on instead of a line
 * missing from the output. */
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

/* How many properties of one kind a component carries. Counted rather than
 * inferred from the values, because a line the round trip *added* — a second
 * place under a key nobody chose — is a different bug from one whose value
 * changed, and only the count tells them apart. */
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

/* The CONFERENCE property kind, which libical-glib 3.0's generated
 * ICalPropertyKind does not name.
 *
 * RFC 7986 §5.11 is a decade newer than the enum's oldest members, and the
 * glib binding shipped with EDS 3.52 has no I_CAL_CONFERENCE_PROPERTY even
 * though the libical C enum it is generated from has ICAL_CONFERENCE_PROPERTY
 * and the parser produces it. The cast is therefore between two spellings of
 * the same value, not a reinterpretation: I_CAL_* is defined as the ICAL_*
 * member throughout i-cal-derived-property.h.
 *
 * Worth stating plainly because it is a fact about the platform this test
 * measures: a client that wanted to *offer* a conference through libical-glib
 * has no named constant for the property, which is part of why Evolution 3.52
 * has no UI for one. */
#define CONFERENCE_PROPERTY ((ICalPropertyKind) ICAL_CONFERENCE_PROPERTY)

/* The IMAGE property kind, missing from the generated ICalPropertyKind for the
 * same reason CONFERENCE is: RFC 7986 §5.10 is a decade newer than the enum's
 * oldest members. The same cast between two spellings of one value applies. */
#define IMAGE_PROPERTY ((ICalPropertyKind) ICAL_IMAGE_PROPERTY)

/* Report what EDS holds for the two places the mapping draws: the one line of
 * text a LOCATION is, and the CONFERENCE beside it.
 *
 * Four observations per place rather than one, because they answer different
 * questions and the value alone answers only the least interesting of them.
 * The value says the drawing arrived; the key says the line can be *saved*
 * back onto the entry the server chose, which is the claim the whole
 * patch-into-the-property design rests on; the count says the round trip
 * neither dropped the line nor added one beside it; and the conference's LABEL
 * says a parameter the mapping invented no name for — a standard one, unlike
 * the key — came along too. */
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

/* Report what EDS holds for the documents the event points at — every ATTACH
 * line, one group of observations per line, numbered from one in the order the
 * component carries them.
 *
 * A separate question from the two places above, and the reason is the value
 * type. A LOCATION and a CONFERENCE are text and a URI, which libical keeps as
 * the string the line carried; an ATTACH (RFC 5545 §3.8.1.1) has a value type of
 * its own, so the parser builds an icalattach and the parameters end up standing
 * beside a value the library re-made. Whether the X-JMAP-KEY survives that, and
 * survives ECalMetaBackend's cache afterwards, is not settled by the other two
 * properties surviving it — which is what this leg is for.
 *
 * Every line rather than the first, because an event may point at several
 * documents and that is where the key stops being decoration: with one resource
 * a save that lost the key finds the server's only entry anyway, and with two it
 * re-addresses whichever the mapping guessed. The number in each observation's
 * name is this program's own counting and means nothing beyond keeping the
 * groups apart — which line belongs to which of the server's entries is what the
 * key beside it says, and the harness looks them up by that.
 *
 * The address is read through i_cal_property_get_attach rather than
 * i_cal_property_get_value_as_string: it is the URL libical parsed out, not the
 * text of the line, so a value the round trip re-spelled shows up as a
 * difference here instead of being hidden by the string form.
 *
 * FMTTYPE and SIZE go out beside the key because they are parameters libical
 * *does* have an enum for. A cache that kept only what it recognised would
 * answer those two and drop the key, so reporting all three tells "the cache
 * dropped everything it did not know" apart from "the cache dropped the line".
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

/* The value of a parameter as it would be written on a line — what follows the
 * `=` in libical's own rendering of it, or the empty string for a parameter the
 * component does not carry.
 *
 * Needed because libical-glib names no function that turns a parameter's enum
 * value back into its iCalendar spelling: i_cal_parameter_get_display hands back
 * an ICalParameterDisplay, and printing the number would tie this program's
 * output to the enum's layout rather than to what stands on the line. Asking the
 * library to render the parameter and taking the value off it reports the
 * spelling a reader of the calendar would see, which is the thing under test.
 *
 * The empty string for absent, for the reason x_parameter gives: a parameter EDS
 * dropped should be an observation the harness asserts on, not a line missing
 * from the output. */
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

/* Report what EDS holds for the pictures *of* the event — every IMAGE line, one
 * group of observations per line, numbered like report_resource's.
 *
 * A third question rather than a variant of the second, because RFC 8984 §4.2.7
 * keeps in one `links` map what iCalendar splits across two properties: a
 * document attached to the event is an ATTACH, a picture of it is RFC 7986
 * §5.10's IMAGE, and jmap-ical tells them apart by the `icon` rel. Reporting them
 * apart is what says a link the mapping read as an icon left on the property it
 * belongs on — an event whose picture came back as an ATTACH would pass every
 * observation report_resource makes.
 *
 * The address is read as the value's string form, and here that is not the same
 * choice report_resource made — it is the only one available. §5.10's grammar
 * makes VALUE=URI REQUIRED on the URI alternative, so the mapping writes it, and
 * with the parameter present libical parses the value as an ICAL_URI_VALUE rather
 * than the ICAL_ATTACH_VALUE an ATTACH gets. i_cal_property_get_attach on such a
 * property does not return NULL: it reaches into a union as though the URI were
 * an icalattach and *crashes*. Measured on this VM's libical 3.0.17, and the
 * reason the two reporters differ.
 *
 * DISPLAY goes out beside the key because it is the one member of a JSCalendar
 * Link with a standard parameter of its own, and because §6.1 requires a reader
 * meeting a DISPLAY it does not know to show no image at all — so a value the
 * round trip mangles is worse for the user than one it drops. FMTTYPE joins it;
 * SIZE does not, since §5.10 admits none on this property. */
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

/* The IMAGE line stating one address, or NULL for a component holding none.
 *
 * The twin of attach_pointing_at, and separate from it for the same reason
 * report_pictures is separate from report_resource: the value must be compared as
 * a string here, since asking this property for an icalattach crashes. Nothing
 * here looks at the key either. */
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

/* The ATTACH line pointing at one address, or NULL for a component holding
 * none.
 *
 * The address is compared as libical hands it over — the URL parsed out of the
 * line, the same one report_resource prints — so this picks the line a consumer
 * would recognise rather than the one a text match would. Nothing here looks at
 * the key: which of the server's entries a line stands for is the *mapping's*
 * business, and a program that picked its line by that could not catch the
 * mapping pairing them up wrongly. */
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

/* Ask for one event until EDS has it, or give up. A miss arrives as
 * E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND rather than as a failure of the call, so
 * it is cleared and retried; any other error would be retried too, and the
 * caller says what it was waiting for when the tries run out. */
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

	/* Reported before anything is asked of the calendar, for the reason
	 * cal-client.c gives: e_cal_client_connect_sync succeeds even when the
	 * backend's connect_sync failed, so a calendar the backend never opened
	 * looks from here exactly like one it opened and forgot to claim
	 * writable. */
	functional_report_connection_status (source, 10);

	cal = E_CAL_CLIENT (client);
	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	fetched = wait_for_event (cal, event_uid);
	if (!fetched) {
		g_printerr ("get-seeded: EDS never handed back the event %s\n", event_uid);
		return 1;
	}

	event = first_vevent (fetched);
	if (!event) {
		g_printerr ("get-seeded: what EDS handed back holds no VEVENT\n");
		return 1;
	}

	g_print ("read-summary=%s\n", i_cal_component_get_summary (event));
	report_places ("read", event);
	report_resource ("read", event);
	report_pictures ("read", event);

	/* The first edit: the user retypes the place, which is what Evolution's
	 * appointment editor writes into. i_cal_component_set_location replaces
	 * the value of the existing LOCATION and leaves its parameters where they
	 * are — measured, not assumed. */
	i_cal_component_set_location (event, new_location);

	/* And the second: the address the event is joined at. Evolution 3.52 has
	 * no control for one — libical-glib does not even name the property, see
	 * CONFERENCE_PROPERTY above — so this half of the edit is what another
	 * client on the same account does, not what a user of this plugin can do
	 * today. It is here because it is the only edit that makes the
	 * X-JMAP-KEY load-bearing: RFC 7986 §5.11 admits several CONFERENCE lines,
	 * so the mapping finds the server's entry by the key on the line and by
	 * nothing else. A LOCATION cannot ask that question — RFC 5545 §3.6.1
	 * allows one, so the save finds the single entry in the server's own map
	 * whatever the line says.
	 *
	 * The value is set through the property rather than by replacing it, for
	 * the same reason set_location is used above: what is under test is
	 * whether a save carries the parameters the load handed over, and a fresh
	 * property would carry none by construction. */
	conference = i_cal_component_get_first_property (event, CONFERENCE_PROPERTY);
	if (!conference) {
		g_printerr ("edit-conference: the event EDS handed back has no "
			    "CONFERENCE to retype\n");
		return 1;
	}
	i_cal_property_set_value_from_string (conference, new_conference_uri, "URI");
	g_object_unref (conference);

	/* And the third: the address of one of the documents the event points at.
	 * Like the conference this is an edit another client makes rather than one
	 * Evolution 3.52 offers — its appointment editor attaches files from the
	 * user's own disk, which is a file: URI the mapping deliberately never
	 * reads — and like the conference it is the edit that makes the key
	 * load-bearing, since RFC 5545 §3.8.1.1 admits any number of ATTACH lines.
	 *
	 * The line is the one already pointing at old_attach_uri, which is the
	 * user picking the attachment they meant out of several; with two on the
	 * event, a save that took "the attachment" to mean the first line moves a
	 * document nobody asked to move.
	 *
	 * Set through the existing property, and through the icalattach API rather
	 * than as a string, for the same two reasons the conference is: a fresh
	 * property would carry no parameters by construction, and the value is the
	 * one libical hands a consumer, so this is the edit a consumer can actually
	 * make. */
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

	/* And the fourth: the address of the picture of the event. The other half
	 * of the same `links` map, and a property of its own — see
	 * report_pictures. Edited in the same save as the attachment above rather
	 * than in a run of its own, because a mapping that paired lines with
	 * entries by counting rather than by the key on them can only be caught
	 * where both kinds of line are present.
	 *
	 * Set through i_cal_property_set_value_from_string, not through the
	 * icalattach API the ATTACH above uses: with the VALUE=URI the mapping
	 * writes, this property's value is a URI, and handing it an icalattach
	 * would be handing it the wrong kind. Setting the value keeps the
	 * parameters standing beside it, which is what makes this the edit a
	 * consumer can make. */
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
		return fail ("modify", error);

	g_object_unref (event);
	g_object_unref (fetched);

	if (!e_cal_client_get_object_sync (cal, event_uid, NULL, &read_back, NULL, &error))
		return fail ("get-after-modify", error);

	read_back_event = first_vevent (read_back);
	if (!read_back_event) {
		g_printerr ("get-after-modify: what EDS handed back holds no VEVENT\n");
		return 1;
	}

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
