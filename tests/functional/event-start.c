/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * event-start.h tells why this is a shared file and not a static function in
 * each program that calls it.
 */

#include "event-start.h"

/* Report three facts about the DTSTART that EDS keeps: the value as written,
 * the TZID, and the UTC instant.
 *
 * Each fact shows a different failure of a zoned DTSTART. The first two are
 * read from the property, as dtstart_parts() in cal-client.c does. The TZID
 * stays verbatim, because the test checks which zone it names, not its spelling.
 *
 * Only the UTC instant shows that the TZID resolves. i_cal_component_get_dtstart()
 * resolves a TZID against the VTIMEZONEs of the parent component first, then
 * against the builtin zone table. libical does not adjust a floating time when
 * it converts it. Thus an unresolved zone gives the wall clock with a "Z" added.
 * This value is the same on every machine, so the test can assert it.
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

	/* Do not unref the UTC zone: libical owns it, as with the zones that
	 * i_cal_timezone_get_builtin_timezone() gives cal-client.c. */
	start = i_cal_component_get_dtstart (event);
	utc = start ? i_cal_time_convert_to_zone (
		start, i_cal_timezone_get_utc_timezone ()) : NULL;
	value = utc ? i_cal_time_as_ical_string (utc) : NULL;
	g_print ("%s-dtstart-utc=%s\n", prefix, value ? value : "");
	g_free (value);
	g_clear_object (&utc);
	g_clear_object (&start);
}

/* Return the first VEVENT of what get_object returns: a bare VEVENT for an
 * event with one instance, or a VCALENDAR that contains the instances. Accept
 * both forms, so that the form of the result does not cause an unrelated
 * failure. */
ICalComponent *
functional_first_vevent (ICalComponent *component)
{
	if (i_cal_component_isa (component) == I_CAL_VEVENT_COMPONENT)
		return g_object_ref (component);

	return i_cal_component_get_first_component (component, I_CAL_VEVENT_COMPONENT);
}
