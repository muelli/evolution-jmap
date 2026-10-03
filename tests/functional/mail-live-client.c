/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Item 82 batch 1: the mail leg's own client against a real server, the
 * fourth of the live-server clients after book-client.c's `write` phase,
 * cal-live-client.c and collection-client.c.
 *
 * `mail-client.c` beside this file is the mock-based suite's client, and it
 * deliberately stays untouched. One run of it subscribes and unsubscribes two
 * named mailboxes, flags a message, creates, renames and deletes a folder,
 * transfers a message out of the inbox and back, and expunges another -- and
 * it asserts nothing, so every one of those is a call that aborts the whole
 * program the moment a real server answers it differently. It also assumes
 * the mock's own seeding throughout: two messages already in the inbox, and
 * mailboxes named exactly "Sent", "Trash" and "Junk", none of which a freshly
 * seeded Stalwart account has. This file is the receive half only -- open the
 * store, list the tree, resolve the three purpose folders by role, put one
 * message in and read it back -- which is the same bet cal-live-client.c
 * made: the simplest leg first, the richer ones as later batches.
 *
 * Everything around it -- the scratch XDG tree, the `.source` keyfile, the
 * private D-Bus session, the proxy to the real server and every assertion --
 * belongs to `rust/crates/jmap-functional/tests/live-stalwart-mail.rs`, which
 * runs this program and reads its output. So this file has no test framework
 * in it and no notion of what "correct" is: it reports what Camel told it on
 * stdout, one `key=value` line per observation, and exits non-zero the moment
 * a call fails.
 *
 * ## The password
 *
 * This is the one piece of the live-server mechanism that does not carry over
 * from the other three legs. They store the password on the `ESource`, with
 * `e_source_store_password_sync`, because an EDS backend asks EDS for
 * credentials. A Camel provider never consults the secret store: it takes
 * whatever its `CamelSession` put on the `CamelService`
 * (`camel_service_get_password`, read in jmap-mail's `authenticate_sync`),
 * and a session is the only object allowed to put one there. So this program
 * does both halves of what Evolution's own `EMailSession` does:
 *
 *   - it seeds the password on the service before connecting, which is the
 *     state `EMailSession` reaches by looking the stored credential up, and
 *   - it subclasses `CamelSession` to answer `get_password`, which is the
 *     vfunc the base class's `authenticate_sync` prompt loop calls when an
 *     attempt comes back REJECTED.
 *
 * Both, rather than either, because which of the two actually carries the
 * password is itself worth reporting: `password-prompts` is 0 when the seeded
 * password was accepted on the first attempt, and non-zero when the server
 * turned it down once and the loop had to re-offer it. Against the mock,
 * which checks no password at all, neither path runs.
 *
 *   usage: functional-mail-live-client <source-uid> <subject>
 */

#include <camel/camel.h>
#include <libedataserver/libedataserver.h>

/* The body of the message this program puts into the inbox. One line, no MIME
 * structure: what is being measured is whether a blob downloaded off a real
 * server decodes back to what went up. */
#define TEST_BODY "Found on the floor."

/* The password the harness seeded, and how many times the base class's
 * authenticate loop had to ask for it. Statics because there is one session
 * in this process and one question to answer about it -- the same shape
 * mail-stale-token-client.c's own counters use. */
static const gchar *seed_password = NULL;
static guint password_prompts = 0;

typedef struct _TestSession {
	CamelSession parent;
} TestSession;

typedef struct _TestSessionClass {
	CamelSessionClass parent_class;
} TestSessionClass;

GType test_session_get_type (void) G_GNUC_CONST;

G_DEFINE_TYPE (TestSession, test_session, CAMEL_TYPE_SESSION)

/* `EMailSession`'s vfunc, with the credential lookup replaced by the value
 * the harness put in the environment. The base `CamelSession` answers this
 * one by returning NULL, which ends its own authenticate loop with "no
 * password" rather than with a password -- see this file's header. */
static gchar *
test_session_get_password (CamelSession *session,
			   CamelService *service,
			   const gchar *prompt,
			   const gchar *item,
			   guint32 flags,
			   GError **error)
{
	password_prompts++;

	if (!seed_password || !*seed_password) {
		g_set_error_literal (error, CAMEL_SERVICE_ERROR,
				     CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
				     "no password was seeded for this test");
		return NULL;
	}

	return g_strdup (seed_password);
}

static void
test_session_class_init (TestSessionClass *klass)
{
	CAMEL_SESSION_CLASS (klass)->get_password = test_session_get_password;
}

static void
test_session_init (TestSession *session)
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

/* 3.60 replaced the borrowed `camel_folder_get_uids`/`camel_folder_free_uids`
 * pair with `camel_folder_dup_uids`, an ordinary reference-counted
 * `GPtrArray` the caller owns. Repeated here as plain C for the reason
 * mail-client.c gives beside its own copy: this test client links no crate
 * from this repository, so it cannot use `eds-sys`'s compat shims. */
static GPtrArray *
folder_dup_uids (CamelFolder *folder)
{
#if EDS_CHECK_VERSION(3, 60, 0)
	return camel_folder_dup_uids (folder);
#else
	return camel_folder_get_uids (folder);
#endif
}

static void
folder_free_uids (CamelFolder *folder,
		  GPtrArray *uids)
{
#if EDS_CHECK_VERSION(3, 60, 0)
	g_ptr_array_unref (uids);
#else
	camel_folder_free_uids (folder, uids);
#endif
}

/* The folder tree, flattened into a list of full names. Mirrors
 * mail-client.c's own helper of the same name. */
static void
collect_folder_names (CamelFolderInfo *info,
		      GPtrArray *names)
{
	for (; info; info = info->next) {
		g_ptr_array_add (names, g_strdup (info->full_name));
		collect_folder_names (info->child, names);
	}
}

/* One `key=a,b,c` line, sorted -- the order Camel hands folders over is the
 * provider's business, and a program that reported it as given would make the
 * harness assert an order nobody promised. Mirrors mail-client.c's own helper
 * of the same name, including leaving `values` as the caller handed it over. */
static void
report_sorted (const gchar *key,
	       GPtrArray *values)
{
	gchar *joined;

	g_ptr_array_sort_values (values, (GCompareFunc) g_strcmp0);
	/* The NULL terminator g_strjoinv wants has to go on after the sort so
	 * it is not sorted into the middle. */
	g_ptr_array_add (values, NULL);
	joined = g_strjoinv (",", (gchar **) values->pdata);
	g_print ("%s=%s\n", key, joined);
	g_free (joined);
	g_ptr_array_remove_index (values, values->len - 1);
}

/* A message's decoded body text, stripped of the trailing newline the MIME
 * transfer carries and the text does not. Mirrors the block mail-client.c
 * runs inline for every message in the inbox. */
static gchar *
message_body (CamelMimeMessage *message,
	      GError **error)
{
	GByteArray *bytes;
	CamelStream *stream;
	gchar *text;

	bytes = g_byte_array_new ();
	stream = camel_stream_mem_new_with_byte_array (bytes);

	if (camel_data_wrapper_decode_to_stream_sync (
			camel_medium_get_content (CAMEL_MEDIUM (message)),
			stream, NULL, error) < 0) {
		/* The stream owns `bytes`. */
		g_object_unref (stream);
		return NULL;
	}

	/* The decoded bytes are not NUL-terminated. */
	text = g_strndup ((const gchar *) bytes->data, bytes->len);
	g_object_unref (stream);

	return g_strstrip (text);
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
	CamelStore *store;
	CamelFolder *inbox;
	CamelFolder *trash;
	CamelFolder *junk;
	CamelFolderInfo *info;
	CamelMimeMessage *outside;
	CamelMimeMessage *reread;
	CamelMessageInfo *message_info;
	GPtrArray *names;
	GPtrArray *uids;
	gchar *raw;
	gchar *body;
	gchar *appended_uid = NULL;
	guint count_before;
	guint index;
	gboolean listed = FALSE;
	const gchar *source_uid;
	const gchar *subject;
	const gchar *protocol;
	const gchar *data_dir;
	const gchar *cache_dir;

	if (argc != 3) {
		g_printerr ("usage: %s <source-uid> <subject>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	subject = argv[2];
	seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

	/* The scratch tree the harness built. Camel keeps a summary database
	 * and a message cache per service under these, and a session that fell
	 * back to the real XDG directories would write into the developer's
	 * own Evolution store. */
	data_dir = g_get_user_data_dir ();
	cache_dir = g_get_user_cache_dir ();

	camel_init (data_dir, FALSE);
	camel_provider_init ();

	/* Generates the ESourceCamel subtype each provider's settings live
	 * under, so that the keyfile's groups parse into a CamelSettings
	 * object rather than being ignored. */
	e_source_camel_register_types ();

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* The protocol comes off the source rather than being spelled here --
	 * see mail-client.c's own note on why a program that hardcoded it
	 * would only agree with itself. */
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

	/* Copies the keyfile's settings onto the service -- the host, the port,
	 * the user name and the security method the provider reads in
	 * connect_sync. */
	e_source_camel_configure_service (source, service);

	/* The state EMailSession reaches by looking the stored credential up,
	 * reached here by being told it. See this file's header. */
	if (seed_password && *seed_password)
		camel_service_set_password (service, seed_password);

	if (!camel_service_connect_sync (service, NULL, &error))
		return fail ("connect", error);

	g_print ("store-connected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	/* 0 when the seeded password was taken on the first attempt. Not a
	 * pass/fail here -- the harness decides what it means. */
	g_print ("password-prompts=%u\n", password_prompts);

	store = CAMEL_STORE (service);

	info = camel_store_get_folder_info_sync (store, NULL,
						 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
						 NULL, &error);
	if (!info)
		return fail ("folder-info", error);

	names = g_ptr_array_new_with_free_func (g_free);
	collect_folder_names (info, names);
	camel_folder_info_free (info);

	report_sorted ("folders", names);
	g_ptr_array_unref (names);

	/* The three purpose folders by their JMAP role rather than by name.
	 * This is the assertion the whole leg exists for: a real Stalwart
	 * names the trash and junk roles "Deleted Items" and "Junk Mail",
	 * where the mock names them "Trash" and "Junk", so a provider that
	 * went looking for a folder by name passes against the mock and fails
	 * here. */
	inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
	if (!inbox)
		return fail ("inbox", error);
	g_print ("inbox-full-name=%s\n", camel_folder_get_full_name (inbox));

	trash = camel_store_get_trash_folder_sync (store, NULL, &error);
	if (!trash)
		return fail ("trash", error);
	g_print ("trash-full-name=%s\n", camel_folder_get_full_name (trash));
	g_object_unref (trash);

	junk = camel_store_get_junk_folder_sync (store, NULL, &error);
	if (!junk)
		return fail ("junk", error);
	g_print ("junk-full-name=%s\n", camel_folder_get_full_name (junk));
	g_object_unref (junk);

	if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
		return fail ("refresh", error);

	/* Counted rather than asserted to be zero: a throwaway account
	 * outlives the run that seeded it, so what this leg can claim is that
	 * the listing grew by exactly one, not what it started at. */
	uids = folder_dup_uids (inbox);
	count_before = uids->len;
	folder_free_uids (inbox, uids);
	g_print ("inbox-count-before=%u\n", count_before);

	/* `append_message_sync`: a message Camel is already holding, arriving
	 * in this account's inbox from outside it -- `Email/import` over an
	 * uploaded blob. Parsed from raw RFC 5322 bytes rather than built
	 * header by header, the same way mail-client.c constructs one: the
	 * parse on the way in has to be Camel's, or the write on the way out
	 * could disagree with it.
	 *
	 * The subject is the harness's, and is unique per run, so a repeated
	 * run against the same throwaway account cannot read back a previous
	 * run's message and call it this one's. */
	raw = g_strdup_printf (
		"From: Dave <dave@example.com>\r\n"
		"To: Alice <alice@example.com>\r\n"
		"Subject: %s\r\n"
		"Message-ID: <%s@example.com>\r\n"
		"Date: Thu, 15 Jan 2026 11:00:00 +0000\r\n"
		"\r\n"
		"%s\r\n",
		subject, subject, TEST_BODY);

	outside = camel_mime_message_new ();
	if (!camel_data_wrapper_construct_from_data_sync (
			CAMEL_DATA_WRAPPER (outside), raw, strlen (raw),
			NULL, &error)) {
		g_free (raw);
		g_object_unref (outside);
		return fail ("parse-outside-message", error);
	}
	g_free (raw);

	if (!camel_folder_append_message_sync (inbox, outside, NULL,
						&appended_uid, NULL, &error)) {
		g_object_unref (outside);
		return fail ("append-message", error);
	}
	g_object_unref (outside);

	g_print ("append-uid=%s\n", appended_uid ? appended_uid : "");

	if (!appended_uid) {
		g_printerr ("append-message: the append minted no uid\n");
		return 1;
	}

	/* The row is the listing's to write, not the append's -- the message
	 * appears only once the folder is next refreshed. */
	if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
		g_free (appended_uid);
		return fail ("refresh-after-append", error);
	}

	uids = folder_dup_uids (inbox);
	g_print ("inbox-count-after=%u\n", uids->len);

	for (index = 0; index < uids->len && !listed; index++)
		listed = g_strcmp0 (uids->pdata[index], appended_uid) == 0;

	g_print ("appended-in-listing=%d\n", listed ? 1 : 0);
	folder_free_uids (inbox, uids);

	/* Read back twice over, because those are two different requests: the
	 * summary comes from Email/query plus Email/get, and the body from a
	 * blob download, which is a plain HTTP GET rather than a method call.
	 * A provider that lists mail it cannot open is a common enough failure
	 * to be worth separating. */
	message_info = camel_folder_get_message_info (inbox, appended_uid);
	if (!message_info) {
		g_free (appended_uid);
		g_printerr ("summary: no message info for the appended uid\n");
		return 1;
	}

	g_print ("appended-subject=%s\n", camel_message_info_get_subject (message_info));
	g_clear_object (&message_info);

	reread = camel_folder_get_message_sync (inbox, appended_uid, NULL, &error);
	if (!reread) {
		g_free (appended_uid);
		return fail ("get-message", error);
	}

	body = message_body (reread, &error);
	if (!body) {
		g_object_unref (reread);
		g_free (appended_uid);
		return fail ("message-body", error);
	}

	g_print ("appended-body=%s\n", body);
	g_free (body);
	g_object_unref (reread);

	g_free (appended_uid);
	g_object_unref (inbox);
	g_object_unref (service);
	g_object_unref (session);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
