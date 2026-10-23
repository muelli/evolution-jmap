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

/* Count the `credentials-required` signal in the guint at `user_data`. In the
 * application, this signal shows the credentials prompter (for an OAuth2
 * source, the consent window). */
void
functional_credentials_required_cb (ESource *source,
				    ESourceCredentialsReason reason,
				    const gchar *certificate_pem,
				    GTlsCertificateFlags certificate_errors,
				    const GError *op_error,
				    gpointer user_data)
{
	guint *count = user_data;

	(*count)++;
}

/* Iterate the default main context of this thread for `milliseconds`, so that
 * queued idles run. ESource applies D-Bus changes only from idles. The wait
 * always uses the full time, because an observation can be the absence of an
 * event. */
void
functional_settle (guint milliseconds)
{
	gint64 deadline = g_get_monotonic_time () + milliseconds * G_TIME_SPAN_MILLISECOND;

	while (g_get_monotonic_time () < deadline) {
		g_main_context_iteration (NULL, FALSE);
		/* g_usleep and G_TIME_SPAN_MILLISECOND both use microseconds. */
		g_usleep (10 * G_TIME_SPAN_MILLISECOND);
	}
}
