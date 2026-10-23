/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * An acceptance test: a 401 that occurs after a redirect removed the
 * `Authorization` header is not a rejected password.
 * `jmap_client::Error::RedirectCredentialsWithheld`
 * (`rust/crates/jmap-client/src/error.rs`) is not an HTTP 401 error, thus
 * `jmap_backend_core::connect::is_wrong_password` returns false for it. This
 * test does a real `EBookMetaBackend` connect through this scenario. The
 * operator's report was about a client that repeatedly asked for OAuth2
 * consent, not about an address book.
 *
 * This client is a usual libebook consumer, like `book-client.c`. The
 * account host and port name a small front door that the Rust side runs.
 * The front door redirects each request to a different origin (a different
 * port is sufficient: see `jmap_client::url::same_origin`). That origin
 * answers with a 401, because the redirect removed the credentials.
 *
 * The first connect: EDS calls `connect_sync` once with no credentials,
 * before it reads the secret store (see the doc of
 * `jmap_backend_core::connect::credentials`). This attempt always fails with
 * `CredentialsRequired` (`E_SOURCE_AUTHENTICATION_REQUIRED`) and sends no
 * request to the front door. A real credentials prompter answers the
 * `credentials-required` signal with `e_source_invoke_authenticate_sync` and
 * the password. This second connect is the subject of the test: it gets the
 * redirect and the 401, and `RedirectCredentialsWithheld` must not cause a
 * new prompt. EDS `ensure_connected_sync` sends `credentials-required` from
 * an idle callback. Thus the client iterates the default main context
 * (`settle`) after each attempt, before it reads the count.
 *
 *   usage: functional-redirect-credentials-withheld-client <source-uid> <password>
 */

#include <libebook/libebook.h>

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

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* Count the signal, as `book-stale-token-client.c` does. In the application,
 * this signal shows the credentials prompter. The first connect sends it
 * once (no password yet). The 401 on the second attempt must not send it
 * again. */
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

/* `e_backend_schedule_credentials_required` sends its signal from an idle
 * callback. Iterate the main context after a sync call, or the count is old.
 * Same value as in `book-stale-token-client.c`. */
#define SETTLE_MILLISECONDS 1500

static void
settle (guint milliseconds)
{
	gint64 deadline = g_get_monotonic_time () + milliseconds * G_TIME_SPAN_MILLISECOND;

	while (g_get_monotonic_time () < deadline) {
		g_main_context_iteration (NULL, FALSE);
		g_usleep (10 * G_TIME_SPAN_MILLISECOND);
	}
}

/* Answer with the password, as a real credentials prompter does. With `NULL`,
 * EDS gets the credentials itself, and here (no Shell, no
 * default-credentials-required listener) it gets nothing. */
static gboolean
reauthenticate_with_password (ESource *source,
                              const gchar *password,
                              GError **error)
{
	ENamedParameters *credentials;
	gboolean ok;

	credentials = e_named_parameters_new ();
	e_named_parameters_set (credentials, E_SOURCE_CREDENTIAL_PASSWORD, password);
	ok = e_source_invoke_authenticate_sync (source, credentials, NULL, error);
	e_named_parameters_free (credentials);

	return ok;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	const gchar *source_uid;
	const gchar *password;
	guint credentials_required = 0;

	if (argc != 3) {
		g_printerr ("usage: %s <source-uid> <password>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	password = argv[2];

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

	/* The second attempt uses this password. The first attempt does not read
	 * it (see the file header). */
	if (!e_source_store_password_sync (source, password, TRUE, NULL, &error))
		return fail ("store-password", error);

	g_signal_connect (source, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);

	/* This starts evolution-addressbook-factory, which loads
	 * libebookbackendjmap.so. The backend connect fails with no password and
	 * sends no request. connect-ok is only for diagnosis: EDS gives a client
	 * and no GError also after a `CredentialsRequired` failure. */
	client = e_book_client_connect_sync (source, (guint32) -1, NULL, &error);
	settle (SETTLE_MILLISECONDS);
	observe_boolean ("connect-ok", client != NULL);
	g_print ("credentials-required-after-connect=%u\n", credentials_required);
	g_clear_error (&error);

	if (!client) {
		g_object_unref (source);
		g_object_unref (registry);
		return 1;
	}

	/* The second attempt gets the redirect and the 401. Give one answer, as a
	 * real credentials prompter gives one answer for each prompt. */
	if (reauthenticate_with_password (source, password, &error)) {
		observe_boolean ("reauthenticate-ok", TRUE);
	} else {
		observe_boolean ("reauthenticate-ok", FALSE);
		observe ("reauthenticate-error", error && error->message ? error->message : "(no error set)");
		g_clear_error (&error);
	}
	settle (SETTLE_MILLISECONDS);
	g_print ("credentials-required-after-reauthenticate=%u\n", credentials_required);

	/* Wait again with no new request. A count that increases here shows that
	 * the backend asks for credentials again by itself: the retry storm that
	 * this test must find. */
	settle (SETTLE_MILLISECONDS);
	g_print ("credentials-required=%u\n", credentials_required);
	g_print ("done=1\n");

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
