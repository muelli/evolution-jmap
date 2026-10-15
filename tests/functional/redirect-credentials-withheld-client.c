/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * An acceptance test: a 401 that arrives only after a redirect
 * dropped the `Authorization` header must not look like a rejected
 * password. `jmap_client::Error::RedirectCredentialsWithheld`
 * (`rust/crates/jmap-client/src/error.rs`) already keeps that case out of
 * `is_wrong_password()`, but nothing before this file had ever driven a real
 * `EBookMetaBackend` connect through the scenario end to end: the operator's
 * own report was a client that kept escalating to interactive OAuth2
 * consent, never an address book.
 *
 * This client is an ordinary libebook consumer, like `book-client.c`. The
 * account's host/port name a small front door the Rust side runs, which
 * answers every request with a redirect to a different origin (a different
 * port counts: see `jmap_client::url::same_origin`) that then 401s the
 * request the redirect stripped credentials from.
 *
 * `jmap_backend_core::connect::credentials`'s own doc explains the first
 * connect: EDS calls `connect_sync` once with no credentials at all before
 * ever consulting the secret store, so this attempt always fails with
 * `CredentialsRequired`
 * (`E_SOURCE_AUTHENTICATION_REQUIRED`) without making any network request:
 * the front door is not hit yet. A real credentials prompter answers the
 * `credentials-required` signal that failure schedules by calling
 * `e_source_invoke_authenticate_sync` with the resolved password, which
 * drives the real second connect this test is actually about: that one
 * reaches the front door, follows the redirect, and gets the 401 that
 * `RedirectCredentialsWithheld` must not escalate any further. EDS's own
 * `ensure_connected_sync` posts `credentials-required` through an idle
 * callback rather than firing it synchronously, so this client iterates the
 * default main context (`settle`) after each attempt to give it a chance to
 * run before checking the count.
 *
 *   usage: functional-redirect-credentials-withheld-client <source-uid> <password>
 */

#include <libebook/libebook.h>

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

/* Counted rather than acted on, like `book-stale-token-client.c`'s own copy:
 * in the running application this signal is what puts the credentials
 * prompter in front of the user. The first connect schedules it once, since
 * no password is available yet; a redirect-withheld 401 on the second,
 * credentialed attempt must not schedule it again. */
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

/* See the file header: `e_backend_schedule_credentials_required` posts its
 * signal through an idle callback, so a caller that never iterates the
 * default main context after a sync call would read `credentials_required`
 * as stale. Matches `book-stale-token-client.c`'s own copy. */
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

/* What a real credentials prompter does on `credentials-required`: resolve
 * the password and answer with it explicitly. Passing `NULL` instead tells
 * EDS to resolve credentials itself, which in this headless harness (no
 * Shell, no default-credentials-required listener) resolves to nothing. */
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

	/* Stored ahead of the connect so the second, credentialed attempt below
	 * has a real password to answer with; the first attempt never consults
	 * it (see the file header). */
	if (!e_source_store_password_sync (source, password, TRUE, NULL, &error))
		return fail ("store-password", error);

	g_signal_connect (source, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);

	/* Activates evolution-addressbook-factory, which dlopens
	 * libebookbackendjmap.so. This first attempt is expected to fail with
	 * no password available and no network request made; connect-ok is
	 * diagnostic only; EDS reports it as success regardless (NULL GError on
	 * a `CredentialsRequired` failure is indistinguishable from one on a
	 * genuine connect at this layer). */
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

	/* The real second attempt, through the front door, the redirect, and
	 * the credentials-stripped 401: one explicit answer, the way a real
	 * credentials prompter supplies one answer per prompt. */
	if (reauthenticate_with_password (source, password, &error)) {
		observe_boolean ("reauthenticate-ok", TRUE);
	} else {
		observe_boolean ("reauthenticate-ok", FALSE);
		observe ("reauthenticate-error", error && error->message ? error->message : "(no error set)");
		g_clear_error (&error);
	}
	settle (SETTLE_MILLISECONDS);
	g_print ("credentials-required-after-reauthenticate=%u\n", credentials_required);

	/* No further prompting from this client from here on: settled again
	 * with nothing else asked, to tell a one-time signal from the
	 * reauthenticate call above apart from the backend re-escalating on its
	 * own without being asked again, which is the retry storm this test
	 * exists to catch. */
	settle (SETTLE_MILLISECONDS);
	g_print ("credentials-required=%u\n", credentials_required);
	g_print ("done=1\n");

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
