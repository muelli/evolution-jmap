/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * docs/ROADMAP.md item 25: item 23's acceptance test, made headless.
 *
 * Item 23 is the hourly re-consent — a pooled connection keeps an access
 * token past its expiry, gets a 401, and escalates that to a consent window
 * instead of refreshing. The fix is wired into all four call sites; what it
 * did not have is a test that drives it through a *real* `CamelService` in
 * the process Camel dlopens the provider into. Item 23's own parting note
 * says such a test cannot be built headlessly, "because the refresh path
 * needs a real CamelService registered on a real CamelSession
 * (EMailSession)". This program is the disproof.
 *
 * `functional-mail-client` already builds the real `CamelService` half: it is
 * an ordinary libcamel consumer, and the store it opens is the one Camel
 * found by reading `libcameljmap.urls`. The only thing a plain `CamelSession`
 * does not do is answer `get_oauth2_access_token_sync`, which is a class
 * method — so this program subclasses `CamelSession` and answers it, exactly
 * as `EMailSession` does (evolution-data-server 3.52.3,
 * `libemail-engine/e-mail-session.c`, `mail_session_get_oauth2_access_token_
 * sync`). Where that one looks the service's `ESource` up in the registry and
 * forwards to `e_source_get_oauth2_access_token_sync`, this one reads a file
 * the harness writes. That substitution is the whole point rather than a
 * shortcut: what item 23 is about is whether *this provider* asks its session
 * for a fresh token and retries, not whether EDS's own OAuth 2.0 machinery can
 * mint one — which `functional-oauth2-stale-proxy` and
 * `functional-secret-store-lock` already cover from the registry side.
 *
 * ## The handshake
 *
 * A token goes stale while a connection is open, which is a thing that has to
 * happen *between* two of this program's own calls. The Rust harness owns the
 * mock and therefore owns the rotation, so the two sides meet at two files:
 * this program writes `ready` once it has a connected store with a folder
 * listing already fetched, then waits for `go`, which the harness creates
 * after it has told the mock to accept a different bearer token. See
 * `rust/crates/jmap-functional/tests/mail-stale-token.rs`.
 *
 * ## What is reported
 *
 * Both counters, before and after — the point of the exercise is not only
 * that the second listing succeeded but that it took exactly one extra token
 * fetch to do it, and no re-authentication. A provider that reconnected from
 * scratch would also answer the listing, and would be the bug this is about.
 *
 *   usage: functional-mail-stale-token-client <source-uid> <token-file> <ready-file> <go-file>
 */

#include <camel/camel.h>
#include <libedataserver/libedataserver.h>

/* How long to wait for the harness's `go` file. Generous: the harness has to
 * notice `ready`, and it polls. A limit at all is so that a harness that died
 * fails this program instead of hanging the ctest run. */
#define HANDSHAKE_TIMEOUT_SECONDS 60
#define HANDSHAKE_POLL_MICROSECONDS (50 * 1000)

/* The file this session's `get_oauth2_access_token_sync` answers out of, and
 * the two counters it and `authenticate_sync` keep. Statics because there is
 * one session in this process and one of each question to answer about it;
 * the alternative is instance fields whose only reader is `main`. */
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

/* `EMailSession`'s vfunc, with the registry lookup replaced by a file read —
 * see this file's header for why that substitution is the point. The token is
 * re-read on every call rather than cached, which is what makes a refresh
 * observable at all: the harness rewrites the file between the two calls in
 * the positive case, and deliberately does not in the negative one. */
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

	/* The harness writes the token with no trailing newline, but a file
	 * that grew one would otherwise send a bearer token the mock has
	 * never heard of and fail this test for the wrong reason. */
	g_strstrip (contents);

	if (out_access_token)
		*out_access_token = contents;
	else
		g_free (contents);

	if (out_expires_in)
		*out_expires_in = 3600;

	return TRUE;
}

/* Counted, then handed to the base class unchanged: this is the escalation
 * item 23 is about. A 401 that the provider answered by re-authenticating
 * from scratch — rather than by refreshing the token on the connection it
 * already has — is what puts a consent window in front of the user, and it
 * would show up here as a second call. */
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

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

/* The folder tree, flattened into a sorted, comma-joined line — the same
 * shape `functional-mail-client` reports, and for the same reason: the order
 * a store hands its folders over in is the provider's business. */
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

/* Tell the harness the connection is up and settled, then wait for it to say
 * the server has moved on. Returns FALSE on timeout, which is a dead harness
 * rather than anything this test is about. */
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
		return fail ("registry", error);

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
		return fail ("add-service", error);

	e_source_camel_configure_service (source, service);

	/* Reported rather than assumed. `jmap-mail` decides an account is an
	 * OAuth 2.0 one by reading `CamelNetworkSettings:auth-mechanism`
	 * (`jmap_mail::oauth2::uses_oauth2`), which EDS binds to the keyfile's
	 * `[Authentication] Method`. If that binding were not what this test
	 * believes it is, every later observation here would be about a
	 * password account that happens to send no credentials, and the
	 * failure would name the connect rather than the cause. */
	settings = camel_service_ref_settings (service);
	g_print ("auth-mechanism=%s\n",
		 CAMEL_IS_NETWORK_SETTINGS (settings)
		 ? (camel_network_settings_get_auth_mechanism (CAMEL_NETWORK_SETTINGS (settings)) ?: "")
		 : "");
	g_clear_object (&settings);

	if (!camel_service_connect_sync (service, NULL, &error))
		return fail ("connect", error);

	g_print ("store-connected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	store = CAMEL_STORE (service);

	/* The first listing, over the token the connection was built with.
	 * `REFRESH` on both calls, because `JmapStore::folders` answers out of
	 * the listing it already holds when the flag is absent — a second call
	 * without it would never reach the network and so could never see a
	 * 401. */
	info = camel_store_get_folder_info_sync (store, NULL, listing_flags, NULL, &error);
	if (!info)
		return fail ("folder-info", error);
	report_folder_names ("folders", info);
	camel_folder_info_free (info);

	g_print ("token-fetches-before-rotation=%u\n", token_fetches);
	g_print ("authenticate-calls-before-rotation=%u\n", authenticate_calls);

	if (!handshake (ready_file, go_file, &error))
		return fail ("handshake", error);

	/* The whole exercise. The connection is the one that already worked;
	 * the server now refuses the token it carries. Whether this answers is
	 * reported rather than checked, because it is the observation in both
	 * directions: the harness runs this program once where the refresh can
	 * succeed and once where it cannot. */
	info = camel_store_get_folder_info_sync (store, NULL, listing_flags, NULL, &error);
	g_print ("second-listing-ok=%d\n", info ? 1 : 0);
	if (info) {
		report_folder_names ("folders-after-rotation", info);
		camel_folder_info_free (info);
	} else {
		/* One line, so a multi-line message cannot be read as several
		 * observations. */
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

	/* Zero even when the second listing failed: which of the two runs this
	 * is, and therefore whether that is the expected answer, is the
	 * harness's to know. A non-zero exit here means a step that is not
	 * what the test is about went wrong. */
	return 0;
}
