/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "test-util.h"

/* Fail the run and name the failed step. Use it only for a precondition of the
 * program. Report the behaviour under test, do not judge it here. */
int
functional_fail (const gchar *step,
		 GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* Write one `key=value` line to stdout for `jmap_functional::observations`. */
void
functional_observe (const gchar *key,
		    const gchar *value)
{
	g_print ("%s=%s\n", key, value ? value : "(null)");
}
