/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of `docs/ROADMAP.md` item 25's ADDRESS-BOOK leg: item 23's
 * hourly re-consent, driven through a real `evolution-addressbook-factory`
 * and a real `evolution-source-registry` rather than through the operator
 * leaving Evolution open for an afternoon.
 *
 * An ordinary libebook consumer. It opens a book — which is what makes the
 * factory dlopen `libebookbackendjmap.so` and keep a live, connected
 * `EBookMetaBackend` instance holding one pooled JMAP connection — creates a
 * contact, waits at a two-file handshake while the harness makes the server
 * refuse the access token that connection was built with, and creates a
 * second contact. Whether that second create succeeds, and at what cost, is
 * the whole question; this program answers nothing about it. Every judgement
 * belongs to `rust/crates/jmap-functional/tests/book-stale-token.rs`, which
 * runs this program and reads the `key=value` observations it writes to
 * stdout.
 *
 * This is `tests/functional/cal-stale-token-client.c` with `libecal` swapped
 * for `libebook` — the mechanism and every constraint in its header apply
 * unchanged: `jmap_backend_core::oauth2::source_uses_oauth2` is the same
 * function for both backends, so the account must say `Method=JMAP`
 * (`e_oauth2_services_is_oauth2_alias` matches only a registered service),
 * which is exactly the condition under which EDS's own
 * `module-oauth2-services.so` exports `Source.OAuth2Support` and routes the
 * factory's `e_source_get_oauth2_access_token_sync` over D-Bus to the
 * registry, against this project's own `EOAuth2Service::get_refresh_uri` and
 * `[JMAP OAuth2]`'s `TokenEndpoint`. The refresh is gap-driven rather than
 * clock-driven for the same reason: a seeded secret carrying only a
 * `refresh_token` leaves `eos_lookup_token_sync`'s `expires_in` at -1, so
 * `e_oauth2_service_get_access_token_sync` (`<= TOKEN_VALIDITY_GAP_SECS`, 10)
 * refreshes on every fetch, with no wall-clock window to lose.
 *
 *   usage: functional-book-stale-token-client <source-uid> <secret-uid> \
 *              <secret-json> <ready-path> <go-path>
 */

#include <libebook/libebook.h>

#include "connection-status.h"

/* See `cal-stale-token-client.c`'s own copy of this macro for why it is
 * named as a string rather than through EDS's generated accessors. */
#define OAUTH2_SUPPORT_INTERFACE "org.gnome.evolution.dataserver.Source.OAuth2Support"

/* See `cal-stale-token-client.c`. */
#define GO_TIMEOUT_SECONDS 45
#define SETTLE_MILLISECONDS 1500

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
 * consent window — in front of the user. */
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

/* Create one contact and report whether EDS accepted it, plus the message it
 * refused with. Never fatal: the second call's failure is a result this test
 * has two opposite expectations of, depending on which token the harness let
 * the server keep. */
static gboolean
create_contact (EBookClient *book,
                const gchar *full_name,
                const gchar *ok_key,
                const gchar *error_key)
{
	EContact *contact;
	GError *error = NULL;
	gchar *added_uid = NULL;
	gboolean ok;

	contact = e_contact_new ();
	e_contact_set (contact, E_CONTACT_FULL_NAME, full_name);

	ok = e_book_client_add_contact_sync (book, contact, E_BOOK_OPERATION_FLAG_NONE,
					     &added_uid, NULL, &error);
	g_object_unref (contact);

	observe_boolean (ok_key, ok);
	if (!ok)
		observe (error_key, error && error->message ? error->message : "(no error set)");

	g_free (added_uid);
	g_clear_error (&error);

	return ok;
}

/* See `cal-stale-token-client.c`'s own copy: iterates this thread's default
 * main context for `milliseconds`, so queued idles — the only way ESource
 * applies anything it learned over D-Bus — get to run. */
static void
settle (guint milliseconds)
{
	gint64 deadline = g_get_monotonic_time () + milliseconds * G_TIME_SPAN_MILLISECOND;

	while (g_get_monotonic_time () < deadline) {
		g_main_context_iteration (NULL, FALSE);
		g_usleep (10 * G_TIME_SPAN_MILLISECOND);
	}
}

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
	EBookClient *book;
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
	 * been consented to once already has. */
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
	 * raised by the connect itself would be counted too. */
	g_signal_connect (source, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);

	/* Activates evolution-addressbook-factory and keeps a live backend
	 * instance for the rest of this process's life. This is where the
	 * pooled connection item 23 is about is built, and the access token it
	 * carries is fetched here, once. */
	client = e_book_client_connect_sync (source, (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	book = E_BOOK_CLIENT (client);

	functional_report_connection_status (source, 10);

	/* The operation before the token goes stale, and this program's own
	 * precondition: a backend that could not write at all would produce the
	 * same "second create failed" this test is otherwise about. */
	if (!create_contact (book, "Before the rotation",
			     "first-create-ok", "first-create-error"))
		return 1;

	g_print ("credentials-required-before-rotation=%u\n", credentials_required);

	/* Meet the harness. It rotates the server's accepted bearer token — and,
	 * in the positive case, what its token endpoint hands out next — only
	 * once the connection above exists, because the token has to go stale
	 * BETWEEN two of this client's own calls. */
	if (!g_file_set_contents (ready_path, "ready", -1, &error))
		return fail ("write-ready", error);

	if (!wait_for_file (go_path, GO_TIMEOUT_SECONDS)) {
		g_printerr ("the harness never wrote %s\n", go_path);
		return 1;
	}

	/* The whole question. */
	create_contact (book, "After the rotation",
			"second-create-ok", "second-create-error");

	settle (SETTLE_MILLISECONDS);

	g_print ("credentials-required=%u\n", credentials_required);
	g_print ("done=1\n");

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
