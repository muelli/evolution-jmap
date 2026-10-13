/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Item 101(b)'s acceptance test: a 401 that arrives only after a redirect
 * dropped the `Authorization` header must not look like a rejected
 * password. `jmap_client::Error::RedirectCredentialsWithheld`
 * (`rust/crates/jmap-client/src/error.rs`) already keeps that case out of
 * `is_wrong_password()`, but nothing before this file had ever driven a real
 * `EBookMetaBackend` connect through the scenario end to end: the operator's
 * own report was a client that kept escalating to interactive OAuth2
 * consent, never an address book.
 *
 * This client is an ordinary libebook consumer, like `book-client.c`, except
 * it expects the connect itself to fail. The account's host/port name a
 * small front door the Rust side runs, which answers every request with a
 * redirect to a different origin (a different port counts: see
 * `jmap_client::url::same_origin`) that then 401s the request the redirect
 * stripped credentials from. Whether `e_book_client_connect_sync` escalated
 * to interactive consent is read off the `credentials-required` signal,
 * counted the same way `book-stale-token-client.c` counts it; whether it
 * looped is the Rust side's question, from the front door's and the mock's
 * own request counts, not this program's.
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
 * prompter in front of the user. A redirect-withheld 401 must never fire
 * it. */
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

	/* Stored the ordinary way EDS looks one up for Basic auth, so the
	 * keyfile's `User=` and this password reach the front door on the very
	 * first request rather than failing `CredentialsRequired` before
	 * anything is sent. */
	if (!e_source_store_password_sync (source, password, TRUE, NULL, &error))
		return fail ("store-password", error);

	/* Connected before the connect itself: a consent escalation the connect
	 * raises directly must be counted too, not just one from a later
	 * operation. */
	g_signal_connect (source, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);

	/* Activates evolution-addressbook-factory, which dlopens
	 * libebookbackendjmap.so and runs the connect this whole test is about.
	 * Expected to fail: the question is how, not whether. */
	client = e_book_client_connect_sync (source, (guint32) -1, NULL, &error);
	if (client) {
		observe_boolean ("connect-ok", TRUE);
		g_object_unref (client);
	} else {
		observe_boolean ("connect-ok", FALSE);
		observe ("connect-error", error && error->message ? error->message : "(no error set)");
		g_clear_error (&error);
	}

	g_print ("credentials-required=%u\n", credentials_required);
	g_print ("done=1\n");

	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
