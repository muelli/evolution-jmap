/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * See event-start.h for why this is a file of its own rather than a static
 * function in each of the two programs that call it.
 */

#include "event-start.h"

/* Report when EDS says the event starts: the value as it stands on the line, the
 * TZID naming the clock it is on, and the instant a consumer's own clock lands
 * on.
 *
 * Three observations because a `DTSTART` in a zone can go wrong in three
 * independent ways, and each answers one of them. The first two are dtstart_parts
 * in cal-client.c, read off the property so that what is reported is what EDS
 * *kept* — the wall clock, and the identifier verbatim, since how an identifier is
 * spelled is libical's business and the question here is which zone it names.
 *
 * The third is the only one that says the identifier means anything:
 * i_cal_component_get_dtstart resolves a TZID the way any libical consumer does —
 * against the enclosing component's VTIMEZONEs first and then against the builtin
 * zone table — so converting its answer to UTC is a consumer asking "when is that,
 * really?". libical does not adjust a *floating* time it converts, so a zone
 * nothing could resolve reports the wall clock with a Z stuck on it rather than
 * something this machine's own zone decides: the failure is the same on every
 * machine, which is what makes it worth asserting.
 */
void
functional_report_start (const gchar *prefix,
			 ICalComponent *event)
{
	ICalProperty *property;
	ICalParameter *parameter;
	ICalTime *start;
	ICalTime *utc;
	gchar *value;

	property = i_cal_component_get_first_property (event, I_CAL_DTSTART_PROPERTY);
	value = property ? i_cal_property_get_value_as_string (property) : NULL;
	g_print ("%s-dtstart=%s\n", prefix, value ? value : "");
	g_free (value);

	parameter = property ? i_cal_property_get_first_parameter (
		property, I_CAL_TZID_PARAMETER) : NULL;
	g_print ("%s-dtstart-tzid=%s\n", prefix,
		 parameter ? i_cal_parameter_get_tzid (parameter) : "");
	g_clear_object (&parameter);
	g_clear_object (&property);

	/* The UTC zone is libical's own object rather than one built here, so it is
	 * not unrefed — the same borrowed thing i_cal_timezone_get_builtin_timezone
	 * hands cal-client.c. */
	start = i_cal_component_get_dtstart (event);
	utc = start ? i_cal_time_convert_to_zone (
		start, i_cal_timezone_get_utc_timezone ()) : NULL;
	value = utc ? i_cal_time_as_ical_string (utc) : NULL;
	g_print ("%s-dtstart-utc=%s\n", prefix, value ? value : "");
	g_free (value);
	g_clear_object (&utc);
	g_clear_object (&start);
}
