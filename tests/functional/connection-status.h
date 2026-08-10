/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Shared by both functional client programs, because the question is the
 * same for a book and for a calendar: did EDS itself decide the backend is
 * connected? See connection-status.c for why this cannot be a call to
 * e_client_wait_for_connected_sync().
 */

#ifndef FUNCTIONAL_CONNECTION_STATUS_H
#define FUNCTIONAL_CONNECTION_STATUS_H

#include <libedataserver/libedataserver.h>

G_BEGIN_DECLS

void	functional_report_connection_status	(ESource *source,
						 guint timeout_seconds);

G_END_DECLS

#endif /* FUNCTIONAL_CONNECTION_STATUS_H */
