/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Reports when EDS says a server event starts, and the instant that a libecal
 * consumer calculates.
 *
 * Two programs use it: cal-edit-client.c (a zone in the libical database) and
 * cal-zone-client.c (a zone that only the server names). The tests compare
 * their results. One shared function makes sure that a difference comes from
 * the event, not from the measurement.
 */

#ifndef FUNCTIONAL_EVENT_START_H
#define FUNCTIONAL_EVENT_START_H

#include <libecal/libecal.h>

G_BEGIN_DECLS

void	functional_report_start		(const gchar *prefix,
					 ICalComponent *event);

G_END_DECLS

#endif /* FUNCTIONAL_EVENT_START_H */
