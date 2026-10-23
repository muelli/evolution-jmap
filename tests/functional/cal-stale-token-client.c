/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the CALENDAR part of the stale-access-token acceptance
 * test. It reproduces the hourly re-consent with a real
 * `evolution-calendar-factory` and a real `evolution-source-registry`.
 *
 * A usual libecal consumer. It opens a calendar: the factory then loads
 * `libecalbackendjmap.so` and keeps a connected `ECalMetaBackend` with one
 * pooled JMAP connection. The program creates an event, then waits at a
 * two-file handshake. Meanwhile the harness makes the server refuse the access
 * token of that connection. Then the program creates a second event. The test
 * examines the result of the second create, but this program does not judge
 * it. `rust/crates/jmap-functional/tests/cal-stale-token.rs` runs this program
 * and examines the `key=value` lines on stdout.
 *
 * Why the method of the mail test does not apply here (EDS 3.52.3).
 * `tests/functional/mail-stale-token-client.c` subclasses `CamelSession` and
 * answers `get_oauth2_access_token_sync` from a file that the harness writes.
 * This works because `jmap_mail::oauth2::uses_oauth2` reads the Camel
 * `auth-mechanism` field, and `[Authentication] Method=OAuth2` is sufficient
 * for it. The calendar backend has no such field.
 * `jmap_backend_core::oauth2::source_uses_oauth2` uses
 * `e_oauth2_services_is_oauth2_alias`, which matches only a `Method` that
 * names an `EOAuth2Service` registered in the process. Thus the account must
 * have `Method=JMAP`. With this method, EDS `module-oauth2-services.so`
 * exports `Source.OAuth2Support` for the source. Then the factory call to
 * `e_source_get_oauth2_access_token_sync` goes over D-Bus to the registry
 * (`e-source.c::source_get_oauth2_access_token_sync` uses the in-process
 * `e_oauth2_services_find` fallback only when the interface is ABSENT). Thus
 * the refresh runs in the registry, through the JMAP
 * `EOAuth2Service::get_refresh_uri`, which reads `TokenEndpoint` from
 * `[JMAP OAuth2]`. The `/oauth/token` endpoint of the mock JMAP server
 * controls which access token is current.
 *
 * How the test causes a refresh without a clock. Before it connects, this
 * program stores the token JSON format of `e_oauth2_service.c` in the secret
 * store, but with ONLY a `refresh_token`. `eos_lookup_token_sync` (line 1316)
 * calculates `expires_in` from the stored `expires_after`. With no
 * `expires_after`, the value is -1. `e_oauth2_service_get_access_token_sync`
 * (line 1893) refreshes when the value is `<= TOKEN_VALIDITY_GAP_SECS`
 * (10, line 47). Thus each fetch goes to the token endpoint, with no timing
 * window. The `/oauth/token` handler of the harness controls which token is
 * current, as the token file of `mail-stale-token-client.c` does for mail.
 * This program stores the secret, not the Rust side, because
 * `e_secret_store_store_sync` uses the `org.freedesktop.secrets` provider on
 * the session bus of this process.
 *
 *   usage: functional-cal-stale-token-client <source-uid> <secret-uid> \
 *              <secret-json> <ready-path> <go-path>
 *
 * The harness supplies <secret-uid> and <secret-json>, as it supplies all
 * other inputs in this tree. The Rust side asserts about them, and values
 * that this client made could become different from the values in the test.
 */

#include <libecal/libecal.h>

#include "connection-status.h"

/* The interface that `e_server_side_source_set_oauth2_support` exports. A
 * string, as in `oauth2-stale-proxy-client.c`: the generated
 * `EDBusSourceOAuth2Support` accessors are in the private header
 * `src/private/e-dbus-source.h`, which EDS does not install. From
 * `src/private/org.gnome.evolution.dataserver.Source.xml:210`. */
#define OAUTH2_SUPPORT_INTERFACE "org.gnome.evolution.dataserver.Source.OAuth2Support"

/* The maximum wait for the harness to change the token that the server
 * accepts. If the harness stops, the limit makes the run fail and not hang. */
#define GO_TIMEOUT_SECONDS 45

/* How long to iterate the default main context after the second create, to
 * receive a `credentials-required` from the registry. ESource applies D-Bus
 * changes from an idle on the thread-default context of the `ESourceRegistry`
 * (this thread). Only iteration delivers the signal. See the header of
 * `connection-status.c`. */
#define SETTLE_MILLISECONDS 1500

/* Write one `key=value` line to stdout for `jmap_functional::observations`. */
static void
observe (const gchar *key,
         const gchar *value)
{
	g_print ("%s=%s\n", key, value ? value : "(null)");
}

static void
observe_boolean (const gchar *key,
                 gboolean value)
{
	observe (key, value ? "1" : "0");
}

/* Fail the run and name the failed step. Use only for preconditions of this
 * program. Report the behaviour under test, do not judge it here. */
static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* Count the signal. In the application, it shows the credentials prompter
 * (for an OAuth2 source, the consent window). A stale access token must not
 * cause it. */
static void
credentials_required_cb (ESource *source,
                         ESourceCredentialsReason reason,
                         const gchar *certificate_pem,
                         GTlsCertificateFlags certificate_errors,
                         const GError *op_error,
                         gpointer user_data)
{
	guint *count = user_data;

	(*count)++;
}

/* Return TRUE if the registry exported `Source.OAuth2Support` for this source.
 * This is a necessary precondition, which the Rust side asserts. Without the
 * interface, the token fetch uses the in-process fallback in `e-source.c`. In
 * a calendar factory, that fallback has no `[JMAP OAuth2]` extension type: the
 * `jmap-backend-cal` module registers the OAuth2 *service*, but does not call
 * `jmap_config::oauth2::ensure_registered`. Thus it cannot make a refresh
 * URI. */
static gboolean
oauth2_support_exported (ESource *source)
{
	GDBusObject *dbus_object;
	GDBusInterface *interface;
	gboolean exported;

	dbus_object = e_source_ref_dbus_object (source);
	if (!dbus_object)
		return FALSE;

	interface = g_dbus_object_get_interface (dbus_object, OAUTH2_SUPPORT_INTERFACE);
	exported = interface != NULL;

	g_clear_object (&interface);
	g_object_unref (dbus_object);

	return exported;
}

/* A minimal VEVENT with the given UID and summary. Text, not setters: the
 * event only makes the backend send a request, and a minimal one is safest. */
static ICalComponent *
build_event (const gchar *uid,
             const gchar *summary)
{
	ICalComponent *event;
	gchar *icalendar;

	icalendar = g_strdup_printf (
		"BEGIN:VEVENT\r\n"
		"UID:%s\r\n"
		"DTSTAMP:20260901T080000Z\r\n"
		"DTSTART:20260901T090000Z\r\n"
		"DTEND:20260901T100000Z\r\n"
		"SUMMARY:%s\r\n"
		"END:VEVENT\r\n",
		uid, summary);
	event = i_cal_component_new_from_string (icalendar);
	g_free (icalendar);

	return event;
}

/* Create one event and report the result and the error message. Not fatal:
 * the expected result of the second call depends on the token that the
 * harness lets the server accept. */
static gboolean
create_event (ECalClient *cal,
              const gchar *uid,
              const gchar *summary,
              const gchar *ok_key,
              const gchar *error_key)
{
	ICalComponent *event;
	GError *error = NULL;
	gchar *created_uid = NULL;
	gboolean ok;

	event = build_event (uid, summary);
	if (!event) {
		g_printerr ("build: libical would not parse the event this test writes\n");
		exit (1);
	}

	ok = e_cal_client_create_object_sync (cal, event, E_CAL_OPERATION_FLAG_NONE,
					      &created_uid, NULL, &error);
	g_object_unref (event);

	observe_boolean (ok_key, ok);
	if (!ok)
		observe (error_key, error && error->message ? error->message : "(no error set)");

	g_free (created_uid);
	g_clear_error (&error);

	return ok;
}

/* Iterate the default main context for `milliseconds`, so that queued idles
 * run (ESource applies D-Bus changes only from idles). This always uses the
 * full time, because the observation is the *absence* of an event. */
static void
settle (guint milliseconds)
{
	gint64 deadline = g_get_monotonic_time () + milliseconds * G_TIME_SPAN_MILLISECOND;

	while (g_get_monotonic_time () < deadline) {
		g_main_context_iteration (NULL, FALSE);
		/* g_usleep and G_TIME_SPAN_MILLISECOND both use microseconds. */
		g_usleep (10 * G_TIME_SPAN_MILLISECOND);
	}
}

/* Wait until `path` exists, and iterate the main context so that signals from
 * the registry and factory arrive. Returns FALSE at the timeout. */
static gboolean
wait_for_file (const gchar *path,
               guint timeout_seconds)
{
	gint64 deadline = g_get_monotonic_time () + timeout_seconds * G_TIME_SPAN_SECOND;

	while (!g_file_test (path, G_FILE_TEST_EXISTS)) {
		if (g_get_monotonic_time () >= deadline)
			return FALSE;

		g_main_context_iteration (NULL, FALSE);
		g_usleep (20 * G_TIME_SPAN_MILLISECOND);
	}

	return TRUE;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	ECalClient *cal;
	const gchar *source_uid;
	const gchar *secret_uid;
	const gchar *secret_json;
	const gchar *ready_path;
	const gchar *go_path;
	guint credentials_required = 0;

	if (argc != 6) {
		g_printerr ("usage: %s <source-uid> <secret-uid> <secret-json> "
			    "<ready-path> <go-path>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	secret_uid = argv[2];
	secret_json = argv[3];
	ready_path = argv[4];
	go_path = argv[5];

	/* Before a connect, store the refresh token from an earlier consent. EDS
	 * needs it to get an access token silently. */
	if (!e_secret_store_store_sync (secret_uid, secret_json,
					"Evolution Data Source - jmap-functional",
					TRUE, NULL, &error))
		return fail ("seed-secret", error);

	/* This starts evolution-source-registry, which reads the scratch sources
	 * directory. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	observe_boolean ("oauth2-support-exported", oauth2_support_exported (source));

	/* Connect the signal before the first connect, to count a prompt from the
	 * connect too. The connect fetches its own token, and a failure there is
	 * a different bug. */
	g_signal_connect (source, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);

	/* This starts evolution-calendar-factory, which keeps a backend for the
	 * life of this process. Here the backend makes the pooled connection and
	 * fetches its access token once. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	functional_report_connection_status (source, 10);

	/* The create before the token becomes stale. It is a precondition: a
	 * backend that cannot write also makes the second create fail. */
	if (!create_event (cal, "jmap-functional-stale-token-1", "Before the rotation",
			   "first-create-ok", "first-create-error"))
		return 1;

	g_print ("credentials-required-before-rotation=%u\n", credentials_required);

	/* Synchronize with the harness. After the connection exists, it changes
	 * the bearer token that the server accepts (and, in the positive case,
	 * the next token from the token endpoint). The bug needs a token that
	 * becomes stale BETWEEN two calls of this client. */
	if (!g_file_set_contents (ready_path, "ready", -1, &error))
		return fail ("write-ready", error);

	if (!wait_for_file (go_path, GO_TIMEOUT_SECONDS)) {
		g_printerr ("the harness never wrote %s\n", go_path);
		return 1;
	}

	/* The create under test. */
	create_event (cal, "jmap-functional-stale-token-2", "After the rotation",
		      "second-create-ok", "second-create-error");

	settle (SETTLE_MILLISECONDS);

	g_print ("credentials-required=%u\n", credentials_required);
	g_print ("done=1\n");

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
