/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * A headless acceptance test for the hourly re-consent bug.
 *
 * In that bug, a pooled connection keeps an access token after it expires,
 * gets a 401, and opens a consent window instead of a token refresh. This
 * program tests the fix through a *real* `CamelService`, in the process that
 * Camel dlopens the provider into. An earlier analysis said that such a test
 * could not be headless, "because the refresh path needs a real CamelService
 * registered on a real CamelSession (EMailSession)". This program shows that
 * the analysis was wrong.
 *
 * As in `functional-mail-client`, the store is the one that Camel finds
 * through `libcameljmap.urls`. A plain `CamelSession` does not answer the
 * `get_oauth2_access_token_sync` class method. Thus this program subclasses
 * `CamelSession` and answers it, as `EMailSession` does (evolution-data-server
 * 3.52.3, `libemail-engine/e-mail-session.c`,
 * `mail_session_get_oauth2_access_token_sync`). `EMailSession` finds the
 * `ESource` of the service in the registry and calls
 * `e_source_get_oauth2_access_token_sync`. This program reads a file from the
 * harness instead. This change is intentional: the test checks that *this
 * provider* asks its session for a new token and tries again. It does not
 * check the OAuth 2.0 code of EDS. `functional-oauth2-stale-proxy` and
 * `functional-secret-store-lock` test that from the registry side.
 *
 * ## The handshake
 *
 * The token must become stale between two calls of this program, while the
 * connection is open. The Rust harness controls the mock and thus the token
 * rotation, so the two sides use two files. This program writes `ready` after
 * it has a connected store and a first folder listing. Then it waits for `go`.
 * The harness creates `go` after it tells the mock to accept a different
 * bearer token. See `rust/crates/jmap-functional/tests/mail-stale-token.rs`.
 *
 * ## The output
 *
 * Both counters, before and after the rotation. The second listing must
 * succeed with exactly one more token fetch and no new authentication. A
 * provider that reconnected from the start would also give the listing, and
 * that is the bug.
 *
 *   usage: functional-mail-stale-token-client <source-uid> <token-file> <ready-file> <go-file>
 */

#include <camel/camel.h>
#include <libedataserver/libedataserver.h>

#include "test-util.h"

/* The time limit for the `go` file of the harness. It is long, because the
 * harness polls for `ready`. The limit makes this program fail, not hang the
 * ctest run, when the harness stops. */
#define HANDSHAKE_TIMEOUT_SECONDS 60
#define HANDSHAKE_POLL_MICROSECONDS (50 * 1000)

/* The token file for `get_oauth2_access_token_sync`, and the counters of that
 * vfunc and `authenticate_sync`. They are static because the process has one
 * session, and only `main` reads them. */
static const gchar *token_file = NULL;
static guint token_fetches = 0;
static guint authenticate_calls = 0;

typedef struct _TestSession {
	CamelSession parent;
} TestSession;

typedef struct _TestSessionClass {
	CamelSessionClass parent_class;
} TestSessionClass;

GType test_session_get_type (void) G_GNUC_CONST;

G_DEFINE_TYPE (TestSession, test_session, CAMEL_TYPE_SESSION)

/* The `EMailSession` vfunc, with a file read instead of the registry lookup
 * (the file header gives the reason). Each call reads the file again, without
 * a cache, so a refresh is visible. The harness changes the file between the
 * two calls in the positive case, and not in the negative case. */
static gboolean
test_session_get_oauth2_access_token_sync (CamelSession *session,
					   CamelService *service,
					   gchar **out_access_token,
					   gint *out_expires_in,
					   GCancellable *cancellable,
					   GError **error)
{
	gchar *contents = NULL;

	token_fetches++;

	if (!g_file_get_contents (token_file, &contents, NULL, error))
		return FALSE;

	/* The harness writes no trailing newline. Remove whitespace anyway: a
	 * token with a newline is unknown to the mock and fails the test for
	 * the wrong reason. */
	g_strstrip (contents);

	if (out_access_token)
		*out_access_token = contents;
	else
		g_free (contents);

	if (out_expires_in)
		*out_expires_in = 3600;

	return TRUE;
}

/* Count the call, then give it to the base class. This test looks for this
 * escalation. If the provider answers a 401 with a new authentication instead
 * of a token refresh on its connection, the user sees a consent window. This
 * shows as a second call here. */
static gboolean
test_session_authenticate_sync (CamelSession *session,
				CamelService *service,
				const gchar *mechanism,
				GCancellable *cancellable,
				GError **error)
{
	authenticate_calls++;

	return CAMEL_SESSION_CLASS (test_session_parent_class)->authenticate_sync (
		session, service, mechanism, cancellable, error);
}

static void
test_session_class_init (TestSessionClass *klass)
{
	CamelSessionClass *session_class = CAMEL_SESSION_CLASS (klass);

	session_class->get_oauth2_access_token_sync = test_session_get_oauth2_access_token_sync;
	session_class->authenticate_sync = test_session_authenticate_sync;
}

static void
test_session_init (TestSession *self)
{
}

/* Writes the folder tree as one sorted, comma-separated line, as
 * `functional-mail-client` does. The provider controls the folder order. */
static void
report_folder_names (const gchar *key,
		     CamelFolderInfo *info)
{
	GPtrArray *names = g_ptr_array_new_with_free_func (g_free);
	GQueue queue = G_QUEUE_INIT;
	gchar *joined;

	for (; info; info = info->next)
		g_queue_push_tail (&queue, info);

	while (!g_queue_is_empty (&queue)) {
		CamelFolderInfo *node = g_queue_pop_head (&queue);
		CamelFolderInfo *child;

		g_ptr_array_add (names, g_strdup (node->full_name));
		for (child = node->child; child; child = child->next)
			g_queue_push_tail (&queue, child);
	}

	g_ptr_array_sort_values (names, (GCompareFunc) g_strcmp0);
	g_ptr_array_add (names, NULL);
	joined = g_strjoinv (",", (gchar **) names->pdata);
	g_print ("%s=%s\n", key, joined);
	g_free (joined);
	g_ptr_array_unref (names);
}

/* Tell the harness that the connection is ready, then wait for the server
 * rotation. Returns FALSE on timeout, which means the harness stopped. */
static gboolean
handshake (const gchar *ready_file,
	   const gchar *go_file,
	   GError **error)
{
	gint waited;

	if (!g_file_set_contents (ready_file, "ready", -1, error))
		return FALSE;

	for (waited = 0;
	     waited < HANDSHAKE_TIMEOUT_SECONDS * (1000 * 1000 / HANDSHAKE_POLL_MICROSECONDS);
	     waited++) {
		if (g_file_test (go_file, G_FILE_TEST_EXISTS))
			return TRUE;
		g_usleep (HANDSHAKE_POLL_MICROSECONDS);
	}

	g_set_error (error, G_IO_ERROR, G_IO_ERROR_TIMED_OUT,
		     "the harness never created '%s'", go_file);

	return FALSE;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	ESourceBackend *backend_extension;
	CamelSession *session;
	CamelService *service;
	CamelSettings *settings;
	CamelStore *store;
	CamelFolderInfo *info;
	const gchar *source_uid;
	const gchar *ready_file;
	const gchar *go_file;
	const gchar *protocol;
	const gchar *data_dir;
	const gchar *cache_dir;
	guint32 listing_flags = CAMEL_STORE_FOLDER_INFO_RECURSIVE |
				CAMEL_STORE_FOLDER_INFO_REFRESH;

	if (argc != 5) {
		g_printerr ("usage: %s <source-uid> <token-file> <ready-file> <go-file>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	token_file = argv[2];
	ready_file = argv[3];
	go_file = argv[4];

	data_dir = g_get_user_data_dir ();
	cache_dir = g_get_user_cache_dir ();

	camel_init (data_dir, FALSE);
	camel_provider_init ();
	e_source_camel_register_types ();

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	if (!e_source_has_extension (source, E_SOURCE_EXTENSION_MAIL_ACCOUNT)) {
		g_printerr ("source '%s' is not a mail account\n", source_uid);
		return 1;
	}

	backend_extension = e_source_get_extension (source, E_SOURCE_EXTENSION_MAIL_ACCOUNT);
	protocol = e_source_backend_get_backend_name (backend_extension);
	g_print ("protocol=%s\n", protocol ? protocol : "");

	session = g_object_new (test_session_get_type (),
				"user-data-dir", data_dir,
				"user-cache-dir", cache_dir,
				NULL);

	service = camel_session_add_service (session, source_uid, protocol,
					     CAMEL_PROVIDER_STORE, &error);
	if (!service)
		return functional_fail ("add-service", error);

	e_source_camel_configure_service (source, service);

	/* Report the mechanism, do not assume it. `jmap-mail` identifies an
	 * OAuth 2.0 account from `CamelNetworkSettings:auth-mechanism`
	 * (`jmap_mail::oauth2::uses_oauth2`). EDS binds this to the keyfile
	 * `[Authentication] Method`. If that binding is different, all later
	 * results are about a password account with no credentials, and the
	 * failure shows at the connect, not at the cause. */
	settings = camel_service_ref_settings (service);
	g_print ("auth-mechanism=%s\n",
		 CAMEL_IS_NETWORK_SETTINGS (settings)
		 ? (camel_network_settings_get_auth_mechanism (CAMEL_NETWORK_SETTINGS (settings)) ?: "")
		 : "");
	g_clear_object (&settings);

	if (!camel_service_connect_sync (service, NULL, &error))
		return functional_fail ("connect", error);

	g_print ("store-connected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	store = CAMEL_STORE (service);

	/* The first listing, with the original token. Both calls use
	 * `REFRESH`, because without it `JmapStore::folders` answers from the
	 * listing that it has. Then the second call does not use the network
	 * and cannot get a 401. */
	info = camel_store_get_folder_info_sync (store, NULL, listing_flags, NULL, &error);
	if (!info)
		return functional_fail ("folder-info", error);
	report_folder_names ("folders", info);
	camel_folder_info_free (info);

	g_print ("token-fetches-before-rotation=%u\n", token_fetches);
	g_print ("authenticate-calls-before-rotation=%u\n", authenticate_calls);

	if (!handshake (ready_file, go_file, &error))
		return functional_fail ("handshake", error);

	/* The main step. The connection worked before, but the server now
	 * refuses its token. Report the result, do not check it: the harness
	 * runs this program once where the refresh can succeed and once where
	 * it cannot. */
	info = camel_store_get_folder_info_sync (store, NULL, listing_flags, NULL, &error);
	g_print ("second-listing-ok=%d\n", info ? 1 : 0);
	if (info) {
		report_folder_names ("folders-after-rotation", info);
		camel_folder_info_free (info);
	} else {
		/* Use one line, so that a multi-line message does not look
		 * like many observations. */
		gchar *message = g_strdelimit (g_strdup (error ? error->message : "(no error set)"),
					       "\r\n", ' ');

		g_print ("second-listing-error=%s\n", message);
		g_free (message);
		g_clear_error (&error);
	}

	g_print ("token-fetches=%u\n", token_fetches);
	g_print ("authenticate-calls=%u\n", authenticate_calls);

	g_object_unref (session);
	g_object_unref (source);
	g_object_unref (registry);

	/* Return zero also when the second listing failed: only the harness
	 * knows which run this is. A non-zero exit means that a different step
	 * failed. */
	return 0;
}
