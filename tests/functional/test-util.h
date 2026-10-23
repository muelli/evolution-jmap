/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Small helpers that many client programs use in the same form.
 */

#ifndef FUNCTIONAL_TEST_UTIL_H
#define FUNCTIONAL_TEST_UTIL_H

#include <glib.h>

G_BEGIN_DECLS

int	functional_fail			(const gchar *step,
					 GError *error);

G_END_DECLS

#endif /* FUNCTIONAL_TEST_UTIL_H */
