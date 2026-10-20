/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Shared by the book and calendar client programs. The question is the same
 * for both: does EDS itself report that the backend is connected?
 * connection-status.c tells why this is not a call to
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
