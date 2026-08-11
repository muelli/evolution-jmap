/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Shared by the calendar client programs that read an event the *server* held:
 * when does EDS say it starts, and what instant does a libecal consumer land on?
 *
 * Shared rather than copied because the two programs that ask it — cal-edit-client.c
 * for an event in a zone libical's own database names, cal-zone-client.c for one in
 * a zone only the server can name — exist to be *compared*. Reporting the instant
 * through one function is what makes the difference between their answers a
 * difference in the event rather than in how it was measured.
 */

#ifndef FUNCTIONAL_EVENT_START_H
#define FUNCTIONAL_EVENT_START_H

#include <libecal/libecal.h>

G_BEGIN_DECLS

void	functional_report_start		(const gchar *prefix,
					 ICalComponent *event);

G_END_DECLS

#endif /* FUNCTIONAL_EVENT_START_H */
