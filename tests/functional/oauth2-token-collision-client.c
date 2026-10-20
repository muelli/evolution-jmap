/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of a reproduction: two JMAP accounts with the same
 * `[Authentication] User` use the same OAuth2 secret-store slot. EDS makes
 * the slot key from `(service name, user)` only.
 *
 * `eos_generate_secret_uid` (evolution-data-server 3.52.3,
 * `src/libedataserver/e-oauth2-service.c:1086`) is:
 *
 *     g_strdup_printf ("OAuth2::%s[%s]", e_oauth2_service_get_name (service), user)
 *
 * The key has no host. Each in-tree `EOAuth2Service` (Google, Outlook,
 * Yahoo, ...) names one cloud, thus `(service, user)` is unique there. JMAP
 * has many servers: two accounts on different servers with the same address
 * get the same key.
 *
 * `rust/crates/jmap-functional/tests/oauth2-token-collision.rs` runs this
 * program twice, for two accounts with the same `[Authentication] User` on
 * two different `jmap_mock::MockServer`s. These are in place of two
 * unrelated real servers (Fastmail and a self-hosted Stalwart). The Rust
 * side selects the run with the source UID and secret JSON. It reads all
 * results from the `key=value` lines on stdout.
 *
 *   usage: functional-oauth2-token-collision-client <source-uid> \
 *              <secret-uid> <secret-json-or-empty>
 *
 * If `<secret-json-or-empty>` is not empty, the program first stores it
 * under `<secret-uid>`. This gives account A (the first run) a refresh token
 * from an earlier consent. If it is empty, the program stores nothing.
 * Account B (the second run) then asks for a token as a new account does,
 * and gets the value that the shared key already holds.
 */

#include <libedataserver/libedataserver.h>

/* Write one `key=value` line to stdout for `jmap_functional::observations`. */
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

static void G_GNUC_NORETURN
fatal (const gchar *what,
       const GError *error)
{
	g_printerr ("%s: %s\n", what, error ? error->message : "(no error set)");
	exit (1);
}

int
main (int argc,
      char **argv)
{
	ESourceRegistry *registry;
	ESource *source;
	GError *error = NULL;
	const gchar *source_uid;
	const gchar *secret_uid;
	const gchar *secret_json;
	gchar *access_token = NULL;
	gint expires_in = 0;
	gboolean fetched;

	if (argc != 4) {
		g_printerr ("usage: %s <source-uid> <secret-uid> <secret-json-or-empty>\n",
			    argv[0]);

		return 2;
	}

	source_uid = argv[1];
	secret_uid = argv[2];
	secret_json = argv[3];

	if (*secret_json != '\0') {
		/* Account A: store a refresh token directly under `secret_uid`,
		 * not through the `EOAuth2Service`. `cal-stale-token-client.c` does
		 * the same: `e_secret_store_store_sync` uses the
		 * `org.freedesktop.secrets` provider on the session bus of this
		 * process. */
		if (!e_secret_store_store_sync (secret_uid, secret_json,
						"Evolution Data Source - jmap-functional",
						TRUE, NULL, &error))
			fatal ("seed-secret", error);
	}

	/* This starts evolution-source-registry on the session bus. It reads the
	 * scratch sources directory and loads module-jmap-backend.so (the "JMAP"
	 * EOAuth2Service) and EDS's module-oauth2-services.so. Its
	 * EOAuth2SourceMonitor exports Source.OAuth2Support for a Method=JMAP
	 * account, thus the token fetch below occurs in the registry process. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		fatal ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	observe_boolean ("source-found", source != NULL);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);

		return 1;
	}

	/* The reproduction. `eos_lookup_token_sync` makes the same `secret_uid`
	 * as the caller (same service name, same [Authentication] User) and
	 * reads that slot. [Authentication] Host and [JMAP OAuth2]
	 * TokenEndpoint have no effect on the key. */
	fetched = e_source_get_oauth2_access_token_sync (
		source, NULL, &access_token, &expires_in, &error);

	observe_boolean ("fetched", fetched);
	if (fetched) {
		observe ("access-token", access_token);
		gchar *expires = g_strdup_printf ("%d", expires_in);
		observe ("expires-in", expires);
		g_free (expires);
	} else if (error) {
		observe ("error-domain", g_quark_to_string (error->domain));
		observe ("error-message", error->message);
	}

	g_print ("done=1\n");

	g_clear_pointer (&access_token, g_free);
	g_clear_error (&error);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
