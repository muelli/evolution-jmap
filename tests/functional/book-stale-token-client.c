/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client part of the address-book test for a stale access token (the
 * hourly re-consent). It uses a real `evolution-addressbook-factory` and a
 * real `evolution-source-registry`.
 *
 * An ordinary libebook consumer. It opens a book, which makes the factory
 * dlopen `libebookbackendjmap.so` and keep a connected `EBookMetaBackend` with
 * one pooled JMAP connection. It creates a contact. Then it waits at a
 * two-file handshake while the test makes the server refuse the access token
 * of that connection. Then it creates a second contact. This program only
 * writes `key=value` observations to stdout.
 * `rust/crates/jmap-functional/tests/book-stale-token.rs` makes all decisions.
 *
 * This is `tests/functional/cal-stale-token-client.c` with `libebook` in
 * place of `libecal`. The mechanism and the constraints in its header apply:
 * `jmap_backend_core::oauth2::source_uses_oauth2` is the same for both
 * backends, so the account must have `Method=JMAP`
 * (`e_oauth2_services_is_oauth2_alias` matches only a registered service).
 * With that method, the EDS `module-oauth2-services.so` exports
 * `Source.OAuth2Support` and sends the factory's
 * `e_source_get_oauth2_access_token_sync` over D-Bus to the registry. The
 * registry uses `EOAuth2Service::get_refresh_uri` from this project and the
 * `TokenEndpoint` of `[JMAP OAuth2]`.
 *
 * The refresh depends on the validity gap, not on the clock. A stored secret
 * with only a `refresh_token` makes `eos_lookup_token_sync` set `expires_in`
 * to -1. Thus `e_oauth2_service_get_access_token_sync`
 * (`<= TOKEN_VALIDITY_GAP_SECS`, 10) refreshes on each fetch, with no
 * time window to miss.
 *
 *   usage: functional-book-stale-token-client <source-uid> <secret-uid> \
 *              <secret-json> <ready-path> <go-path>
 */

#include <libebook/libebook.h>

#include "connection-status.h"
#include "test-util.h"

/* `cal-stale-token-client.c` tells why this is a string and not an EDS
 * generated accessor. */
#define OAUTH2_SUPPORT_INTERFACE "org.gnome.evolution.dataserver.Source.OAuth2Support"

/* See `cal-stale-token-client.c`. */
#define GO_TIMEOUT_SECONDS 45
#define SETTLE_MILLISECONDS 1500

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

/* Only count this signal. In Evolution, it shows the credentials prompter (and
 * the consent window for an OAuth2 source) to the user. */
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

/* Create one contact and report if EDS accepted it, or the refusal message.
 * A failure is not fatal: the test expects either result from the second
 * call, related to the token that the server accepts. */
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

/* Iterates the default main context of this thread for `milliseconds`, so
 * that queued idles run. ESource applies D-Bus updates only from idles. */
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

	/* Before the connect, store the refresh token that an account has after
	 * one consent. */
	if (!e_secret_store_store_sync (secret_uid, secret_json,
					"Evolution Data Source - jmap-functional",
					TRUE, NULL, &error))
		return functional_fail ("seed-secret", error);

	/* Starts evolution-source-registry on the session bus. It reads the
	 * temporary sources directory that the test wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	observe_boolean ("oauth2-support-exported", oauth2_support_exported (source));

	/* Connect the signal before the first connect, to count a consent request
	 * from the connect too. */
	g_signal_connect (source, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);

	/* Starts evolution-addressbook-factory, with a backend instance that
	 * stays for the life of this process. This builds the pooled connection
	 * and gets its access token once. The test rotates that token later. */
	client = e_book_client_connect_sync (source, (guint32) -1, NULL, &error);
	if (!client)
		return functional_fail ("connect", error);

	book = E_BOOK_CLIENT (client);

	functional_report_connection_status (source, 10);

	/* The create before the token becomes stale. It is also a precondition: a
	 * backend that cannot write also causes "second create failed". */
	if (!create_contact (book, "Before the rotation",
			     "first-create-ok", "first-create-error"))
		return 1;

	g_print ("credentials-required-before-rotation=%u\n", credentials_required);

	/* Synchronize with the test. It rotates the bearer token that the server
	 * accepts (and, in the positive case, the next token from its token
	 * endpoint) after the connection exists. The token must become stale
	 * BETWEEN two calls of this client. */
	if (!g_file_set_contents (ready_path, "ready", -1, &error))
		return functional_fail ("write-ready", error);

	if (!wait_for_file (go_path, GO_TIMEOUT_SECONDS)) {
		g_printerr ("the harness never wrote %s\n", go_path);
		return 1;
	}

	/* The operation that the test examines. */
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
