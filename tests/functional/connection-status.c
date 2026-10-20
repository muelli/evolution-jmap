/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Reports ESource:connection-status. This is the EDS answer to "is this
 * backend connected?", which Evolution shows and which EDS clients wait on.
 * e_book_meta_backend_ensure_connected_sync() and its calendar equivalent set
 * it: first `connecting`, then `connected` if the connect_sync() vfunc returns
 * TRUE, or `disconnected` if it returns FALSE. Thus `connected` comes from EDS,
 * not from this backend.
 *
 * # Why this file exists
 *
 * A non-zero wait to e_book_client_connect_sync() calls
 * e_client_wait_for_connected_sync(). In a program like this one, that call
 * always uses the full timeout. The symptom (a 30-second stall when a JMAP
 * address book opens) looks like a backend that does not connect:
 *
 *   - The status comes over D-Bus. source_notify_dbus_connection_status_cb()
 *     does not apply it immediately: it queues an idle on the GMainContext of
 *     the source (e-source.c:899). That context is the thread-default context
 *     when ESourceRegistry is constructed (e-source-registry.c:1726, :683).
 *     Here, that is the default context of this program, on this thread.
 *   - e_client_wait_for_connected_sync() blocks the same thread on an EFlag
 *     until notify::connection-status occurs (e-client.c:1935). The idle emits
 *     the signal, but the idle runs only when the blocked thread iterates the
 *     context.
 *
 * Evolution does not have this problem: it has a main loop and waits on a
 * worker thread. A synchronous program with no main loop blocks itself until
 * the timeout expires. A backend cannot fix this. The client must run a main
 * loop.
 *
 * This analysis comes from the evolution-data-server 3.52.3 source code.
 */

#include "connection-status.h"

/* State for one wait. The deadline is necessary: a backend that does not
 * connect stays `disconnected`, and the test must fail, not hang. */
typedef struct {
	ESource *source;
	GMainLoop *loop;
	gint64 deadline;
} WaitForConnected;

static gboolean
functional_connection_status_tick_cb (gpointer user_data)
{
	WaitForConnected *wait = user_data;

	if (e_source_get_connection_status (wait->source) == E_SOURCE_CONNECTION_STATUS_CONNECTED ||
	    g_get_monotonic_time () >= wait->deadline) {
		g_main_loop_quit (wait->loop);

		return G_SOURCE_REMOVE;
	}

	return G_SOURCE_CONTINUE;
}

/**
 * functional_report_connection_status:
 * @source: the #ESource the client opened
 * @timeout_seconds: the maximum time to wait for `connected` before the
 *     function reports the current status
 *
 * Iterates the default main context of this thread until @source is connected
 * or @timeout_seconds pass. Then prints one observation line with the enum nick:
 *
 *   connection-status=connected
 *
 * The other values that the tests look for are `connecting` and `disconnected`.
 */
void
functional_report_connection_status (ESource *source,
                                    guint timeout_seconds)
{
	WaitForConnected wait;
	GEnumClass *enum_class;
	GEnumValue *enum_value;

	g_return_if_fail (E_IS_SOURCE (source));

	wait.source = source;
	wait.loop = g_main_loop_new (NULL, FALSE);
	wait.deadline = g_get_monotonic_time () + timeout_seconds * G_TIME_SPAN_SECOND;

	if (e_source_get_connection_status (source) != E_SOURCE_CONNECTION_STATUS_CONNECTED) {
		GSource *tick;

		/* Poll the condition. The loop delivers the status from an idle
		 * on this context. The timeout source checks the deadline when
		 * no update arrives. */
		tick = g_timeout_source_new (25);
		g_source_set_callback (tick, functional_connection_status_tick_cb, &wait, NULL);
		g_source_attach (tick, NULL);

		g_main_loop_run (wait.loop);

		g_source_destroy (tick);
		g_source_unref (tick);
	}

	g_main_loop_unref (wait.loop);

	enum_class = g_type_class_ref (E_TYPE_SOURCE_CONNECTION_STATUS);
	enum_value = g_enum_get_value (enum_class, e_source_get_connection_status (source));

	g_print ("connection-status=%s\n", enum_value ? enum_value->value_nick : "unknown");

	g_type_class_unref (enum_class);
}
