/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of `docs/ROADMAP.md` item 41's reproduction: two JMAP
 * accounts that share one `[Authentication] User` string collide on one
 * OAuth2 secret-store slot, because EDS derives that slot's key from
 * `(service name, user)` alone.
 *
 * `eos_generate_secret_uid` (evolution-data-server 3.52.3,
 * `src/libedataserver/e-oauth2-service.c:1086`, read rather than assumed) is:
 *
 *     g_strdup_printf ("OAuth2::%s[%s]", e_oauth2_service_get_name (service), user)
 *
 * No host. Every in-tree `EOAuth2Service` (Google, Outlook, Yahoo, ...) names
 * exactly one cloud, so `(service, user)` is unique there. JMAP is
 * multi-server: an account at one deployment and an account at a completely
 * different one, both authenticating as the same address, derive the
 * identical key.
 *
 * This program is one plain `ESourceRegistry`/`ESource` consumer, run twice
 * by `rust/crates/jmap-functional/tests/oauth2-token-collision.rs` against
 * two accounts that share a `[Authentication] User` but point at two
 * different mock JMAP deployments (two independent `jmap_mock::MockServer`s,
 * standing in for two real, unrelated servers — Fastmail and a self-hosted
 * Stalwart, in the scenario `docs/ROADMAP.md` item 41 names). It knows
 * nothing about which run it is; the Rust side decides that by which source
 * UID and secret JSON it is handed, and reads every judgement off this
 * program's `key=value` stdout.
 *
 *   usage: functional-oauth2-token-collision-client <source-uid> \
 *              <secret-uid> <secret-json-or-empty>
 *
 * When `<secret-json-or-empty>` is non-empty, it is stored under
 * `<secret-uid>` before anything else happens — the "this account has been
 * consented to before and holds a refresh token" precondition the first run
 * (account A) needs. When it is empty, nothing is stored: the second run
 * (account B) asks for a token exactly as a freshly added account normally
 * would, having consented to nothing at all, and whatever the secret store
 * already holds under the (host-blind) shared key is what it gets.
 */

#include <libedataserver/libedataserver.h>

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
		/* Account A's precondition: a stored refresh token, as an account
		 * that has already been consented to once carries. Stored under
		 * `secret_uid` directly rather than through this account's own
		 * `EOAuth2Service`, for the same reason `cal-stale-token-client.c`
		 * seeds this way: `e_secret_store_store_sync` talks to the
		 * `org.freedesktop.secrets` provider on the session bus this
		 * process was started on. */
		if (!e_secret_store_store_sync (secret_uid, secret_json,
						"Evolution Data Source - jmap-functional",
						TRUE, NULL, &error))
			fatal ("seed-secret", error);
	}

	/* Activates evolution-source-registry on the session bus, which reads
	 * the scratch sources directory the harness wrote and loads
	 * module-jmap-backend.so (our "JMAP" EOAuth2Service) and EDS's own
	 * module-oauth2-services.so (whose EOAuth2SourceMonitor is what exports
	 * Source.OAuth2Support for a Method=JMAP account, routing the token
	 * fetch below into the registry process rather than in-process). */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		fatal ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	observe_boolean ("source-found", source != NULL);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);

		return 1;
	}

	/* The reproduction. `eos_lookup_token_sync` derives the very same
	 * `secret_uid` this program's caller derived by hand (same service
	 * name, same [Authentication] User) — regardless of what this
	 * account's own [Authentication] Host or [JMAP OAuth2] TokenEndpoint
	 * say — and looks *that* slot up. */
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
