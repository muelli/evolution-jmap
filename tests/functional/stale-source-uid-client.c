/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the stale-source-uid question: can a source UID that is
 * only in dconf/GSettings cause a credential lookup in
 * `evolution-source-registry`, and then a consent window?
 *
 * The operator saw this two times with `ESR_DEBUG=1`:
 *
 *   server_side_source_credentials_lookup_cb: Failed to lookup password for
 *   source d06b56726065b3531de05498dd3519c2bd6efcb3
 *
 * In the same second, an interactive `authorization_code` consent followed.
 * The UID was not in `~/.config/evolution/sources/`. The only file on the
 * machine with the UID was `~/.config/dconf/user`.
 *
 * This program examines the two parts of that problem separately, and does
 * not judge them. `rust/crates/jmap-functional/tests/stale-source-uid.rs`
 * runs this program and asserts on the `key=value` lines on stdout.
 *
 * MODE `dconf-only`: write the stale UID and see if it has an effect.
 * The program writes a UID that no file names into all six keys of the
 * `org.gnome.Evolution.DefaultSources` schema (`e-source-registry.c:68`, the
 * only EDS mechanism that keeps source UIDs in dconf). It syncs the value to
 * `$XDG_CONFIG_HOME/dconf/user` and checks the file, thus a run that uses the
 * GSettings memory backend reports this and does not pass. Only THEN does it
 * start the registry, thus the setting exists when the daemon starts. It
 * reports if the registry has such a source and what the default-source
 * getters return.
 *
 * MODE `collection-child`: reproduce the log line.
 * `collection_backend_new_user_file` (`e-collection-backend.c:176-200`)
 * writes the children of a collection backend to
 * `$XDG_CACHE_HOME/evolution/sources/<collection-uid>/`, NOT to the config
 * directory. Thus "the UID matches no configured source" is normal for each
 * child source of an account. This mode waits for the children of the JMAP
 * collection backend. Then it sends one child through the path of the
 * operator: `e_source_invoke_credentials_required_sync (reason=required)`,
 * which `e_backend_schedule_credentials_required()` calls for a backend that
 * needs credentials.
 *
 * Why the signal counts are the measurement.
 * `server_side_source_invoke_credentials_required_cb`
 * (`e-server-side-source.c:252`) does NOT only send the request to clients.
 * For reason `required`, it sets `skip_emit = TRUE` (line 318) and starts a
 * silent `e_source_credentials_provider_lookup`. The callback of the lookup
 * selects what the user sees. Thus this process receives one of two results:
 *
 *   - The lookup found a password: `authenticate`, and NO
 *     `credentials-required`. The user sees nothing.
 *   - The lookup failed: the log line of the operator, and a new
 *     `credentials-required` with reason `required` (line 490 and after).
 *     The prompter opens, and for an OAuth2 source, the consent window.
 *
 * Thus the two counts are a real check. This mode takes an optional password:
 * without it and with it, the two counts change places. A test of only the
 * failure cannot tell the escalation from a simple echo of the request.
 *
 *   usage: functional-stale-source-uid-client dconf-only <dangling-uid>
 *          functional-stale-source-uid-client collection-child <account-uid>
 *                                             [<password-to-store-first>]
 */

#include <libedataserver/libedataserver.h>

#include "test-util.h"

/* The schema in which `ESourceRegistry` keeps the default sources. The six
 * keys are written out (not read from the schema), thus a key that upstream
 * removes makes this run fail with its name. From `e-source-registry.c:68`
 * and `/usr/share/glib-2.0/schemas/org.gnome.Evolution.DefaultSources.gschema.xml`. */
#define DEFAULT_SOURCES_SCHEMA "org.gnome.Evolution.DefaultSources"

/* The maximum wait for the collection backend to make children. Same value as
 * in `collection-client.c`. */
#define WAIT_SECONDS 30

/* How long to iterate the default main context after the credentials request,
 * to receive the result from the registry. ESource applies D-Bus changes from
 * an idle on the thread-default context of the `ESourceRegistry`. Only
 * iteration delivers the signals. This always uses the full time, because the
 * *absence* of a signal is also a measurement. */
#define SETTLE_MILLISECONDS 2500

/* The maximum wait for the `ca.desrt.dconf` service to write the database
 * file after `g_settings_sync`. Usually this takes some milliseconds. If
 * GSettings does not use dconf, the limit makes the run report it, not hang. */
#define DCONF_WRITE_TIMEOUT_SECONDS 5

/* Return TRUE if `needle` is in the `length` bytes at `haystack`.
 *
 * Do not use `g_strstr_len`: its loop `while (p <= end && *p)` stops at the
 * first NUL byte. The dconf database is a binary GVDB file with NUL bytes in
 * its header, thus `g_strstr_len` does not find a UID that is in the file. */
static gboolean
contains_bytes (const gchar *haystack,
                gsize length,
                const gchar *needle)
{
	gsize needle_length = strlen (needle);
	gsize offset;

	if (needle_length == 0 || length < needle_length)
		return FALSE;

	for (offset = 0; offset + needle_length <= length; offset++) {
		if (memcmp (haystack + offset, needle, needle_length) == 0)
			return TRUE;
	}

	return FALSE;
}

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
	g_print ("%s=%d\n", key, value ? 1 : 0);
}

/* Iterate the default main context for `milliseconds` (see
 * SETTLE_MILLISECONDS). */
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

/* All UIDs in the registry, comma-separated. The Rust side compares them with
 * the `.source` files. Use one line: `jmap_functional::observations` is a map
 * and keeps only the last value of a repeated key. */
static gchar *
source_uids (ESourceRegistry *registry)
{
	GList *all;
	GList *link;
	GString *uids = g_string_new (NULL);

	all = e_source_registry_list_sources (registry, NULL);
	for (link = all; link != NULL; link = g_list_next (link)) {
		if (uids->len > 0)
			g_string_append_c (uids, ',');
		g_string_append (uids, e_source_get_uid (E_SOURCE (link->data)));
	}
	g_list_free_full (all, g_object_unref);

	return g_string_free (uids, FALSE);
}

/* ---------------------------------------------------------------- dconf-only */

static int
run_dconf_only (const gchar *dangling_uid)
{
	/* All keys in the schema. The operator had a mail account, but the same
	 * question applies to the other defaults. */
	static const gchar * const keys[] = {
		"default-address-book",
		"default-calendar",
		"default-mail-account",
		"default-mail-identity",
		"default-memo-list",
		"default-task-list"
	};
	GSettings *settings;
	ESourceRegistry *registry;
	ESource *source;
	ESource *default_mail_account;
	GError *error = NULL;
	gchar *dconf_path;
	gchar *dconf_contents = NULL;
	gsize dconf_length = 0;
	gint64 deadline;
	gchar *uids;
	guint ii;

	settings = g_settings_new (DEFAULT_SOURCES_SCHEMA);
	for (ii = 0; ii < G_N_ELEMENTS (keys); ii++) {
		if (!g_settings_set_string (settings, keys[ii], dangling_uid)) {
			g_printerr ("g_settings_set_string: %s refused the UID\n", keys[ii]);
			g_object_unref (settings);

			return 1;
		}
	}
	/* Send the write to the backend now. Without this, the write stays in
	 * the queue of this process and `~/.config/dconf/user` does not have the
	 * UID yet. */
	g_settings_sync ();
	g_object_unref (settings);

	/* The finding of the operator: the UID is only in `~/.config/dconf/user`.
	 * Report it. If GSettings uses the memory backend (no dconf GIO module, or
	 * no `ca.desrt.dconf` on this private bus), the precondition on the Rust
	 * side fails. Otherwise this mode measures nothing. */
	dconf_path = g_build_filename (g_get_user_config_dir (), "dconf", "user", NULL);
	deadline = g_get_monotonic_time () + DCONF_WRITE_TIMEOUT_SECONDS * G_TIME_SPAN_SECOND;
	for (;;) {
		gboolean found = FALSE;

		if (g_file_get_contents (dconf_path, &dconf_contents, &dconf_length, NULL)) {
			found = contains_bytes (dconf_contents, dconf_length, dangling_uid);
			g_free (dconf_contents);
			dconf_contents = NULL;
		}

		if (found || g_get_monotonic_time () >= deadline) {
			observe_boolean ("dconf-file-names-uid", found);
			break;
		}

		/* `g_settings_sync` sends the queue to the dconf backend, but the
		 * `ca.desrt.dconf` service writes the file later. Usually the first
		 * read finds it. Poll, so that a slow machine gives a correct result. */
		g_main_context_iteration (NULL, FALSE);
		g_usleep (20 * G_TIME_SPAN_MILLISECOND);
	}
	observe_boolean ("dconf-file-present", g_file_test (dconf_path, G_FILE_TEST_EXISTS));
	g_free (dconf_path);

	/* Start the registry only now. The setting exists when the daemon starts:
	 * the question is if a new registry does anything with the stale UID. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, dangling_uid);
	observe_boolean ("dangling-uid-in-registry", source != NULL);
	g_clear_object (&source);

	/* `e_source_registry_ref_default_mail_account` (`e-source-registry.c:3300`)
	 * reads the dconf UID and has a documented fallback: it calls
	 * `e_source_registry_ref_source` on the UID, and if that returns NULL,
	 * it uses the built-in account. If it made a source, EDS changes a stale
	 * dconf UID into a live source. */
	default_mail_account = e_source_registry_ref_default_mail_account (registry);
	observe ("default-mail-account-uid",
		 default_mail_account ? e_source_get_uid (default_mail_account) : "(none)");
	g_clear_object (&default_mail_account);

	uids = source_uids (registry);
	observe ("source-uids", uids);
	g_free (uids);

	/* Wait the same time as the other mode, so that "no change" is a real
	 * measurement. */
	settle (SETTLE_MILLISECONDS);

	uids = source_uids (registry);
	observe ("source-uids-after-settling", uids);
	g_free (uids);

	observe_boolean ("done", TRUE);
	g_object_unref (registry);

	return 0;
}

/* ----------------------------------------------------------- collection-child */

/* Count the signals. In the application, `credentials-required` shows the
 * credentials prompter (for an OAuth2 source, the consent window).
 * `authenticate` is the silent path: the registry found a stored secret and
 * gave it to the backend. */
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

static void
authenticate_cb (ESource *source,
                 const ENamedParameters *credentials,
                 gpointer user_data)
{
	guint *count = user_data;

	(*count)++;
}

/* The children of `parent_uid` in the registry. Poll, as in
 * `collection-client.c`: the backend writes the children to disk, and the file
 * monitor of the registry reads them. */
static GList *
children_of (ESourceRegistry *registry,
             const gchar *parent_uid)
{
	GList *all;
	GList *link;
	GList *children = NULL;

	all = e_source_registry_list_sources (registry, NULL);
	for (link = all; link != NULL; link = g_list_next (link)) {
		ESource *source = E_SOURCE (link->data);

		if (g_strcmp0 (e_source_get_parent (source), parent_uid) == 0)
			children = g_list_prepend (children, g_object_ref (source));
	}
	g_list_free_full (all, g_object_unref);

	return children;
}

static GList *
wait_for_children (ESourceRegistry *registry,
                   const gchar *parent_uid)
{
	gint64 deadline = g_get_monotonic_time () + WAIT_SECONDS * G_TIME_SPAN_SECOND;
	GList *children;

	for (;;) {
		children = children_of (registry, parent_uid);
		if (children != NULL || g_get_monotonic_time () >= deadline)
			return children;

		g_main_context_iteration (NULL, FALSE);
		g_usleep (100 * G_TIME_SPAN_MILLISECOND);
	}
}

static int
run_collection_child (const gchar *account_uid,
                      const gchar *password)
{
	ESourceRegistry *registry;
	ESourceCredentialsProvider *provider;
	ESource *account;
	ESource *child;
	ESource *credentials_source;
	GList *children;
	GError *error = NULL;
	guint credentials_required = 0;
	guint authenticate = 0;

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	account = e_source_registry_ref_source (registry, account_uid);
	if (!account) {
		g_printerr ("registry: no source with UID '%s'\n", account_uid);

		return 1;
	}
	observe_boolean ("account-found", TRUE);

	children = wait_for_children (registry, account_uid);
	g_print ("children-found=%u\n", g_list_length (children));
	if (!children) {
		g_printerr ("the collection backend produced no children in %d seconds\n",
			    WAIT_SECONDS);

		return 1;
	}

	/* Any child: all are in the cache directory. Report its UID, so that the
	 * Rust side can find the keyfile itself. */
	child = E_SOURCE (children->data);
	observe ("child-uid", e_source_get_uid (child));
	observe ("child-display-name", e_source_get_display_name (child));

	/* Connect before the store below, to count its signals too. */
	g_signal_connect (child, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);
	g_signal_connect (child, "authenticate",
			  G_CALLBACK (authenticate_cb), &authenticate);

	/* Report WHICH source holds the credentials of this child.
	 * `source_credential_provider_ref_impl_for_source`
	 * (`e-source-credentials-provider.c:216-237`) does not use the given
	 * source for the lookup. It first calls
	 * `e_source_credentials_provider_ref_credentials_source`, which goes up
	 * the `Parent=` chain to the nearest source with `[Collection]`
	 * (line 416-437): "the credentials are usually stored on the collection
	 * source, thus shared between child sources".
	 *
	 * But the failure message uses `data->source`, the CHILD
	 * (`e-server-side-source.c:460-462`). Thus `Failed to lookup password
	 * for source <uid>` names a source that is different from the source of
	 * the lookup. This is why the UID was unknown to the operator. The Rust
	 * side asserts that the two UIDs are different. */
	provider = e_source_credentials_provider_new (registry);
	credentials_source = e_source_credentials_provider_ref_credentials_source (provider, child);
	observe ("credentials-source-uid",
		 e_source_get_uid (credentials_source ? credentials_source : child));

	/* The control: store the password where the lookup reads.
	 * `e_source_store_password_sync` writes to the secret store under the UID
	 * of the source (`e-source.c:4062-4083`), and
	 * `e_source_credentials_provider_impl_password_lookup_sync` reads it. This
	 * is the only difference between the two runs. */
	observe_boolean ("password-stored", password != NULL);
	if (password != NULL &&
	    !e_source_store_password_sync (credentials_source ? credentials_source : child,
					   password, TRUE, NULL, &error))
		return functional_fail ("store-password", error);

	g_clear_object (&credentials_source);
	g_object_unref (provider);

	/* The path of the operator, called as
	 * `e_backend_schedule_credentials_required()` calls it for a backend that
	 * needs credentials. This program is in place of that backend, as
	 * `oauth2-stale-proxy-client.c` is in place of a long-running factory. */
	if (!e_source_invoke_credentials_required_sync (child,
						       E_SOURCE_CREDENTIALS_REASON_REQUIRED,
						       NULL, 0, NULL, NULL, &error))
		return functional_fail ("invoke-credentials-required", error);

	settle (SETTLE_MILLISECONDS);

	g_print ("credentials-required=%u\n", credentials_required);
	g_print ("authenticate=%u\n", authenticate);
	observe_boolean ("done", TRUE);

	g_list_free_full (children, g_object_unref);
	g_object_unref (account);
	g_object_unref (registry);

	return 0;
}

gint
main (gint argc,
      gchar **argv)
{
	const gchar *mode;

	if (argc < 3) {
		g_printerr ("usage: %s dconf-only <dangling-uid>\n"
			    "       %s collection-child <account-uid> [<password>]\n",
			    argv[0], argv[0]);

		return 2;
	}
	mode = argv[1];

	if (g_strcmp0 (mode, "dconf-only") == 0 && argc == 3)
		return run_dconf_only (argv[2]);

	if (g_strcmp0 (mode, "collection-child") == 0 && argc == 3)
		return run_collection_child (argv[2], NULL);

	if (g_strcmp0 (mode, "collection-child") == 0 && argc == 4)
		return run_collection_child (argv[2], argv[3]);

	g_printerr ("%s: unknown mode '%s' or wrong argument count\n", argv[0], mode);

	return 2;
}
