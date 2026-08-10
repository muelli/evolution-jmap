/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Reports ESource:connection-status, which is EDS's own answer to "is this
 * backend connected?" — the thing Evolution shows as a connected account, and
 * the thing every EDS client that waits for a backend waits on. The meta
 * backends set it themselves: e_book_meta_backend_ensure_connected_sync()
 * and its calendar twin move the source to `connecting`, then to `connected`
 * if the backend's connect_sync() vfunc returned TRUE and to `disconnected`
 * if it did not. So a source that reaches `connected` is EDS agreeing that
 * our connect worked, said by EDS rather than by us.
 *
 * # Why this file exists at all
 *
 * The obvious way to ask is to pass a non-zero wait to
 * e_book_client_connect_sync(), which calls
 * e_client_wait_for_connected_sync() for you. In a program like this one it
 * can only ever burn the whole timeout, and that is worth writing down
 * because the symptom — a 30-second stall on opening a JMAP address book —
 * reads exactly like a backend that never connects:
 *
 *   - The status arrives over D-Bus. ESource learns about it in
 *     source_notify_dbus_connection_status_cb(), which does not apply it
 *     there: it queues an idle on the source's GMainContext
 *     (e-source.c:899). That context is whatever was thread-default when
 *     ESourceRegistry was constructed (e-source-registry.c:1726, :683) —
 *     here, this program's default context, on this thread.
 *   - e_client_wait_for_connected_sync() blocks that same thread on an EFlag
 *     until notify::connection-status fires (e-client.c:1732). The signal
 *     comes from the idle. The idle needs the context iterated. The thread
 *     that would iterate it is the one blocked on the EFlag.
 *
 * Evolution never notices because it has a main loop and does the wait on a
 * worker thread; a synchronous program with no main loop deadlocks against
 * itself until the timeout expires. The wait is therefore not something a
 * backend can fix and not something to sit through — it is a client-side
 * contract, and the fix is on this side of it: run a main loop.
 *
 * All of the above was read out of evolution-data-server 3.52.3, not
 * inferred from the symptom.
 */

#include "connection-status.h"

/* State for one wait. The deadline matters as much as the condition: a
 * backend that fails to connect leaves the source in `disconnected` forever,
 * and a wait without a limit would hang the test run instead of failing it
 * with the status it saw. */
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
 * @timeout_seconds: how long to wait for `connected` before reporting
 *     whatever the status is instead
 *
 * Iterates this thread's default main context until @source is connected or
 * @timeout_seconds have passed, then prints one observation line:
 *
 *   connection-status=connected
 *
 * with the enum's nick — `connecting` and `disconnected` are the two the
 * tests are interested in seeing instead.
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

		/* The condition is polled rather than waited on. The status
		 * update and notify::connection-status both come from an idle
		 * on this context, so running the loop is what delivers them
		 * either way; a timeout source on top of that guarantees the
		 * deadline is still checked when nothing at all arrives. */
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
