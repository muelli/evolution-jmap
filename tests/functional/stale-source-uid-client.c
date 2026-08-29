/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of `docs/ROADMAP.md` item 26: does a source UID that exists
 * only in dconf/GSettings drive `evolution-source-registry` into a credential
 * lookup, and thence into a consent window?
 *
 * What the operator saw on 2026-08-29, twice, with `ESR_DEBUG=1` set:
 *
 *   server_side_source_credentials_lookup_cb: Failed to lookup password for
 *   source d06b56726065b3531de05498dd3519c2bd6efcb3
 *
 * followed in the same second by an interactive `authorization_code` consent —
 * for a UID absent from `~/.config/evolution/sources/`, whose only trace
 * anywhere on the machine was inside `~/.config/dconf/user`.
 *
 * This program runs the two halves of that story separately. It judges
 * neither: every assertion belongs to `rust/crates/jmap-functional/tests/
 * stale-source-uid.rs`, which runs this program and reads the `key=value`
 * observations it writes to stdout.
 *
 * MODE `dconf-only` — plant the debris and see whether it is inert.
 * Writes a UID nothing on disk names into all six keys of the
 * `org.gnome.Evolution.DefaultSources` schema (`e-source-registry.c:68`, the
 * only dconf-linked source-UID mechanism EDS ships), syncs it out to
 * `$XDG_CONFIG_HOME/dconf/user` — the file is checked, so a run that silently
 * fell back to GSettings' memory backend reports that rather than passing —
 * and only THEN starts the registry, so the setting is already there when the
 * daemon comes up. It then reports whether the registry has any such source
 * and what the default-source getters answer.
 *
 * MODE `collection-child` — reproduce the log line for real.
 * `collection_backend_new_user_file` (`e-collection-backend.c:176-200`) writes
 * a collection backend's children to `$XDG_CACHE_HOME/evolution/sources/
 * <collection-uid>/`, NOT to the config directory: "the UID matches no
 * configured source" is the normal condition of every child source Evolution
 * ever fans an account out into. This mode waits for our own collection
 * backend's children, then drives one of them down the operator's exact path:
 * `e_source_invoke_credentials_required_sync (reason=required)`, which is what
 * `e_backend_schedule_credentials_required()` calls for a backend that needs
 * credentials.
 *
 * WHY THE SIGNAL COUNTS ARE THE MEASUREMENT.
 * `server_side_source_invoke_credentials_required_cb` (`e-server-side-source.c:
 * 252`) does NOT simply forward the request to clients. For reason `required`
 * it sets `skip_emit = TRUE` (line 318) and starts a silent
 * `e_source_credentials_provider_lookup` instead; only its callback decides
 * what the user sees. So exactly one of two things reaches this process:
 *
 *   - the lookup found a password -> `authenticate`, and NO
 *     `credentials-required` at all: the user is never bothered.
 *   - the lookup failed -> the operator's log line, and `credentials-required`
 *     re-emitted with reason `required` (line 490 onward) -> the prompter, and
 *     for an OAuth2-method source the consent window.
 *
 * That makes the pair discriminating rather than decorative, which is why this
 * mode takes an optional password: run it once without and once with, and the
 * two counts swap. A test that only ever ran the failing half could not tell
 * the escalation from an unconditional echo of its own request.
 *
 *   usage: functional-stale-source-uid-client dconf-only <dangling-uid>
 *          functional-stale-source-uid-client collection-child <account-uid>
 *                                             [<password-to-store-first>]
 */

#include <libedataserver/libedataserver.h>

/* The schema `ESourceRegistry` tracks the default sources in, and the six keys
 * it defines. Spelled out rather than enumerated from the schema so that a key
 * disappearing upstream fails this run by name. From `e-source-registry.c:68`
 * and `/usr/share/glib-2.0/schemas/org.gnome.Evolution.DefaultSources.gschema.xml`. */
#define DEFAULT_SOURCES_SCHEMA "org.gnome.Evolution.DefaultSources"

/* How long to wait for the collection backend's populate/fan-out to produce
 * children, matching `collection-client.c`'s own wait for the same step. */
#define WAIT_SECONDS 30

/* How long to keep iterating this thread's default main context after the
 * credentials request, so that whatever the registry decided has a chance to
 * arrive before the counts are reported. ESource applies D-Bus news from an
 * idle on the context that was thread-default when `ESourceRegistry` was
 * constructed, so the signals are delivered by iterating and only by
 * iterating. Always burned in full: the *absence* of a signal is half of what
 * this program measures. */
#define SETTLE_MILLISECONDS 2500

/* How long to wait for the `ca.desrt.dconf` service to rewrite the database
 * file after `g_settings_sync` has handed it the change. A limit at all is so
 * that a session whose GSettings never reached dconf reports that instead of
 * hanging; the wait itself is normally over in milliseconds. */
#define DCONF_WRITE_TIMEOUT_SECONDS 5

/* Whether `needle` appears anywhere in the `length` bytes at `haystack`.
 *
 * Hand-rolled rather than `g_strstr_len`, which looks like the right tool and
 * is not: its bounded path walks `while (p <= end && *p)` and so stops at the
 * first NUL byte. dconf's database is a binary GVDB blob whose header contains
 * NULs long before any key, so `g_strstr_len` reports "not found" for a UID
 * that is plainly in the file. */
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

/* One `key=value` observation on stdout, the format `jmap_functional::
 * observations` parses. */
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

/* Iterate this thread's default main context for `milliseconds`. Not a wait
 * for a condition: see SETTLE_MILLISECONDS. */
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

/* Every UID the registry currently has, comma-separated, so the Rust side can
 * compare the live set against the `.source` files on disk without this
 * program deciding what that comparison means. One line rather than one line
 * per source because `jmap_functional::observations` is a map: repeated keys
 * would collapse into the last one. */
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
	/* Every key in the schema, so that no default-source path is left
	 * untested — the operator's account had a mail account, but the same
	 * question applies to the address book and calendar defaults. */
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
	/* Push it through the backend before anything reads it: without this the
	 * write is still queued in this process and `~/.config/dconf/user` would
	 * not yet name the UID the check below is about. */
	g_settings_sync ();
	g_object_unref (settings);

	/* The operator's forensic finding, reproduced: the UID is now in
	 * `~/.config/dconf/user` and nowhere else. Reported rather than assumed
	 * so that a session whose GSettings fell back to the memory backend —
	 * dconf's GIO module missing, or `ca.desrt.dconf` not activatable on this
	 * private bus — fails the Rust side's precondition instead of quietly
	 * making the rest of this mode measure nothing. */
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

		/* `g_settings_sync` flushes this process's queue into the dconf
		 * backend, but the `ca.desrt.dconf` service rewrites the database
		 * file itself afterwards. Observed to be done by the time the
		 * first read happens; polled anyway so that a slower machine
		 * reports a real answer rather than a race. */
		g_main_context_iteration (NULL, FALSE);
		g_usleep (20 * G_TIME_SPAN_MILLISECOND);
	}
	observe_boolean ("dconf-file-present", g_file_test (dconf_path, G_FILE_TEST_EXISTS));
	g_free (dconf_path);

	/* Only now. The setting is already in place when the daemon starts, so
	 * this is the strongest form of the question: not "can a running registry
	 * be pushed into it" but "does a registry that comes up with this debris
	 * already present do anything with it at all". */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, dangling_uid);
	observe_boolean ("dangling-uid-in-registry", source != NULL);
	g_clear_object (&source);

	/* `e_source_registry_ref_default_mail_account` (`e-source-registry.c:3300`)
	 * is the one getter that reads a dconf UID and is documented to fall back:
	 * `e_source_registry_ref_source` on the UID, then the built-in account if
	 * that came back NULL. What it answers here is the whole dconf-side
	 * question — a getter that fabricated a source would be the mechanism item
	 * 26 asks about. */
	default_mail_account = e_source_registry_ref_default_mail_account (registry);
	observe ("default-mail-account-uid",
		 default_mail_account ? e_source_get_uid (default_mail_account) : "(none)");
	g_clear_object (&default_mail_account);

	uids = source_uids (registry);
	observe ("source-uids", uids);
	g_free (uids);

	/* Give the registry the same window the other mode gives it, so that
	 * "nothing happened" is a measurement over a comparable stretch of time
	 * rather than a snapshot taken before anything could have. */
	settle (SETTLE_MILLISECONDS);

	uids = source_uids (registry);
	observe ("source-uids-after-settling", uids);
	g_free (uids);

	observe_boolean ("done", TRUE);
	g_object_unref (registry);

	return 0;
}

/* ----------------------------------------------------------- collection-child */

/* Counted rather than acted on. In the running application `credentials-
 * required` is what puts the credentials prompter — and, for an OAuth2-method
 * source, the consent window — in front of the user; `authenticate` is the
 * silent path, where the registry found a stored secret and handed it to the
 * backend without anybody being asked. */
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

/* The children of `parent_uid` the registry currently has. Polled rather than
 * awaited on a signal, for the reason `collection-client.c` gives: the
 * children are written to disk by the backend and picked up by the registry's
 * own file monitor. */
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
		return fail ("registry", error);

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

	/* Any one of them; they are all children written to the cache directory,
	 * which is the property under test. The Rust side is told which so it can
	 * go and look for the keyfile itself rather than take this program's word
	 * for where it landed. */
	child = E_SOURCE (children->data);
	observe ("child-uid", e_source_get_uid (child));
	observe ("child-display-name", e_source_get_display_name (child));

	/* Connected before anything is asked for, so a signal raised by the store
	 * below would be counted too. */
	g_signal_connect (child, "credentials-required",
			  G_CALLBACK (credentials_required_cb), &credentials_required);
	g_signal_connect (child, "authenticate",
			  G_CALLBACK (authenticate_cb), &authenticate);

	/* WHICH source actually holds this child's credentials — and a finding of
	 * item 26's in its own right. `source_credential_provider_ref_impl_for_
	 * source` (`e-source-credentials-provider.c:216-237`) does not look the
	 * secret up under the source it was asked about: it first calls
	 * `e_source_credentials_provider_ref_credentials_source`, which walks up
	 * the `Parent=` chain to the nearest source carrying `[Collection]`
	 * (line 416-437), "where the credentials are usually stored on the
	 * collection source, thus shared between child sources".
	 *
	 * The failure message, however, is formatted from `data->source` — the
	 * CHILD (`e-server-side-source.c:461-463`). So `Failed to lookup password
	 * for source <uid>` names one source while the store was searched for
	 * another, which is the sharpest reason the operator's UID looked
	 * unfamiliar. Reported here rather than assumed so the Rust side can pin
	 * that the two UIDs really do differ. */
	provider = e_source_credentials_provider_new (registry);
	credentials_source = e_source_credentials_provider_ref_credentials_source (provider, child);
	observe ("credentials-source-uid",
		 e_source_get_uid (credentials_source ? credentials_source : child));

	/* The control half, stored where the lookup will actually look.
	 * `e_source_store_password_sync` writes to the secret store under the
	 * source's own UID (`e-source.c:4062-4083`), which is what
	 * `e_source_credentials_provider_impl_password_lookup_sync` reads back —
	 * so this is the one difference between the registry's silent lookup
	 * succeeding and failing, and nothing else changes between the two runs. */
	observe_boolean ("password-stored", password != NULL);
	if (password != NULL &&
	    !e_source_store_password_sync (credentials_source ? credentials_source : child,
					   password, TRUE, NULL, &error))
		return fail ("store-password", error);

	g_clear_object (&credentials_source);
	g_object_unref (provider);

	/* The operator's path, invoked the way `e_backend_schedule_credentials_
	 * required()` invokes it for a backend that needs credentials. This
	 * program stands in for that backend, the same way `oauth2-stale-proxy-
	 * client.c` stands in for a long-lived factory. */
	if (!e_source_invoke_credentials_required_sync (child,
						       E_SOURCE_CREDENTIALS_REASON_REQUIRED,
						       NULL, 0, NULL, NULL, &error))
		return fail ("invoke-credentials-required", error);

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
