/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of `docs/ROADMAP.md` item 25's CALENDAR leg: item 23's
 * hourly re-consent, driven through a real `evolution-calendar-factory` and a
 * real `evolution-source-registry` rather than through the operator leaving
 * Evolution open for an afternoon.
 *
 * An ordinary libecal consumer. It opens a calendar — which is what makes the
 * factory dlopen `libecalbackendjmap.so` and keep a live, connected
 * `ECalMetaBackend` instance holding one pooled JMAP connection — creates an
 * event, waits at a two-file handshake while the harness makes the server
 * refuse the access token that connection was built with, and creates a
 * second event. Whether that second create succeeds, and at what cost, is the
 * whole question; this program answers nothing about it. Every judgement
 * belongs to `rust/crates/jmap-functional/tests/cal-stale-token.rs`, which
 * runs this program and reads the `key=value` observations it writes to
 * stdout.
 *
 * WHY THE MAIL LEG'S SHORTCUT IS NOT AVAILABLE HERE (EDS 3.52.3, read
 * 2026-08-29). `tests/functional/mail-stale-token-client.c` subclasses
 * `CamelSession` and answers `get_oauth2_access_token_sync` out of a file the
 * harness rewrites, because `jmap_mail::oauth2::uses_oauth2` reads Camel's own
 * `auth-mechanism` field and the generic `[Authentication] Method=OAuth2`
 * spelling is enough for it. The calendar backend has no such field to read:
 * `jmap_backend_core::oauth2::source_uses_oauth2` goes through
 * `e_oauth2_services_is_oauth2_alias`, which matches only a `Method` naming an
 * `EOAuth2Service` actually registered in the asking process. So the account
 * must say `Method=JMAP`, and per item 22's finding that is exactly the
 * condition under which EDS's own `module-oauth2-services.so` exports
 * `Source.OAuth2Support` for the source — which routes the factory's
 * `e_source_get_oauth2_access_token_sync` over D-Bus to the registry
 * (`e-source.c::source_get_oauth2_access_token_sync` takes its in-process
 * `e_oauth2_services_find` fallback only when the interface is ABSENT). The
 * refresh therefore runs in the registry, against a real
 * `EOAuth2Service::get_refresh_uri` — ours, reading `[JMAP OAuth2]`'s
 * `TokenEndpoint` — and the mock JMAP server's own `/oauth/token` endpoint is
 * what decides which access token exists at any moment.
 *
 * HOW THE REFRESH IS PROVOKED, and why it needs no wall clock. This program
 * seeds the secret store, before it connects anything, with the JSON
 * `e_oauth2_service.c` stores tokens as — but carrying ONLY a `refresh_token`.
 * `eos_lookup_token_sync` (line 1316) derives `expires_in` from the stored
 * `expires_after`, so an absent one leaves it at -1, and
 * `e_oauth2_service_get_access_token_sync` (line 1893) refreshes whenever that
 * number is `<= TOKEN_VALIDITY_GAP_SECS` (10, line 47). Every fetch therefore
 * goes to the token endpoint, deterministically, with no timing window for the
 * test to lose — the harness's `/oauth/token` handler is the single authority
 * on which token is current, exactly as `mail-stale-token-client.c`'s token
 * file is for the mail leg. The secret is seeded from here rather than by the
 * Rust side because `e_secret_store_store_sync` talks to the `org.freedesktop.
 * secrets` provider on the session bus, and the session bus is the one this
 * process was started on.
 *
 *   usage: functional-cal-stale-token-client <source-uid> <secret-uid> \
 *              <secret-json> <ready-path> <go-path>
 *
 * <secret-uid> and <secret-json> are stated by the harness rather than built
 * here for the reason every other input in this tree is: they are what the
 * Rust side asserts about, and a client that derived them could drift from
 * the test that reads its output.
 */

#include <libecal/libecal.h>

#include "connection-status.h"

/* The interface `e_server_side_source_set_oauth2_support` exports. Named as a
 * string for the reason `oauth2-stale-proxy-client.c` gives: EDS's generated
 * `EDBusSourceOAuth2Support` accessors live in `src/private/e-dbus-source.h`,
 * which is not installed, while `e_source_ref_dbus_object` is public and hands
 * back a plain `GDBusObject`. From `src/private/
 * org.gnome.evolution.dataserver.Source.xml:210`. */
#define OAUTH2_SUPPORT_INTERFACE "org.gnome.evolution.dataserver.Source.OAuth2Support"

/* How long to wait for the harness to finish rotating the server's mind about
 * which token it accepts. Generous for the same reason the ctest timeout is;
 * a limit at all is so a harness that died fails this run instead of hanging
 * it. */
#define GO_TIMEOUT_SECONDS 45

/* How long to keep iterating this thread's default main context after the
 * second create, so that a `credentials-required` the registry emitted has a
 * chance to arrive before the count is reported. ESource applies D-Bus news
 * from an idle on the context that was thread-default when `ESourceRegistry`
 * was constructed — this thread's — so the signal is delivered by iterating,
 * and only by iterating. See `connection-status.c`'s header. */
#define SETTLE_MILLISECONDS 1500

/* One `key=value` observation on stdout, the format `jmap_functional::
 * observations` parses. */
static void
observe (const gchar *key,
         const gchar *value)
{
	g_print ("%s=%s\n", key, value);
}

static void
observe_boolean (const gchar *key,
                 gboolean value)
{
	observe (key, value ? "1" : "0");
}

/* Fail the run, naming what broke. Used only for this program's own
 * preconditions — the behaviour under test is always reported, never judged
 * here. */
static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* Counted rather than acted on: in the running application this signal is
 * what puts the credentials prompter — and, for an OAuth2-method source, the
 * consent window — in front of the user. Item 23 is the claim that a merely
 * stale access token must never reach it. */
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

/* Whether the registry exported `Source.OAuth2Support` for this source. The
 * load-bearing precondition, asserted by the Rust side for the reason item
 * 22's own test asserts it: without the interface the token fetch takes
 * `e-source.c`'s in-process fallback, which in a calendar factory finds no
 * `[JMAP OAuth2]` extension type registered at all (`jmap-backend-cal`'s
 * module registers the OAuth2 *service* but never calls
 * `jmap_config::oauth2::ensure_registered`) and so could never produce a
 * refresh URI. A run without it would measure something else entirely. */
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

/* A minimal VEVENT with the summary and uid given. Built as text rather than
 * through the setters because nothing here is about the mapping — the event
 * is a reason to make the backend talk to the server, and the smallest one
 * that libical will accept is the least that can go wrong in between. */
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

/* Create one event and report whether EDS accepted it, plus the message it
 * refused with. Never fatal: the second call's failure is a result this test
 * has two opposite expectations of, depending on which token the harness let
 * the server keep. */
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

/* Iterate this thread's default main context for `milliseconds`, so queued
 * idles — the only way ESource applies anything it learned over D-Bus — get
 * to run. Not a wait for a condition: this is used where the *absence* of an
 * event is the observation, so it always burns the whole window. */
static void
settle (guint milliseconds)
{
	gint64 deadline = g_get_monotonic_time () + milliseconds * G_TIME_SPAN_MILLISECOND;

	while (g_get_monotonic_time () < deadline) {
		g_main_context_iteration (NULL, FALSE);
		/* Microseconds: g_usleep's unit, and G_TIME_SPAN_MILLISECOND's. */
		g_usleep (10 * G_TIME_SPAN_MILLISECOND);
	}
}

/* Block until `path` exists, iterating the main context meanwhile so that
 * anything the registry or factory has to say still reaches this process.
 * Returns FALSE if the harness never got there. */
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

	/* Before anything connects: the stored refresh token an account that has
	 * been consented to once already has. Everything this test is about
	 * happens downstream of EDS being able to mint an access token silently,
	 * and this is the only thing that makes that possible. */
	if (!e_secret_store_store_sync (secret_uid, secret_json,
					"Evolution Data Source - jmap-functional",
					TRUE, NULL, &error))
		return fail ("seed-secret", error);

	/* Activates evolution-source-registry on the session bus, which reads the
	 * scratch sources directory the harness wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	observe_boolean ("oauth2-support-exported", oauth2_support_exported (source));

	/* Connected before the first connect, so that a consent escalation
	 * raised by the connect itself would be counted too — the connect
	 * fetches a token of its own and a failure there is a different bug
	 * from the one under test. */
	g_signal_connect (source, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);

	/* Activates evolution-calendar-factory and keeps a live backend instance
	 * for the rest of this process's life. This is where the pooled
	 * connection item 23 is about is built, and the access token it carries
	 * is fetched here, once. */
	client = e_cal_client_connect_sync (source, E_CAL_CLIENT_SOURCE_TYPE_EVENTS,
					    (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	cal = E_CAL_CLIENT (client);

	functional_report_connection_status (source, 10);

	/* The operation before the token goes stale, and this program's own
	 * precondition: a backend that could not write at all would produce the
	 * same "second create failed" this test is otherwise about. */
	if (!create_event (cal, "jmap-functional-stale-token-1", "Before the rotation",
			   "first-create-ok", "first-create-error"))
		return 1;

	g_print ("credentials-required-before-rotation=%u\n", credentials_required);

	/* Meet the harness. It rotates the server's accepted bearer token — and,
	 * in the positive case, what its token endpoint hands out next — only
	 * once the connection above exists, because the token has to go stale
	 * BETWEEN two of this client's own calls. That is the shape of the bug. */
	if (!g_file_set_contents (ready_path, "ready", -1, &error))
		return fail ("write-ready", error);

	if (!wait_for_file (go_path, GO_TIMEOUT_SECONDS)) {
		g_printerr ("the harness never wrote %s\n", go_path);
		return 1;
	}

	/* The whole question. */
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
