/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Item 82 batches 1 to 3: the mail leg's own client against a real server,
 * the fourth of the live-server clients after book-client.c's `write` phase,
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
 * seeded Stalwart account has. This file takes the same bet
 * cal-live-client.c made instead -- the simplest leg first, the richer ones
 * as later batches -- and the batch is chosen by argv:
 *
 *   - `receive` opens the store, lists the tree, resolves the three purpose
 *     folders by role, puts one message in and reads it back;
 *   - `flags` shares all of that except the reporting, and then writes to the
 *     appended message's flags, clears them again, and expunges it;
 *   - `transfer` creates a folder of its own, moves the appended message into
 *     it, copies it back to the inbox, and then puts everything away again:
 *     the folder emptied and deleted, the message expunged.
 *
 * One program rather than three because the setup the later phases need -- a
 * connected store with one known message in the inbox -- is exactly what
 * `receive` already does.
 *
 * ## Reading a write back
 *
 * A flag Camel wrote is in Camel's own summary database a moment later
 * whether or not the server ever heard about it, so the `flags` phase asks a
 * second `CamelStore` on a scratch tree of its own (`report_reopened`). It
 * has no summary to answer from, so what it reports came off the server.
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
 *   usage: functional-mail-live-client <source-uid> <subject> <phase>
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

/* The one account this program drives, and the two things opening a store on
 * it takes. Statics for the same reason the password above is one: there is
 * exactly one of each in this process, and the alternative is threading three
 * more arguments through every helper that opens a second store. */
static ESource *account_source = NULL;
static const gchar *account_uid = NULL;
static const gchar *account_protocol = NULL;

/* Which batch of observations one run of this program makes -- see the
 * header. Every phase shares the preamble: a connected store and one known
 * message appended to the inbox. */
typedef enum {
	PHASE_RECEIVE,
	PHASE_FLAGS,
	PHASE_TRANSFER
} MailPhase;

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

/* A connected store on this account, with its summary database and message
 * cache under `data_dir`/`cache_dir`.
 *
 * The session is handed back rather than dropped: `CamelService` holds only a
 * weak reference to the session that made it, so a session the caller let go
 * of would take the service's notion of itself with it.
 *
 * NULL with `error` set on any failure, and nothing left alive behind it. */
static CamelStore *
open_store (const gchar *data_dir,
	    const gchar *cache_dir,
	    CamelSession **out_session,
	    GError **error)
{
	CamelSession *session;
	CamelService *service;

	session = g_object_new (test_session_get_type (),
				"user-data-dir", data_dir,
				"user-cache-dir", cache_dir,
				NULL);

	service = camel_session_add_service (session, account_uid, account_protocol,
					     CAMEL_PROVIDER_STORE, error);
	if (!service) {
		g_object_unref (session);
		return NULL;
	}

	/* Copies the keyfile's settings onto the service -- the host, the port,
	 * the user name and the security method the provider reads in
	 * connect_sync. */
	e_source_camel_configure_service (account_source, service);

	/* The state EMailSession reaches by looking the stored credential up,
	 * reached here by being told it. See this file's header. */
	if (seed_password && *seed_password)
		camel_service_set_password (service, seed_password);

	if (!camel_service_connect_sync (service, NULL, error)) {
		g_object_unref (service);
		g_object_unref (session);
		return NULL;
	}

	*out_session = session;

	return CAMEL_STORE (service);
}

/* What a store that has never seen this account before makes of one message:
 * whether the inbox holds it at all, and its read and important marks.
 *
 * Reported as `<tag>-listed`, `<tag>-seen` and `<tag>-flagged`, the last two
 * only when the message is there at all. The scratch tree is `tag`'s own, so
 * no earlier call's summary database can answer for this one -- which is the
 * whole point: every one of these three numbers came off the real server. */
static gboolean
report_reopened (const gchar *tag,
		 const gchar *uid,
		 GError **error)
{
	CamelSession *session = NULL;
	CamelStore *store;
	CamelFolder *inbox;
	CamelMessageInfo *info;
	gchar *data_dir;
	gchar *cache_dir;
	gboolean refreshed;

	data_dir = g_build_filename (g_get_user_data_dir (), "reopened", tag, NULL);
	cache_dir = g_build_filename (g_get_user_cache_dir (), "reopened", tag, NULL);
	g_mkdir_with_parents (data_dir, 0700);
	g_mkdir_with_parents (cache_dir, 0700);

	store = open_store (data_dir, cache_dir, &session, error);
	g_free (data_dir);
	g_free (cache_dir);

	if (!store)
		return FALSE;

	inbox = camel_store_get_inbox_folder_sync (store, NULL, error);
	if (!inbox) {
		g_object_unref (store);
		g_object_unref (session);
		return FALSE;
	}

	refreshed = camel_folder_refresh_info_sync (inbox, NULL, error);
	if (refreshed) {
		info = camel_folder_get_message_info (inbox, uid);
		g_print ("%s-listed=%d\n", tag, info ? 1 : 0);

		if (info) {
			guint32 flags = camel_message_info_get_flags (info);

			g_print ("%s-seen=%d\n", tag,
				 (flags & CAMEL_MESSAGE_SEEN) ? 1 : 0);
			g_print ("%s-flagged=%d\n", tag,
				 (flags & CAMEL_MESSAGE_FLAGGED) ? 1 : 0);
			g_clear_object (&info);
		}
	}

	g_object_unref (inbox);
	g_object_unref (store);
	g_object_unref (session);

	return refreshed;
}

/* Sets `mask` to `set` on one message of `folder`'s, then pushes the change
 * with `synchronize_sync` -- which is the only call in this program that ever
 * writes to a message's flags, and the one a real server can answer
 * differently from the mock.
 *
 * `set` is a flags word and not a boolean because clearing a mark is a
 * different `Email/set` patch from setting one, not the same write with the
 * other value. */
static gboolean
write_flags (CamelFolder *folder,
	     const gchar *uid,
	     guint32 mask,
	     guint32 set,
	     GError **error)
{
	CamelMessageInfo *info;

	info = camel_folder_get_message_info (folder, uid);
	if (!info) {
		g_set_error_literal (error, CAMEL_ERROR, CAMEL_ERROR_GENERIC,
				     "the summary has no row for the message to flag");
		return FALSE;
	}

	camel_message_info_set_flags (info, mask, set);
	g_clear_object (&info);

	return camel_folder_synchronize_sync (folder, FALSE, NULL, error);
}

/* One message's read and important marks as the summary row holds them now,
 * reported as `seen-<tag>` and `flagged-<tag>`.
 *
 * Weaker than report_reopened and deliberately kept beside it: this is the
 * row the program just wrote to, so it separates a write that never reached
 * the server from one Camel never attempted in the first place. */
static gboolean
report_local_flags (CamelFolder *folder,
		    const gchar *uid,
		    const gchar *tag)
{
	CamelMessageInfo *info;
	guint32 flags;

	info = camel_folder_get_message_info (folder, uid);
	if (!info)
		return FALSE;

	flags = camel_message_info_get_flags (info);
	g_print ("seen-%s=%d\n", tag, (flags & CAMEL_MESSAGE_SEEN) ? 1 : 0);
	g_print ("flagged-%s=%d\n", tag, (flags & CAMEL_MESSAGE_FLAGGED) ? 1 : 0);
	g_clear_object (&info);

	return TRUE;
}

/* The folder's two counts, under `inbox-count-<tag>` and
 * `inbox-summary-<tag>`.
 *
 * Two of them rather than one because they are two different questions with
 * two different answers to get wrong: the uid listing is what a message list
 * is drawn from, and `camel_folder_summary_count` is what the folder tree's
 * own total column asks. A provider that dropped a row from one and not the
 * other leaves a folder claiming a message nobody can open. */
static void
report_counts (CamelFolder *folder,
	       const gchar *tag)
{
	GPtrArray *uids;

	uids = folder_dup_uids (folder);
	g_print ("inbox-count-%s=%u\n", tag, uids->len);
	folder_free_uids (folder, uids);

	g_print ("inbox-summary-%s=%u\n", tag,
		 camel_folder_summary_count (camel_folder_get_folder_summary (folder)));
}

/* One message, filed from `from` into `to` -- the move when
 * `delete_originals`, the copy otherwise. The uid the vfunc reports back goes
 * out under `key`: RFC 8621 gives an Email one immutable id per account and a
 * transfer only patches its `mailboxIds`, so it should be the uid that went
 * in, not one the destination minted. */
static gboolean
transfer_one (CamelFolder *from,
	      CamelFolder *to,
	      const gchar *uid,
	      gboolean delete_originals,
	      const gchar *key,
	      GError **error)
{
	GPtrArray *uids;
	GPtrArray *transferred = NULL;
	gboolean ok;

	uids = g_ptr_array_new ();
	g_ptr_array_add (uids, (gpointer) uid);

	ok = camel_folder_transfer_messages_to_sync (from, uids, to,
						     delete_originals,
						     &transferred, NULL, error);
	g_ptr_array_unref (uids);

	if (!ok)
		return FALSE;

	g_print ("%s=%s\n", key,
		 (transferred && transferred->len > 0 && transferred->pdata[0])
		 ? (const gchar *) transferred->pdata[0] : "");
	if (transferred) {
		guint t;

		for (t = 0; t < transferred->len; t++)
			g_free (transferred->pdata[t]);
		g_ptr_array_unref (transferred);
	}

	return TRUE;
}

/* `folder`'s uid listing after a refresh: its length under `count-<tag>`, and
 * whether it names `uid` under `holds-<tag>`. The refresh is what makes either
 * number a claim about the server rather than about the summary rows the last
 * transfer left behind. */
static gboolean
report_refreshed_listing (CamelFolder *folder,
			  const gchar *uid,
			  const gchar *tag,
			  GError **error)
{
	GPtrArray *uids;
	guint index;
	gboolean listed = FALSE;

	if (!camel_folder_refresh_info_sync (folder, NULL, error))
		return FALSE;

	uids = folder_dup_uids (folder);
	for (index = 0; index < uids->len && !listed; index++)
		listed = g_strcmp0 (uids->pdata[index], uid) == 0;

	g_print ("count-%s=%u\n", tag, uids->len);
	g_print ("holds-%s=%d\n", tag, listed ? 1 : 0);
	folder_free_uids (folder, uids);

	return TRUE;
}

/* What a store that has never seen this account before makes of the message's
 * filing: whether the inbox lists it, whether `folder_name` lists it, and the
 * subject `folder_name`'s own summary gives it. Reported as
 * `<tag>-inbox-listed`, `<tag>-folder-listed` and `<tag>-subject`, the last
 * only when the folder holds the row at all. Same scratch-tree-per-tag
 * discipline as report_reopened, and for the same reason: no earlier call's
 * summary database can answer for this one. */
static gboolean
report_reopened_filed (const gchar *tag,
		       const gchar *uid,
		       const gchar *folder_name,
		       GError **error)
{
	CamelSession *session = NULL;
	CamelStore *store;
	CamelFolder *inbox;
	CamelFolder *folder;
	CamelMessageInfo *info;
	gchar *data_dir;
	gchar *cache_dir;

	data_dir = g_build_filename (g_get_user_data_dir (), "reopened", tag, NULL);
	cache_dir = g_build_filename (g_get_user_cache_dir (), "reopened", tag, NULL);
	g_mkdir_with_parents (data_dir, 0700);
	g_mkdir_with_parents (cache_dir, 0700);

	store = open_store (data_dir, cache_dir, &session, error);
	g_free (data_dir);
	g_free (cache_dir);

	if (!store)
		return FALSE;

	inbox = camel_store_get_inbox_folder_sync (store, NULL, error);
	if (!inbox || !camel_folder_refresh_info_sync (inbox, NULL, error)) {
		g_clear_object (&inbox);
		g_object_unref (store);
		g_object_unref (session);
		return FALSE;
	}

	info = camel_folder_get_message_info (inbox, uid);
	g_print ("%s-inbox-listed=%d\n", tag, info ? 1 : 0);
	g_clear_object (&info);
	g_object_unref (inbox);

	folder = camel_store_get_folder_sync (store, folder_name,
					      CAMEL_STORE_FOLDER_NONE,
					      NULL, error);
	if (!folder || !camel_folder_refresh_info_sync (folder, NULL, error)) {
		g_clear_object (&folder);
		g_object_unref (store);
		g_object_unref (session);
		return FALSE;
	}

	info = camel_folder_get_message_info (folder, uid);
	g_print ("%s-folder-listed=%d\n", tag, info ? 1 : 0);
	if (info)
		g_print ("%s-subject=%s\n", tag, camel_message_info_get_subject (info));
	g_clear_object (&info);

	g_object_unref (folder);
	g_object_unref (store);
	g_object_unref (session);

	return TRUE;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESourceBackend *backend_extension;
	CamelSession *session = NULL;
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
	MailPhase phase_id;
	const gchar *subject;
	const gchar *phase;
	const gchar *data_dir;
	const gchar *cache_dir;

	if (argc != 4) {
		g_printerr ("usage: %s <source-uid> <subject> <phase>\n", argv[0]);
		return 2;
	}

	account_uid = argv[1];
	subject = argv[2];
	phase = argv[3];
	seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

	if (g_strcmp0 (phase, "receive") == 0) {
		phase_id = PHASE_RECEIVE;
	} else if (g_strcmp0 (phase, "flags") == 0) {
		phase_id = PHASE_FLAGS;
	} else if (g_strcmp0 (phase, "transfer") == 0) {
		phase_id = PHASE_TRANSFER;
	} else {
		g_printerr ("%s: unknown phase '%s'\n", argv[0], phase);
		return 2;
	}

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

	account_source = e_source_registry_ref_source (registry, account_uid);
	if (!account_source) {
		g_printerr ("registry: no source with UID '%s'\n", account_uid);
		return 1;
	}

	/* The protocol comes off the source rather than being spelled here --
	 * see mail-client.c's own note on why a program that hardcoded it
	 * would only agree with itself. */
	if (!e_source_has_extension (account_source, E_SOURCE_EXTENSION_MAIL_ACCOUNT)) {
		g_printerr ("source '%s' is not a mail account\n", account_uid);
		return 1;
	}

	backend_extension = e_source_get_extension (account_source, E_SOURCE_EXTENSION_MAIL_ACCOUNT);
	account_protocol = e_source_backend_get_backend_name (backend_extension);
	g_print ("protocol=%s\n", account_protocol ? account_protocol : "");

	store = open_store (data_dir, cache_dir, &session, &error);
	if (!store)
		return fail ("connect", error);

	g_print ("store-connected=%d\n",
		 camel_service_get_connection_status (CAMEL_SERVICE (store)) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	/* 0 when the seeded password was taken on the first attempt. Not a
	 * pass/fail here -- the harness decides what it means. */
	g_print ("password-prompts=%u\n", password_prompts);

	if (phase_id == PHASE_RECEIVE) {
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
	}

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

	if (phase_id == PHASE_RECEIVE) {
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
	}

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

	if (phase_id == PHASE_RECEIVE) {
		/* Read back twice over, because those are two different
		 * requests: the summary comes from Email/query plus Email/get,
		 * and the body from a blob download, which is a plain HTTP GET
		 * rather than a method call. A provider that lists mail it
		 * cannot open is a common enough failure to be worth
		 * separating. */
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
	} else if (phase_id == PHASE_FLAGS) {
		/* What the message arrived carrying. The two writes below are
		 * both measured as differences from here, so a server that
		 * handed keywords of its own to an imported message has to be
		 * visible rather than assumed away. */
		if (!report_local_flags (inbox, appended_uid, "after-append")) {
			g_free (appended_uid);
			g_printerr ("summary: no message info for the appended uid\n");
			return 1;
		}

		/* Read and important together, because they are one patch:
		 * `synchronize_sync` sends the whole difference between the
		 * keywords the last listing found and the ones the row claims
		 * now, not one request per bit. */
		if (!write_flags (inbox, appended_uid,
				  CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED,
				  CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED,
				  &error)) {
			g_free (appended_uid);
			return fail ("synchronize-set", error);
		}

		report_local_flags (inbox, appended_uid, "local-after-set");

		if (!report_reopened ("reopened-set", appended_uid, &error)) {
			g_free (appended_uid);
			return fail ("reopen-after-set", error);
		}

		/* The other direction, which is the half a provider that only
		 * ever added keywords would still pass the check above on. */
		if (!write_flags (inbox, appended_uid,
				  CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED, 0,
				  &error)) {
			g_free (appended_uid);
			return fail ("synchronize-clear", error);
		}

		if (!report_reopened ("reopened-cleared", appended_uid, &error)) {
			g_free (appended_uid);
			return fail ("reopen-after-clear", error);
		}

		/* `expunge_sync`: the deletion itself. CAMEL_MESSAGE_DELETED is
		 * a bit of Camel's own -- JMAP has no deleted keyword -- so
		 * marking the row reaches no server at all and the expunge is
		 * the request that does. */
		report_counts (inbox, "before-expunge");

		message_info = camel_folder_get_message_info (inbox, appended_uid);
		if (!message_info) {
			g_free (appended_uid);
			g_printerr ("summary: no message info to mark deleted\n");
			return 1;
		}
		camel_message_info_set_flags (message_info, CAMEL_MESSAGE_DELETED,
					      CAMEL_MESSAGE_DELETED);
		g_clear_object (&message_info);

		if (!camel_folder_expunge_sync (inbox, NULL, &error)) {
			g_free (appended_uid);
			return fail ("expunge", error);
		}

		/* No refresh in between: the rows go when the expunge does
		 * rather than at the next listing, so a provider that left
		 * them for a later refresh has to fail here rather than be
		 * covered for by one. */
		report_counts (inbox, "after-expunge");

		listed = FALSE;
		uids = folder_dup_uids (inbox);
		for (index = 0; index < uids->len && !listed; index++)
			listed = g_strcmp0 (uids->pdata[index], appended_uid) == 0;
		folder_free_uids (inbox, uids);
		g_print ("expunged-still-listed=%d\n", listed ? 1 : 0);

		/* And the same question of the server, which neither of the
		 * two counts above can answer: they would both look right for
		 * a provider that only forgot the message locally. */
		if (!report_reopened ("reopened-expunged", appended_uid, &error)) {
			g_free (appended_uid);
			return fail ("reopen-after-expunge", error);
		}
	} else {
		CamelFolderInfo *created;
		CamelFolder *scratch;
		CamelMessageInfo *filed_info;
		gchar *scratch_name;

		/* A folder of this run's own, named by the unique subject, so
		 * a rerun against the same throwaway account can never open a
		 * previous run's folder and call it this one's. A NULL parent
		 * is the account root, the same convention mail-client.c
		 * uses. */
		created = camel_store_create_folder_sync (store, NULL, subject,
							  NULL, &error);
		if (!created)
			return fail ("create-folder", error);

		g_print ("created-folder=%s\n", created->full_name);
		scratch_name = g_strdup (created->full_name);
		camel_folder_info_free (created);

		scratch = camel_store_get_folder_sync (store, scratch_name,
						       CAMEL_STORE_FOLDER_NONE,
						       NULL, &error);
		if (!scratch)
			return fail ("get-scratch-folder", error);

		/* Counted so the transfers below are measured against a
		 * known-empty folder; the inbox's own counts can only ever be
		 * differences, because the account outlives the run. */
		if (!report_refreshed_listing (scratch, appended_uid,
					       "scratch-before", &error)) {
			g_object_unref (scratch);
			return fail ("relist-scratch-before", error);
		}

		/* The move: `delete_originals=TRUE`, one `Email/set` that adds
		 * the destination's `mailboxIds` member and takes the inbox's
		 * away. Checked from both folders, not merely that the call
		 * answered -- the row has to leave the inbox as well as land
		 * in the scratch folder. */
		if (!transfer_one (inbox, scratch, appended_uid, TRUE,
				   "moved-uid", &error)) {
			g_object_unref (scratch);
			return fail ("transfer-move", error);
		}

		if (!report_refreshed_listing (inbox, appended_uid,
					       "inbox-after-move", &error)) {
			g_object_unref (scratch);
			return fail ("relist-inbox-after-move", error);
		}
		if (!report_refreshed_listing (scratch, appended_uid,
					       "scratch-after-move", &error)) {
			g_object_unref (scratch);
			return fail ("relist-scratch-after-move", error);
		}

		/* The destination's own summary row -- the refresh above is
		 * what wrote it, since a transfer only ever removes rows
		 * (`transfer.rs`'s "what is not decided here"). */
		filed_info = camel_folder_get_message_info (scratch, appended_uid);
		g_print ("moved-subject=%s\n",
			 filed_info ? camel_message_info_get_subject (filed_info) : "");
		g_clear_object (&filed_info);

		if (!report_reopened_filed ("reopened-moved", appended_uid,
					    scratch_name, &error)) {
			g_object_unref (scratch);
			return fail ("reopen-after-move", error);
		}

		/* The copy back: `delete_originals=FALSE`, the patch that only
		 * adds a `mailboxIds` member -- after which one message is in
		 * two folders under one uid. */
		if (!transfer_one (scratch, inbox, appended_uid, FALSE,
				   "copied-uid", &error)) {
			g_object_unref (scratch);
			return fail ("transfer-copy", error);
		}

		if (!report_refreshed_listing (inbox, appended_uid,
					       "inbox-after-copy", &error)) {
			g_object_unref (scratch);
			return fail ("relist-inbox-after-copy", error);
		}
		if (!report_refreshed_listing (scratch, appended_uid,
					       "scratch-after-copy", &error)) {
			g_object_unref (scratch);
			return fail ("relist-scratch-after-copy", error);
		}

		if (!report_reopened_filed ("reopened-copied", appended_uid,
					    scratch_name, &error)) {
			g_object_unref (scratch);
			return fail ("reopen-after-copy", error);
		}

		/* A third transfer shape the two above cannot produce: a move
		 * into a folder that already holds the message. The patch
		 * adds a `mailboxIds` member that is already there and takes
		 * the scratch folder's away, so the inbox must end up listing
		 * the message once, not twice. */
		if (!transfer_one (scratch, inbox, appended_uid, TRUE,
				   "cleanup-uid", &error)) {
			g_object_unref (scratch);
			return fail ("transfer-cleanup", error);
		}

		if (!report_refreshed_listing (inbox, appended_uid,
					       "inbox-after-cleanup", &error)) {
			g_object_unref (scratch);
			return fail ("relist-inbox-after-cleanup", error);
		}
		if (!report_refreshed_listing (scratch, appended_uid,
					       "scratch-after-cleanup", &error)) {
			g_object_unref (scratch);
			return fail ("relist-scratch-after-cleanup", error);
		}

		/* The folder can go now it is empty -- a JMAP server refuses
		 * to destroy a mailbox that still holds a message
		 * (`mailboxHasEmail`), which is why the cleanup move comes
		 * first. Unreffed before the delete, the way mail-client.c
		 * lets go of "Receipts" before renaming it. */
		g_object_unref (scratch);

		if (!camel_store_delete_folder_sync (store, scratch_name,
						     NULL, &error))
			return fail ("delete-folder", error);

		/* And the store's own listing has to agree it is gone, not
		 * merely the call answer -- manage.rs's own point, made here
		 * against a listing the real server backs. */
		info = camel_store_get_folder_info_sync (store, NULL,
							 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
							 NULL, &error);
		if (!info)
			return fail ("folder-info-after-delete", error);

		names = g_ptr_array_new_with_free_func (g_free);
		collect_folder_names (info, names);
		camel_folder_info_free (info);

		listed = FALSE;
		for (index = 0; index < names->len && !listed; index++)
			listed = g_strcmp0 (names->pdata[index], scratch_name) == 0;
		g_ptr_array_unref (names);
		g_print ("deleted-folder-listed=%d\n", listed ? 1 : 0);

		g_free (scratch_name);

		/* Leave the account where this run found it: the message
		 * marked deleted and expunged, the same sequence the flags
		 * phase measures as its own observation. */
		message_info = camel_folder_get_message_info (inbox, appended_uid);
		if (!message_info) {
			g_printerr ("summary: no message info to mark deleted\n");
			return 1;
		}
		camel_message_info_set_flags (message_info, CAMEL_MESSAGE_DELETED,
					      CAMEL_MESSAGE_DELETED);
		g_clear_object (&message_info);

		if (!camel_folder_expunge_sync (inbox, NULL, &error))
			return fail ("cleanup-expunge", error);

		uids = folder_dup_uids (inbox);
		g_print ("final-inbox-count=%u\n", uids->len);
		folder_free_uids (inbox, uids);
	}

	/* Camel schedules its own `folder_changed` emissions onto the default
	 * main context, and this program runs no main loop, so some are still
	 * pending here. `ESourceRegistry`'s dispose iterates that context,
	 * which would otherwise dispatch them in the middle of the teardown
	 * below: the closure's own unref then reaches a `CamelStore` whose
	 * finalizer is already holding the folder bag it wants, and the process
	 * stops there (seen as a hang on EDS 3.60.2, and as a crash once this
	 * file grew a second store). Draining them here, while everything they
	 * refer to is still alive, costs nothing and is what a program with a
	 * main loop of its own would have done all along. */
	while (g_main_context_iteration (NULL, FALSE))
		;

	g_free (appended_uid);
	g_object_unref (inbox);
	g_object_unref (store);
	g_object_unref (session);
	g_object_unref (account_source);
	g_object_unref (registry);

	return 0;
}
