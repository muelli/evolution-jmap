/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * D2's write half, proven through a real, running `evolution-calendar-
 * factory` rather than the in-process fixtures `jmap-backend-cal/tests/
 * {backend,ops}.rs` stop at: an ordinary libecal consumer that opens a
 * calendar (which is what makes the factory dlopen libecalbackendjmap.so
 * and keeps a live `ECalMetaBackend` instance watching this source), then
 * edits the calendar's own `ESourceSelectable` "Color" the way the
 * calendar-properties dialog's colour picker does —
 * `e_source_selectable_set_color()` followed by `e_source_write_sync()` —
 * and waits. It knows nothing about JMAP or the mock: the push this test
 * is about is the backend's `source_changed` vfunc reacting to the
 * resulting `ESource` "changed" signal on its own worker thread, entirely
 * on the other side of the registry from this process.
 *
 * There is nothing client-observable to wait on for the push itself — it is
 * a one-way write to the server, not a round trip this client takes part
 * in, and EDS's own doc comment for `ECalMetaBackendClass::source_changed`
 * says it runs "from a dedicated thread", not synchronously with
 * `e_source_write_sync()` returning. So this waits a plain, generous,
 * fixed settle time before exiting, rather than polling: this harness's
 * daemons die with the private D-Bus session when this process exits, so a
 * Rust-side poll after the fact would only ever be polling an
 * already-torn-down backend.
 *
 * Everything around this program — the scratch XDG tree, the `.source`
 * keyfile, the private D-Bus session, the mock server and every assertion —
 * belongs to `rust/crates/jmap-functional/tests/calendar-color.rs`, which
 * runs it and reads its output. So this file has no test framework in it
 * and no notion of what "correct" is: it reports what EDS told it on
 * stdout, one `key=value` line per observation, and exits non-zero the
 * moment a call fails.
 *
 *   usage: functional-cal-color-client <source-uid> <color>
 */

#include <libecal/libecal.h>

#include "connection-status.h"

/* How long to let the backend's asynchronous push reach the mock before
 * this process exits and tears its private bus — and the backend instance
 * with it — down. Generous for the reason every other timeout in this tree
 * is: it only costs time on a passing run, and the push is a single local
 * HTTP round trip to an in-process mock, not a real network call. */
#define PUSH_SETTLE_SECONDS 3

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	ESourceSelectable *selectable;
	const gchar *source_uid;
	const gchar *color;
	const gchar *initial;

	if (argc != 3) {
		g_printerr ("usage: %s <source-uid> <color>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	color = argv[2];

	/* Activates evolution-source-registry on the session bus, which reads
	 * the scratch sources directory the harness wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* Activates evolution-calendar-factory and keeps a live backend
	 * instance running for the rest of this process's life —
	 * `source_changed` only ever fires on a backend that is connected and
	 * watching this `ESource`, the same reason cal-client.c holds this
	 * open throughout. (guint32) -1 is EDS's "do not wait for connected";
	 * `functional_report_connection_status` below waits properly. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	functional_report_connection_status (source, 10);

	if (!e_source_has_extension (source, E_SOURCE_EXTENSION_CALENDAR)) {
		g_printerr ("source carries no Calendar extension\n");
		return 1;
	}
	selectable = e_source_get_extension (source, E_SOURCE_EXTENSION_CALENDAR);

	/* `ESourceSelectable:color` is not NULL-by-default — EDS's own
	 * GParamSpec defaults it to "#62a0ea" — so this is reported for the
	 * Rust side's own diagnostics, not treated as "no colour". */
	initial = e_source_selectable_get_color (selectable);
	g_print ("initial-color=%s\n", initial ? initial : "");

	/* The edit a colour-picker in the calendar-properties dialog makes:
	 * change the property, then write the source. This is the other end of
	 * `jmap-backend-collection/src/child_source.rs::apply`'s own colour
	 * write (D2's read path) — a plain `ESourceRegistry` consumer, not the
	 * collection backend's populate. */
	e_source_selectable_set_color (selectable, color);
	if (!e_source_write_sync (source, NULL, &error))
		return fail ("write", error);
	g_print ("written=1\n");

	/* See PUSH_SETTLE_SECONDS. */
	g_usleep (PUSH_SETTLE_SECONDS * G_USEC_PER_SEC);

	g_print ("done=1\n");

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
