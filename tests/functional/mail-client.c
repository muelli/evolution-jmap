/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the mail functional test. It is an ordinary libcamel
 * consumer, like the Evolution mail view. It knows nothing about JMAP or the
 * mock server. It opens a store for a source UID, lists the folder tree,
 * opens the inbox and reads the messages in it.
 *
 * `rust/crates/jmap-functional/tests/mail.rs` creates the scratch tree, the
 * `.source` keyfile, the private bus and the mock, and checks the output.
 * This program writes one `key=value` line for each observation. It exits
 * with a non-zero status when a call fails.
 *
 * Unlike book-client.c, this program is the host process. An EDS factory
 * daemon dlopens an address book backend, but the mail client itself dlopens
 * a Camel provider. Camel finds the provider through the `.urls` file beside
 * each object. Thus `store-connected=1` proves that the `.urls` file, the
 * protocol name and the entry point agree. These are in three files, and a
 * unit test cannot check them together, because it links the provider
 * directly.
 *
 * The program creates a CamelSession directly, without a subclass. Evolution
 * uses a subclass (EMailSession) for the vfuncs that need a user: passwords,
 * trust prompts and filters. The source names no user, so the base class
 * defaults have no effect. The provider's connect_sync asks the session to
 * authenticate, as every Camel provider does. The default authenticate_sync
 * then writes a warning that it "is not intended for production use". This
 * warning is expected, and shows that the authenticate path ran.
 *
 *   usage: functional-mail-client <source-uid>
 */

#include <camel/camel.h>
#include <libedataserver/libedataserver.h>

#include "test-util.h"

/* From 3.58, `camel_folder_dup_uids` replaces the borrowed
 * `camel_folder_get_uids`/`camel_folder_free_uids` pair. It returns a
 * reference-counted `GPtrArray` that the caller owns (see
 * `docs/eds-version-matrix.md`). `eds-sys` has wrappers with the same two
 * names in `compat`, but this client links no crate from this repository. */
static GPtrArray *
folder_dup_uids (CamelFolder *folder)
{
#if EDS_CHECK_VERSION(3, 57, 1)
	return camel_folder_dup_uids (folder);
#else
	return camel_folder_get_uids (folder);
#endif
}

static void
folder_free_uids (CamelFolder *folder,
		   GPtrArray *uids)
{
#if EDS_CHECK_VERSION(3, 57, 1)
	g_ptr_array_unref (uids);
#else
	camel_folder_free_uids (folder, uids);
#endif
}

/* Decodes the content of `message` into a stripped string, the same
 * transform the main body-reading loop below applies. The caller frees the
 * returned string. Returns NULL on a decode failure, with `error` set. */
static gchar *
decoded_body (CamelMimeMessage *message,
	      GError **error)
{
	GByteArray *body;
	CamelStream *stream;
	gchar *joined;

	body = g_byte_array_new ();
	stream = camel_stream_mem_new_with_byte_array (body);
	if (camel_data_wrapper_decode_to_stream_sync (
			camel_medium_get_content (CAMEL_MEDIUM (message)),
			stream, NULL, error) < 0) {
		g_object_unref (stream);
		return NULL;
	}

	/* The decoded bytes have no NUL terminator. The trailing newline is
	 * part of the MIME transfer, not of the text. */
	joined = g_strndup ((const gchar *) body->data, body->len);
	g_strstrip (joined);
	g_object_unref (stream);
	return joined;
}

/* Flattens the folder tree into a list of full names. This test checks the
 * names; the jmap-mail tests check the tree shape. */
static void
collect_folder_names (CamelFolderInfo *info,
		      GPtrArray *names)
{
	for (; info; info = info->next) {
		g_ptr_array_add (names, g_strdup (info->full_name));
		collect_folder_names (info->child, names);
	}
}

/* Writes one sorted `key=a,b,c` line. Each list is a set, because the provider
 * controls the order. `values` stays sorted and the caller keeps ownership. */
static void
report_sorted (const gchar *key,
	       GPtrArray *values)
{
	gchar *joined;

	g_ptr_array_sort_values (values, (GCompareFunc) g_strcmp0);
	/* g_strjoinv needs a NULL terminator. Add it after the sort. */
	g_ptr_array_add (values, NULL);
	joined = g_strjoinv (",", (gchar **) values->pdata);
	g_print ("%s=%s\n", key, joined);
	g_free (joined);
	/* Remove the terminator again. */
	g_ptr_array_remove_index (values, values->len - 1);
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
	CamelMimeMessage *message;
	GPtrArray *names;
	GPtrArray *uids;
	GPtrArray *subjects;
	GPtrArray *bodies;
	gchar *joined;
	gchar *appended_uid = NULL;
	gchar *flagged_uid = NULL;
	gchar *deleted_uid = NULL;
	guint index;
	const gchar *source_uid;
	const gchar *protocol;
	const gchar *data_dir;
	const gchar *cache_dir;
	int status = 0;
	gboolean offline_roundtrip;
	gboolean restart_prime;
	gboolean restart_second;
	gboolean offline_save_draft;
	gboolean offline_expunge_roundtrip;
	gboolean offline_append_restart_prime;
	gboolean offline_append_restart_second;
	gboolean offline_move_restart_prime;
	gboolean offline_move_restart_second;

	if (argc != 2 && argc != 3) {
		g_printerr ("usage: %s <source-uid> [offline-roundtrip|restart-prime|restart-second|offline-save-draft|offline-expunge-roundtrip|offline-append-restart-prime|offline-append-restart-second|offline-move-restart-prime|offline-move-restart-second]\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	offline_roundtrip = argc == 3 && g_strcmp0 (argv[2], "offline-roundtrip") == 0;
	restart_prime = argc == 3 && g_strcmp0 (argv[2], "restart-prime") == 0;
	restart_second = argc == 3 && g_strcmp0 (argv[2], "restart-second") == 0;
	offline_save_draft = argc == 3 && g_strcmp0 (argv[2], "offline-save-draft") == 0;
	offline_expunge_roundtrip = argc == 3 && g_strcmp0 (argv[2], "offline-expunge-roundtrip") == 0;
	offline_append_restart_prime = argc == 3 && g_strcmp0 (argv[2], "offline-append-restart-prime") == 0;
	offline_append_restart_second = argc == 3 && g_strcmp0 (argv[2], "offline-append-restart-second") == 0;
	offline_move_restart_prime = argc == 3 && g_strcmp0 (argv[2], "offline-move-restart-prime") == 0;
	offline_move_restart_second = argc == 3 && g_strcmp0 (argv[2], "offline-move-restart-second") == 0;

	/* The harness points these at the scratch tree. Camel keeps a summary
	 * database and a message cache for each service in them. The real XDG
	 * directories would put data into the developer's own Evolution store. */
	data_dir = g_get_user_data_dir ();
	cache_dir = g_get_user_cache_dir ();

	/* Reads the `.urls` files in EDS_CAMEL_PROVIDER_DIR. The harness
	 * points it at a directory that holds only the provider of this
	 * repository. This step dlopens nothing: a `.urls` file only gives the
	 * protocols of the object beside it. */
	camel_init (data_dir, FALSE);
	camel_provider_init ();

	/* Registers the ESourceCamel subtype for the settings of each
	 * provider. Without it, the `[JMAP Backend]` keyfile group does not
	 * parse into a CamelSettings object, and the store connects to the
	 * default host and port, which are empty. Evolution also calls this. */
	e_source_camel_register_types ();

	/* Starts evolution-source-registry on the private session bus. It
	 * reads the scratch sources directory from the harness. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return functional_fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* Read the protocol from the source, not from a literal. The keyfile
	 * `BackendName=jmap`, libcameljmap.urls and camel_provider_module_init
	 * each spell it, and all three must agree. A literal here would hide a
	 * disagreement. */
	if (!e_source_has_extension (source, E_SOURCE_EXTENSION_MAIL_ACCOUNT)) {
		g_printerr ("source '%s' is not a mail account\n", source_uid);
		return 1;
	}

	backend_extension = e_source_get_extension (source, E_SOURCE_EXTENSION_MAIL_ACCOUNT);
	protocol = e_source_backend_get_backend_name (backend_extension);
	g_print ("protocol=%s\n", protocol ? protocol : "");

	session = g_object_new (CAMEL_TYPE_SESSION,
				"user-data-dir", data_dir,
				"user-cache-dir", cache_dir,
				NULL);

	/* `restart-second` mode: item 176's question whether the message
	 * list of a folder opened before survives a restart with no network
	 * reachable, the way the folder tree already does (176(b)). This
	 * mode simulates a process that starts with the session already
	 * offline, the same as the shell's "Work Offline" state at startup:
	 * no connection is ever attempted, so a dead or unreachable mock
	 * cannot be told apart from "no network" here. */
	if (restart_second || offline_append_restart_second || offline_move_restart_second)
		camel_session_set_online (session, FALSE);

	/* Camel dlopens the provider here. It finds the protocol in the table
	 * from the `.urls` files and opens the object beside the file that
	 * claims it. An unclaimed protocol fails here with "no provider
	 * available". */
	service = camel_session_add_service (session, source_uid, protocol,
					     CAMEL_PROVIDER_STORE, &error);
	if (!service)
		return functional_fail ("add-service", error);

	/* Copies the keyfile settings to the service: the host, the port and
	 * the security method that connect_sync reads. */
	e_source_camel_configure_service (source, service);

	store = CAMEL_STORE (service);

	/* With the session already offline, `reconnect_if_dropped`
	 * (`jmap-mail/src/folders.rs`) never tries the network: it opens the
	 * inbox from the folder tree and the message summary this process
	 * finds already on disk, written by an earlier, online process at
	 * the same cache directory. No `camel_service_connect_sync` call at
	 * all, so a request here would prove this mode reached the network. */
	if (restart_second) {
		GPtrArray *uids;
		GPtrArray *subjects;
		guint restart_index;

		inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
		if (!inbox)
			return functional_fail ("restart-second-inbox", error);

		uids = folder_dup_uids (inbox);
		g_print ("restart-inbox-count=%u\n", uids->len);

		subjects = g_ptr_array_new_with_free_func (g_free);
		for (restart_index = 0; restart_index < uids->len; restart_index++) {
			const gchar *uid = uids->pdata[restart_index];
			CamelMessageInfo *message_info = camel_folder_get_message_info (inbox, uid);

			if (!message_info) {
				g_ptr_array_unref (subjects);
				folder_free_uids (inbox, uids);
				return functional_fail ("restart-second-message-info", NULL);
			}
			g_ptr_array_add (subjects,
					 g_strdup (camel_message_info_get_subject (message_info)));
			g_clear_object (&message_info);
		}
		folder_free_uids (inbox, uids);
		report_sorted ("restart-inbox-subjects", subjects);
		g_ptr_array_unref (subjects);
		g_object_unref (inbox);
		return 0;
	}

	/* `offline-append-restart-second` mode: the second half of item
	 * 176(b)'s restart-survival pair for a queued offline append
	 * (`offline-append-restart-prime` below primes the pending draft).
	 * Starts offline the same way `restart_second` does above, with no
	 * `camel_service_connect_sync` call at all, and reads Drafts from
	 * disk alone: a pending append whose row and bytes were lost on
	 * restart would show as an empty folder here. Then it goes online,
	 * the same generic `CamelOfflineStore` reconnect flush the other
	 * roundtrip modes above rely on, and reports Drafts again to see
	 * whether the pending draft reached the server. */
	if (offline_append_restart_second) {
		CamelFolder *drafts;
		GPtrArray *drafts_uids;
		GPtrArray *subjects;
		guint drafts_index;

		drafts = camel_store_get_folder_sync (store, "Drafts",
						       CAMEL_STORE_FOLDER_NONE,
						       NULL, &error);
		if (!drafts)
			return functional_fail ("offline-append-restart-second-drafts-folder", error);

		drafts_uids = folder_dup_uids (drafts);
		g_print ("restart-drafts-count=%u\n", drafts_uids->len);

		subjects = g_ptr_array_new_with_free_func (g_free);
		for (drafts_index = 0; drafts_index < drafts_uids->len; drafts_index++) {
			const gchar *uid = drafts_uids->pdata[drafts_index];
			CamelMessageInfo *message_info = camel_folder_get_message_info (drafts, uid);

			if (!message_info) {
				g_ptr_array_unref (subjects);
				folder_free_uids (drafts, drafts_uids);
				g_object_unref (drafts);
				return functional_fail ("offline-append-restart-second-message-info", NULL);
			}
			g_ptr_array_add (subjects,
					 g_strdup (camel_message_info_get_subject (message_info)));
			g_clear_object (&message_info);
		}
		folder_free_uids (drafts, drafts_uids);
		report_sorted ("restart-drafts-subjects", subjects);
		g_ptr_array_unref (subjects);

		/* This process started with `camel_session_set_online (session,
		 * FALSE)`, the session-level flag, not only the store's own.
		 * `camel_offline_store_set_online_sync`'s generic reconnect
		 * flush (`camel-offline-store.c`) gates its whole synchronize
		 * walk on `camel_session_get_online`, so the store alone going
		 * online would silently skip the flush. The real shell flips
		 * the session first, on a GNetworkMonitor callback, before any
		 * store's own online state follows; this mirrors that order. */
		camel_session_set_online (session, TRUE);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), TRUE, NULL, &error)) {
			g_object_unref (drafts);
			return functional_fail ("offline-append-restart-second-go-online", error);
		}
		g_print ("went-online=1\n");

		if (!camel_folder_refresh_info_sync (drafts, NULL, &error)) {
			g_object_unref (drafts);
			return functional_fail ("offline-append-restart-second-refresh", error);
		}
		drafts_uids = folder_dup_uids (drafts);
		g_print ("drafts-count-after-reconnect=%u\n", drafts_uids->len);
		folder_free_uids (drafts, drafts_uids);

		g_object_unref (drafts);
		return 0;
	}

	/* `offline-move-restart-second` mode: the second half of item 181(a)
	 * section 4's verification pair (`offline-move-restart-prime` above
	 * primes the pending move). Starts offline like `restart_second`
	 * above, with neither Inbox nor Trash opened in this process, and
	 * goes straight online: Camel's own reconnect flush
	 * (`camel_offline_store_set_online_sync` calling
	 * `camel_store_synchronize_sync` with `expunge=FALSE`) walks only
	 * `camel_store_dup_opened_folders`, a weak-ref bag of the folder
	 * objects this process already has live (`camel-object-bag.c`,
	 * `camel-store.c`'s `store_synchronize_sync`); a fresh process has
	 * none. This checks whether that blind reconnect flushes the queued
	 * move anyway, then opens both folders (as Evolution would once the
	 * user selects or restores one) and synchronizes Inbox directly, to
	 * see whether the queued filing still exists and lands once a
	 * folder that holds it is actually opened and synchronized. */
	if (offline_move_restart_second) {
		CamelFolder *inbox;
		CamelFolder *trash;
		GPtrArray *uids;

		camel_session_set_online (session, TRUE);
		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), TRUE, NULL, &error))
			return functional_fail ("offline-move-restart-second-go-online", error);
		g_print ("went-online=1\n");

		inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
		if (!inbox)
			return functional_fail ("offline-move-restart-second-inbox", error);
		if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
			g_object_unref (inbox);
			return functional_fail ("offline-move-restart-second-refresh-inbox", error);
		}
		uids = folder_dup_uids (inbox);
		g_print ("restart-inbox-count-blind-reconnect=%u\n", uids->len);
		folder_free_uids (inbox, uids);

		trash = camel_store_get_trash_folder_sync (store, NULL, &error);
		if (!trash) {
			g_object_unref (inbox);
			return functional_fail ("offline-move-restart-second-trash", error);
		}
		if (!camel_folder_refresh_info_sync (trash, NULL, &error)) {
			g_object_unref (inbox);
			g_object_unref (trash);
			return functional_fail ("offline-move-restart-second-refresh-trash", error);
		}
		uids = folder_dup_uids (trash);
		g_print ("restart-trash-count-blind-reconnect=%u\n", uids->len);
		folder_free_uids (trash, uids);

		/* Inbox is open now, the same as the moment Evolution's own
		 * mail view selects or closes a folder. Synchronize it
		 * directly to see whether the queued move still exists and
		 * lands once something actually synchronizes the folder that
		 * holds it, as opposed to the blind reconnect flush above. */
		if (!camel_folder_synchronize_sync (inbox, FALSE, NULL, &error)) {
			g_object_unref (inbox);
			g_object_unref (trash);
			return functional_fail ("offline-move-restart-second-synchronize", error);
		}
		g_print ("inbox-synchronized-after-reopen=1\n");

		if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
			g_object_unref (inbox);
			g_object_unref (trash);
			return functional_fail ("offline-move-restart-second-refresh-inbox-after-sync", error);
		}
		uids = folder_dup_uids (inbox);
		g_print ("restart-inbox-count-after-synchronize=%u\n", uids->len);
		folder_free_uids (inbox, uids);

		if (!camel_folder_refresh_info_sync (trash, NULL, &error)) {
			g_object_unref (inbox);
			g_object_unref (trash);
			return functional_fail ("offline-move-restart-second-refresh-trash-after-sync", error);
		}
		uids = folder_dup_uids (trash);
		g_print ("restart-trash-count-after-synchronize=%u\n", uids->len);
		folder_free_uids (trash, uids);

		g_object_unref (inbox);
		g_object_unref (trash);
		return 0;
	}

	if (!camel_service_connect_sync (service, NULL, &error))
		return functional_fail ("connect", error);

	g_print ("store-connected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	/* `restart-prime` mode: the first half of the restart pair above. It
	 * connects normally, refreshes the inbox once (so the message list
	 * and the folder's own summary state reach disk), reports what it
	 * saw, and exits before the rest of main() runs any of the folder
	 * create/rename/transfer operations below, which this test does not
	 * want in the inbox it primes. */
	if (restart_prime) {
		GPtrArray *uids;
		GPtrArray *subjects;
		guint prime_index;

		inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
		if (!inbox)
			return functional_fail ("restart-prime-inbox", error);
		if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
			return functional_fail ("restart-prime-refresh", error);

		/* `camel_folder_changed` (`jmap-mail/src/refresh.rs`'s call
		 * at the end of a refresh) schedules its signal on a
		 * low-priority idle of the session's main context
		 * (`camel-folder.c`); the default handler that saves the
		 * summary to disk runs only once that idle fires. Evolution
		 * always has a running main loop, so it gets there on its
		 * own; this program is a synchronous client with none, so it
		 * drains the context itself before it exits. Without this,
		 * the refreshed rows exist only in memory, and a second
		 * process at the same cache directory finds no saved state. */
		while (g_main_context_pending (NULL))
			g_main_context_iteration (NULL, FALSE);

		uids = folder_dup_uids (inbox);
		g_print ("restart-inbox-count=%u\n", uids->len);

		subjects = g_ptr_array_new_with_free_func (g_free);
		for (prime_index = 0; prime_index < uids->len; prime_index++) {
			const gchar *uid = uids->pdata[prime_index];
			CamelMessageInfo *message_info = camel_folder_get_message_info (inbox, uid);

			if (!message_info) {
				g_ptr_array_unref (subjects);
				folder_free_uids (inbox, uids);
				return functional_fail ("restart-prime-message-info", NULL);
			}
			g_ptr_array_add (subjects,
					 g_strdup (camel_message_info_get_subject (message_info)));
			g_clear_object (&message_info);
		}
		folder_free_uids (inbox, uids);
		report_sorted ("restart-inbox-subjects", subjects);
		g_ptr_array_unref (subjects);
		g_object_unref (inbox);
		return 0;
	}

	/* `offline-append-restart-prime` mode: the first half of item
	 * 176(b)'s restart-survival pair for a queued offline append.
	 * Connects normally, reads Drafts once (so its folder tree and
	 * starting summary reach disk, the same `restart-prime` reason
	 * above), goes offline, appends a draft message, and exits while
	 * still offline: no reconnect in this process, simulating an
	 * application killed before the user ever went back online.
	 * `offline-append-restart-second` above opens the same cache
	 * directory fresh and checks whether the pending draft survived. */
	if (offline_append_restart_prime) {
		CamelFolder *drafts;
		GPtrArray *drafts_uids;
		static const gchar draft_message[] =
			"From: Alice <alice@example.com>\r\n"
			"To: Carol <carol@example.com>\r\n"
			"Subject: Draft while offline\r\n"
			"Message-ID: <offline-restart-draft@example.com>\r\n"
			"Date: Thu, 15 Jan 2026 12:00:00 +0000\r\n"
			"\r\n"
			"Written with no network, then the process restarts.\r\n";
		CamelMimeMessage *draft;
		gchar *draft_uid = NULL;
		GError *draft_error = NULL;

		drafts = camel_store_get_folder_sync (store, "Drafts",
						       CAMEL_STORE_FOLDER_NONE,
						       NULL, &error);
		if (!drafts)
			return functional_fail ("offline-append-restart-prime-drafts-folder", error);

		if (!camel_folder_refresh_info_sync (drafts, NULL, &error)) {
			g_object_unref (drafts);
			return functional_fail ("offline-append-restart-prime-refresh", error);
		}
		drafts_uids = folder_dup_uids (drafts);
		g_print ("drafts-count-before=%u\n", drafts_uids->len);
		folder_free_uids (drafts, drafts_uids);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), FALSE, NULL, &error)) {
			g_object_unref (drafts);
			return functional_fail ("offline-append-restart-prime-go-offline", error);
		}
		g_print ("went-offline=1\n");

		draft = camel_mime_message_new ();
		if (!camel_data_wrapper_construct_from_data_sync (
				CAMEL_DATA_WRAPPER (draft),
				draft_message, sizeof (draft_message) - 1,
				NULL, &error)) {
			g_object_unref (draft);
			g_object_unref (drafts);
			return functional_fail ("offline-append-restart-prime-parse", error);
		}

		if (!camel_folder_append_message_sync (drafts, draft, NULL,
							&draft_uid, NULL, &draft_error)) {
			g_object_unref (draft);
			g_object_unref (drafts);
			g_print ("offline-append-restart-appended=0\n");
			g_print ("offline-append-restart-error=%s\n",
				 draft_error ? draft_error->message : "(no detail)");
			g_clear_error (&draft_error);
			return 1;
		}
		g_object_unref (draft);
		g_print ("offline-append-restart-appended=1\n");
		g_print ("offline-append-restart-uid=%s\n", draft_uid ? draft_uid : "");
		g_free (draft_uid);

		/* As in `restart-prime` above: the append schedules
		 * `camel_folder_changed` on a low-priority idle, and the
		 * default handler that saves the summary to disk only runs
		 * once that idle fires. Drain it here so the pending-append
		 * row and the synthetic summary row both reach disk before
		 * this process exits. */
		while (g_main_context_pending (NULL))
			g_main_context_iteration (NULL, FALSE);

		g_object_unref (drafts);
		/* Exit while still offline: no reconnect in this process. */
		return 0;
	}

	/* `offline-move-restart-prime` mode: the first half of item 181(a)
	 * section 4's verification pair (`offline-move-restart-second` below
	 * reads the result with neither folder reopened). Connects normally,
	 * reads Inbox and Trash once so their folder tree and starting
	 * summary reach disk (the `restart-prime` reason above), goes
	 * offline, then moves the first inbox message into Trash:
	 * `transfer_messages_to_sync` queues the filing on the inbox row
	 * instead of failing (`jmap-mail/src/transfer.rs`), and leaves the
	 * row in Inbox until the filing lands (176(b)'s existing rule).
	 * Exits while still offline, dropping both folder objects, so the
	 * second process starts with neither one open. */
	if (offline_move_restart_prime) {
		CamelFolder *inbox;
		CamelFolder *trash;
		GPtrArray *uids;
		GPtrArray *transfer_uids;
		GPtrArray *transferred = NULL;
		gchar *move_uid;

		inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
		if (!inbox)
			return functional_fail ("offline-move-restart-prime-inbox", error);
		if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
			g_object_unref (inbox);
			return functional_fail ("offline-move-restart-prime-refresh-inbox", error);
		}

		trash = camel_store_get_trash_folder_sync (store, NULL, &error);
		if (!trash) {
			g_object_unref (inbox);
			return functional_fail ("offline-move-restart-prime-trash", error);
		}
		if (!camel_folder_refresh_info_sync (trash, NULL, &error)) {
			g_object_unref (inbox);
			g_object_unref (trash);
			return functional_fail ("offline-move-restart-prime-refresh-trash", error);
		}

		uids = folder_dup_uids (inbox);
		if (uids->len == 0) {
			folder_free_uids (inbox, uids);
			g_object_unref (inbox);
			g_object_unref (trash);
			return functional_fail ("offline-move-restart-prime-no-message", NULL);
		}
		move_uid = g_strdup (uids->pdata[0]);
		folder_free_uids (inbox, uids);
		g_print ("offline-move-restart-uid=%s\n", move_uid);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), FALSE, NULL, &error)) {
			g_free (move_uid);
			g_object_unref (inbox);
			g_object_unref (trash);
			return functional_fail ("offline-move-restart-prime-go-offline", error);
		}
		g_print ("went-offline=1\n");

		transfer_uids = g_ptr_array_new ();
		g_ptr_array_add (transfer_uids, move_uid);

		if (!camel_folder_transfer_messages_to_sync (inbox, transfer_uids,
							      trash, TRUE,
							      &transferred, NULL, &error)) {
			g_ptr_array_unref (transfer_uids);
			g_free (move_uid);
			g_object_unref (inbox);
			g_object_unref (trash);
			g_print ("offline-move-restart-queued=0\n");
			return functional_fail ("offline-move-restart-prime-transfer", error);
		}
		g_ptr_array_unref (transfer_uids);
		g_print ("offline-move-restart-queued=1\n");
		if (transferred) {
			guint t;

			for (t = 0; t < transferred->len; t++)
				g_free (transferred->pdata[t]);
			g_ptr_array_unref (transferred);
		}

		uids = folder_dup_uids (inbox);
		g_print ("inbox-count-after-offline-move=%u\n", uids->len);
		folder_free_uids (inbox, uids);
		uids = folder_dup_uids (trash);
		g_print ("trash-count-after-offline-move=%u\n", uids->len);
		folder_free_uids (trash, uids);

		/* As in `restart-prime` above: `camel_folder_changed` and the
		 * pending-filing mark both reach disk only once the default
		 * handler runs on a low-priority idle. Drain it here. */
		while (g_main_context_pending (NULL))
			g_main_context_iteration (NULL, FALSE);

		g_free (move_uid);
		g_object_unref (inbox);
		g_object_unref (trash);
		/* Exit while still offline: no reconnect in this process, and
		 * neither folder is reopened before it exits. */
		return 0;
	}

	/* `offline-roundtrip` mode: item 176's question whether the generic
	 * `CamelOfflineStore` protocol, not a per-provider write, flushes a
	 * flag changed while offline once the application calls
	 * `camel_offline_store_set_online_sync (TRUE)` again, the same call
	 * the shell makes on a network-monitor reconnect. It flags the first
	 * inbox message while the store reports itself offline, then goes
	 * back online and exits: the Rust harness checks the mock's own copy
	 * of the message to see whether the flush reached the server,
	 * without this program ever calling `camel_folder_synchronize_sync`
	 * itself. Returns here, before the subscription and trash/junk
	 * blocks below: it needs only the inbox, and the mock of this mode
	 * seeds no other mailbox.
	 *
	 * It also opens the message body once while online, then again while
	 * offline, to answer a second item 176 question: does a message read
	 * before going offline stay readable offline? `jmap-mail`'s
	 * `get_message_sync` reads its cache before it checks the connection
	 * (`message.rs`), so this is expected to work with no extra wiring;
	 * the Rust harness compares the two bodies to confirm it from a real
	 * Camel client. A failure here reports it rather than exiting, so the
	 * harness can tell a missing body from a client crash. */
	if (offline_roundtrip) {
		GPtrArray *uids;
		const gchar *uid;
		CamelMessageInfo *flag_info;
		CamelMimeMessage *primed;
		gchar *online_body;

		inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
		if (!inbox)
			return functional_fail ("offline-roundtrip-inbox", error);
		if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
			return functional_fail ("offline-roundtrip-refresh", error);

		uids = folder_dup_uids (inbox);
		if (uids->len == 0) {
			folder_free_uids (inbox, uids);
			return functional_fail ("offline-roundtrip-no-message", NULL);
		}
		uid = uids->pdata[0];

		primed = camel_folder_get_message_sync (inbox, uid, NULL, &error);
		if (!primed) {
			folder_free_uids (inbox, uids);
			return functional_fail ("offline-roundtrip-prime-message", error);
		}
		online_body = decoded_body (primed, &error);
		g_object_unref (primed);
		if (!online_body) {
			folder_free_uids (inbox, uids);
			return functional_fail ("offline-roundtrip-prime-body", error);
		}
		g_print ("online-message-body=%s\n", online_body);
		g_free (online_body);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), FALSE, NULL, &error)) {
			folder_free_uids (inbox, uids);
			return functional_fail ("offline-roundtrip-go-offline", error);
		}
		g_print ("went-offline=1\n");

		{
			CamelMimeMessage *offline_message;
			GError *offline_error = NULL;

			offline_message = camel_folder_get_message_sync (inbox, uid, NULL, &offline_error);
			if (!offline_message) {
				g_print ("offline-message-error=%s\n",
					 offline_error ? offline_error->message : "(no detail)");
				g_clear_error (&offline_error);
			} else {
				gchar *offline_body = decoded_body (offline_message, &offline_error);

				g_object_unref (offline_message);
				if (!offline_body) {
					g_print ("offline-message-error=%s\n",
						 offline_error ? offline_error->message : "(no detail)");
					g_clear_error (&offline_error);
				} else {
					g_print ("offline-message-body=%s\n", offline_body);
					g_free (offline_body);
				}
			}
		}

		flag_info = camel_folder_get_message_info (inbox, uid);
		if (!flag_info) {
			folder_free_uids (inbox, uids);
			return functional_fail ("offline-roundtrip-get-message-info", NULL);
		}
		camel_message_info_set_flags (flag_info, CAMEL_MESSAGE_SEEN, CAMEL_MESSAGE_SEEN);
		g_clear_object (&flag_info);
		g_print ("offline-flag-uid=%s\n", uid);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), TRUE, NULL, &error)) {
			folder_free_uids (inbox, uids);
			return functional_fail ("offline-roundtrip-go-online", error);
		}
		g_print ("went-online=1\n");

		folder_free_uids (inbox, uids);
		return 0;
	}

	/* `offline-save-draft` mode: item 176(a)'s "save a draft" row. Does
	 * `append_message_sync` queue a message for later upload when the
	 * store is offline, the way `synchronize_sync` leaves a flag change
	 * queued (see `going_back_online_flushes_a_flag_changed_while_
	 * offline` above), or does it fail outright with nothing saved
	 * anywhere? Evolution's own `em_utils_composer_save_to_drafts_cb`
	 * (`em-composer-utils.c`) has no online check at all, unlike the
	 * actual Send path, so a real composer reaches this vfunc directly
	 * while offline. This mode goes offline, tries to append a message to
	 * Drafts, reports whether that succeeded, then goes back online and
	 * reports whether Drafts gained the message anyway (a flush, the way
	 * the flag change above flushes) or stayed exactly as before (a lost
	 * write). */
	if (offline_save_draft) {
		CamelFolder *drafts;
		GPtrArray *drafts_uids;
		static const gchar draft_message[] =
			"From: Alice <alice@example.com>\r\n"
			"To: Carol <carol@example.com>\r\n"
			"Subject: Draft while offline\r\n"
			"Message-ID: <offline-draft@example.com>\r\n"
			"Date: Thu, 15 Jan 2026 12:00:00 +0000\r\n"
			"\r\n"
			"Written with no network.\r\n";
		CamelMimeMessage *draft;
		gchar *draft_uid = NULL;
		GError *draft_error = NULL;

		drafts = camel_store_get_folder_sync (store, "Drafts",
						       CAMEL_STORE_FOLDER_NONE,
						       NULL, &error);
		if (!drafts)
			return functional_fail ("offline-save-draft-drafts-folder", error);

		if (!camel_folder_refresh_info_sync (drafts, NULL, &error)) {
			g_object_unref (drafts);
			return functional_fail ("offline-save-draft-refresh", error);
		}
		drafts_uids = folder_dup_uids (drafts);
		g_print ("drafts-count-before=%u\n", drafts_uids->len);
		folder_free_uids (drafts, drafts_uids);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), FALSE, NULL, &error)) {
			g_object_unref (drafts);
			return functional_fail ("offline-save-draft-go-offline", error);
		}
		g_print ("went-offline=1\n");

		draft = camel_mime_message_new ();
		if (!camel_data_wrapper_construct_from_data_sync (
				CAMEL_DATA_WRAPPER (draft),
				draft_message, sizeof (draft_message) - 1,
				NULL, &error)) {
			g_object_unref (draft);
			g_object_unref (drafts);
			return functional_fail ("offline-save-draft-parse", error);
		}

		if (camel_folder_append_message_sync (drafts, draft, NULL,
						       &draft_uid, NULL, &draft_error)) {
			g_print ("offline-save-draft-appended=1\n");
			g_print ("offline-save-draft-uid=%s\n", draft_uid ? draft_uid : "");
			g_free (draft_uid);
		} else {
			g_print ("offline-save-draft-appended=0\n");
			g_print ("offline-save-draft-error=%s\n",
				 draft_error ? draft_error->message : "(no detail)");
			g_clear_error (&draft_error);
		}
		g_object_unref (draft);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), TRUE, NULL, &error)) {
			g_object_unref (drafts);
			return functional_fail ("offline-save-draft-go-online", error);
		}
		g_print ("went-online=1\n");

		if (!camel_folder_refresh_info_sync (drafts, NULL, &error)) {
			g_object_unref (drafts);
			return functional_fail ("offline-save-draft-refresh-after", error);
		}
		drafts_uids = folder_dup_uids (drafts);
		g_print ("drafts-count-after=%u\n", drafts_uids->len);
		folder_free_uids (drafts, drafts_uids);

		g_object_unref (drafts);
		return 0;
	}

	/* `offline-expunge-roundtrip` mode: item 176(b)'s offline-journal fix
	 * for `expunge_sync`. `CAMEL_MESSAGE_DELETED` is already a durable
	 * local mark (see `expunge.rs`'s module doc): a row stays marked until
	 * a server write actually removes it, and every walk of the summary,
	 * including the one `synchronize_sync` runs, finds a previously marked
	 * row again. So unlike a save-a-draft, no new local record is needed
	 * here, only that `expunge_sync` not fail outright while offline. This
	 * mode goes offline, marks the first inbox message deleted (after, not
	 * before: `camel_offline_store_set_online_sync` itself synchronizes
	 * every open folder as a side effect, while still connected for a
	 * moment before it actually disconnects, so marking a row dirty first
	 * would let that call alone flush it, proving nothing about the
	 * offline path), calls `camel_folder_expunge_sync`, reports whether
	 * that call itself succeeded and the inbox listing right after, then
	 * goes back online with no explicit synchronize call (the same
	 * generic EDS reconnect flush that `offline-roundtrip` above already
	 * proved exists) and reports the listing again, to see whether the
	 * deferred removal reached the server. */
	if (offline_expunge_roundtrip) {
		GPtrArray *uids;
		gchar *expunge_uid;
		CamelMessageInfo *delete_info;
		GError *expunge_error = NULL;

		inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
		if (!inbox)
			return functional_fail ("offline-expunge-inbox", error);
		if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
			return functional_fail ("offline-expunge-refresh", error);

		uids = folder_dup_uids (inbox);
		if (uids->len == 0) {
			folder_free_uids (inbox, uids);
			return functional_fail ("offline-expunge-no-message", NULL);
		}
		expunge_uid = g_strdup (uids->pdata[0]);
		folder_free_uids (inbox, uids);
		g_print ("offline-expunge-uid=%s\n", expunge_uid);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), FALSE, NULL, &error)) {
			g_free (expunge_uid);
			return functional_fail ("offline-expunge-go-offline", error);
		}
		g_print ("went-offline=1\n");

		delete_info = camel_folder_get_message_info (inbox, expunge_uid);
		if (!delete_info) {
			g_free (expunge_uid);
			return functional_fail ("offline-expunge-get-message-info", NULL);
		}
		camel_message_info_set_flags (delete_info, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DELETED);
		g_clear_object (&delete_info);

		if (camel_folder_expunge_sync (inbox, NULL, &expunge_error)) {
			g_print ("offline-expunge-succeeded=1\n");
		} else {
			g_print ("offline-expunge-succeeded=0\n");
			g_print ("offline-expunge-error=%s\n",
				 expunge_error ? expunge_error->message : "(no detail)");
			g_clear_error (&expunge_error);
		}

		uids = folder_dup_uids (inbox);
		g_print ("inbox-count-after-offline-expunge=%u\n", uids->len);
		folder_free_uids (inbox, uids);

		if (!camel_offline_store_set_online_sync (
				CAMEL_OFFLINE_STORE (store), TRUE, NULL, &error)) {
			g_free (expunge_uid);
			return functional_fail ("offline-expunge-go-online", error);
		}
		g_print ("went-online=1\n");

		if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
			g_free (expunge_uid);
			return functional_fail ("offline-expunge-refresh-after", error);
		}
		uids = folder_dup_uids (inbox);
		g_print ("inbox-count-after-reconnect=%u\n", uids->len);
		folder_free_uids (inbox, uids);

		g_free (expunge_uid);
		g_object_unref (inbox);
		return 0;
	}

	info = camel_store_get_folder_info_sync (store, NULL,
						 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
						 NULL, &error);
	if (!info)
		return functional_fail ("folder-info", error);

	names = g_ptr_array_new_with_free_func (g_free);
	collect_folder_names (info, names);
	camel_folder_info_free (info);

	report_sorted ("folders", names);
	g_ptr_array_unref (names);

	/* `CamelSubscribable` controls the tick beside a folder in the
	 * subscription editor. The two folders change in opposite directions,
	 * so no write sets a value that is already there. "Sent" starts
	 * subscribed, as all mailboxes from the mock do, and this block
	 * unsubscribes it. "Drafts" starts unsubscribed (the Rust harness
	 * seeds it so), and this block subscribes it. `folder_is_subscribed`
	 * is a non-blocking method with no cancellable or error, so it needs
	 * no functional_fail() check. */
	{
		CamelSubscribable *subscribable = CAMEL_SUBSCRIBABLE (store);

		g_print ("sent-subscribed-initially=%d\n",
			 camel_subscribable_folder_is_subscribed (subscribable, "Sent") ? 1 : 0);
		if (!camel_subscribable_unsubscribe_folder_sync (subscribable, "Sent", NULL, &error))
			return functional_fail ("unsubscribe-sent", error);
		g_print ("sent-subscribed-after-unsubscribe=%d\n",
			 camel_subscribable_folder_is_subscribed (subscribable, "Sent") ? 1 : 0);

		g_print ("drafts-subscribed-initially=%d\n",
			 camel_subscribable_folder_is_subscribed (subscribable, "Drafts") ? 1 : 0);
		if (!camel_subscribable_subscribe_folder_sync (subscribable, "Drafts", NULL, &error))
			return functional_fail ("subscribe-drafts", error);
		g_print ("drafts-subscribed-after-subscribe=%d\n",
			 camel_subscribable_folder_is_subscribed (subscribable, "Drafts") ? 1 : 0);
	}

	/* Get the inbox by role, not by name. The provider answers from the
	 * JMAP role of the mailbox. A wrong answer gives a folder that only
	 * has the name of an inbox. */
	inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
	if (!inbox)
		return functional_fail ("inbox", error);

	g_print ("inbox-full-name=%s\n", camel_folder_get_full_name (inbox));

	/* Trash and junk use the same lookup by role, not the inherited vTrash
	 * and vJunk virtual folders. */
	trash = camel_store_get_trash_folder_sync (store, NULL, &error);
	if (!trash)
		return functional_fail ("trash", error);
	g_print ("trash-full-name=%s\n", camel_folder_get_full_name (trash));
	g_object_unref (trash);

	junk = camel_store_get_junk_folder_sync (store, NULL, &error);
	if (!junk)
		return functional_fail ("junk", error);
	g_print ("junk-full-name=%s\n", camel_folder_get_full_name (junk));
	g_object_unref (junk);

	if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
		return functional_fail ("refresh", error);

	uids = folder_dup_uids (inbox);
	g_print ("inbox-count=%u\n", uids->len);

	/* Read each message two times: from the summary and as a full message.
	 * These are different requests. The summaries come from Email/query
	 * and Email/get. A body comes from a blob download, which is a plain
	 * HTTP GET, not a method call. A provider that lists mail that it
	 * cannot open is a frequent failure.
	 *
	 * Read all messages, not only the first, because the provider controls
	 * the UID order. Each list is sorted, so the output is a set. */
	subjects = g_ptr_array_new_with_free_func (g_free);
	bodies = g_ptr_array_new_with_free_func (g_free);

	for (index = 0; index < uids->len; index++) {
		const gchar *uid = uids->pdata[index];
		CamelMessageInfo *message_info;

		/* Keep the first and second UIDs for the `synchronize_sync`
		 * and `expunge_sync` blocks below. Which messages they are is
		 * not important, because those blocks read back only the rows
		 * they wrote. Use two different messages: `expunge_sync`
		 * destroys its message, and the Rust harness must still find
		 * the `synchronize_sync` message on the mock to check its
		 * keyword. */
		if (index == 0)
			flagged_uid = g_strdup (uid);
		else if (index == 1)
			deleted_uid = g_strdup (uid);

		message_info = camel_folder_get_message_info (inbox, uid);
		if (!message_info) {
			g_printerr ("summary: no message info for uid '%s'\n", uid);
			status = 1;
			break;
		}

		g_ptr_array_add (subjects,
				 g_strdup (camel_message_info_get_subject (message_info)));
		g_clear_object (&message_info);

		message = camel_folder_get_message_sync (inbox, uid, NULL, &error);
		if (!message) {
			folder_free_uids (inbox, uids);
			return functional_fail ("get-message", error);
		}

		joined = decoded_body (message, &error);
		g_object_unref (message);
		if (!joined) {
			folder_free_uids (inbox, uids);
			return functional_fail ("message-body", error);
		}
		g_ptr_array_add (bodies, joined);
	}

	folder_free_uids (inbox, uids);

	report_sorted ("inbox-subjects", subjects);
	report_sorted ("message-bodies", bodies);
	g_ptr_array_unref (subjects);
	g_ptr_array_unref (bodies);

	/* `synchronize_sync` is the write path. A flag on a message stays a
	 * local Camel flag until a synchronize sends it to the server as a
	 * keyword. Only this block writes message flags, so only this block
	 * reaches that vfunc. */
	if (!flagged_uid)
		return functional_fail ("no-message-to-flag", NULL);

	g_print ("flagged-uid=%s\n", flagged_uid);
	{
		CamelMessageInfo *flag_info;

		flag_info = camel_folder_get_message_info (inbox, flagged_uid);
		if (!flag_info) {
			g_free (flagged_uid);
			return functional_fail ("get-message-info-for-flag", NULL);
		}
		camel_message_info_set_flags (flag_info, CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FLAGGED);
		g_clear_object (&flag_info);
	}

	if (!camel_folder_synchronize_sync (inbox, FALSE, NULL, &error)) {
		g_free (flagged_uid);
		return functional_fail ("synchronize", error);
	}

	/* A fresh read of the row shows the flag. A synchronize that ignored a
	 * write failure also shows it, so this does not prove that the server
	 * got the write. The Rust harness checks the method log and stored
	 * keywords of the mock for that. */
	{
		CamelMessageInfo *flag_info;

		flag_info = camel_folder_get_message_info (inbox, flagged_uid);
		if (!flag_info) {
			g_free (flagged_uid);
			return functional_fail ("get-message-info-after-sync", NULL);
		}
		g_print ("flagged-after-sync=%d\n",
			 (camel_message_info_get_flags (flag_info) & CAMEL_MESSAGE_FLAGGED) ? 1 : 0);
		g_clear_object (&flag_info);
	}

	/* The program frees `flagged_uid` at the end. `expunge_sync` below
	 * uses a different message, `deleted_uid`. */

	/* `append_message_sync` adds a message that this account does not
	 * have, for example from another account, a dropped `.eml` file or a
	 * filter. `Email/set` transfer is different: it moves a message that
	 * the account already has. Parse the message from raw RFC 5322 bytes,
	 * as the `jmap-mail` append tests do. Then Camel does the parse, and
	 * the write cannot disagree with it. */
	{
		static const gchar outside_message[] =
			"From: Dave <dave@example.com>\r\n"
			"To: Alice <alice@example.com>\r\n"
			"Subject: Dropped in\r\n"
			"Message-ID: <dropped@example.com>\r\n"
			"Date: Thu, 15 Jan 2026 11:00:00 +0000\r\n"
			"\r\n"
			"Found on the floor.\r\n";
		CamelMimeMessage *outside;
		CamelMimeMessage *reread;

		outside = camel_mime_message_new ();
		if (!camel_data_wrapper_construct_from_data_sync (
				CAMEL_DATA_WRAPPER (outside),
				outside_message, sizeof (outside_message) - 1,
				NULL, &error)) {
			g_object_unref (outside);
			return functional_fail ("parse-outside-message", error);
		}

		if (!camel_folder_append_message_sync (inbox, outside, NULL,
							&appended_uid, NULL, &error)) {
			g_object_unref (outside);
			return functional_fail ("append-message", error);
		}
		g_object_unref (outside);

		g_print ("append-uid=%s\n", appended_uid ? appended_uid : "");

		/* The folder listing writes the row, not the append. The
		 * message shows only after the next refresh. */
		if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
			return functional_fail ("refresh-after-append", error);

		uids = folder_dup_uids (inbox);
		g_print ("inbox-count-after-append=%u\n", uids->len);

		reread = camel_folder_get_message_sync (inbox, appended_uid, NULL, &error);
		folder_free_uids (inbox, uids);
		if (!reread)
			return functional_fail ("get-appended-message", error);

		g_print ("appended-subject=%s\n", camel_mime_message_get_subject (reread));
		g_object_unref (reread);

		/* Keep `appended_uid`: `transfer_messages_to_sync` below moves
		 * this message into "Receipts". */
	}

	/* Create "Receipts" at the account root through `create_folder_sync`
	 * on the live store, not through the decision function that the
	 * `jmap-mail` unit tests call. A NULL parent is the account itself, as
	 * in the `manage.rs` tests. */
	{
		CamelFolderInfo *created;

		created = camel_store_create_folder_sync (store, NULL, "Receipts", NULL, &error);
		if (!created)
			return functional_fail ("create-folder", error);

		g_print ("create-folder-name=%s\n", created->full_name);
		camel_folder_info_free (created);

		/* The `manage.rs` module doc gives the purpose of the vfunc:
		 * the store listing must agree with what the store told Camel
		 * it made. A success return alone is not sufficient. */
		info = camel_store_get_folder_info_sync (store, NULL,
							 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
							 NULL, &error);
		if (!info)
			return functional_fail ("folder-info-after-create", error);

		names = g_ptr_array_new_with_free_func (g_free);
		collect_folder_names (info, names);
		camel_folder_info_free (info);

		report_sorted ("folders-after-create", names);
		g_ptr_array_unref (names);

		/* `transfer_messages_to_sync` moves the appended message from
		 * the inbox into the new folder. Both folders are in this
		 * store, so `camel_folder_transfer_messages_to_sync` calls the
		 * vfunc directly. This does not test the cross-store path
		 * through `get_message`/`append_message` that the
		 * `transfer.rs` doc comment describes. `delete_originals=TRUE`
		 * makes it a move, so check both folders: the row must leave
		 * the inbox and arrive in "Receipts". */
		{
			CamelFolder *receipts;
			GPtrArray *transfer_uids;
			GPtrArray *transferred = NULL;

			receipts = camel_store_get_folder_sync (store, "Receipts",
								 CAMEL_STORE_FOLDER_NONE,
								 NULL, &error);
			if (!receipts)
				return functional_fail ("get-receipts-folder", error);

			transfer_uids = g_ptr_array_new ();
			g_ptr_array_add (transfer_uids, appended_uid);

			if (!camel_folder_transfer_messages_to_sync (inbox, transfer_uids,
								      receipts, TRUE,
								      &transferred, NULL, &error)) {
				g_ptr_array_unref (transfer_uids);
				g_object_unref (receipts);
				return functional_fail ("transfer-message", error);
			}
			g_ptr_array_unref (transfer_uids);

			/* RFC 8621 gives an `Email` one immutable id for each
			 * account. Thus the reported UID is the UID that went
			 * in, not a new one from the destination. The
			 * `Reported` doc in `transfer.rs` says the same. */
			g_print ("transfer-uid=%s\n",
				 (transferred && transferred->len > 0 && transferred->pdata[0])
				 ? (const gchar *) transferred->pdata[0] : "");
			if (transferred) {
				guint t;

				for (t = 0; t < transferred->len; t++)
					g_free (transferred->pdata[t]);
				g_ptr_array_unref (transferred);
			}

			if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
				g_object_unref (receipts);
				return functional_fail ("refresh-after-transfer-inbox", error);
			}
			uids = folder_dup_uids (inbox);
			g_print ("inbox-count-after-transfer=%u\n", uids->len);
			folder_free_uids (inbox, uids);

			if (!camel_folder_refresh_info_sync (receipts, NULL, &error)) {
				g_object_unref (receipts);
				return functional_fail ("refresh-after-transfer-receipts", error);
			}
			uids = folder_dup_uids (receipts);
			g_print ("receipts-count-after-transfer=%u\n", uids->len);
			folder_free_uids (receipts, uids);

			/* Move the message back. A JMAP server refuses to
			 * destroy a mailbox that holds a message
			 * (`mailboxHasEmail`), and the rename and delete below
			 * need an empty "Receipts". */
			transfer_uids = g_ptr_array_new ();
			g_ptr_array_add (transfer_uids, appended_uid);

			if (!camel_folder_transfer_messages_to_sync (receipts, transfer_uids,
								      inbox, TRUE,
								      &transferred, NULL, &error)) {
				g_ptr_array_unref (transfer_uids);
				g_object_unref (receipts);
				return functional_fail ("transfer-message-back", error);
			}
			g_ptr_array_unref (transfer_uids);
			if (transferred) {
				guint t;

				for (t = 0; t < transferred->len; t++)
					g_free (transferred->pdata[t]);
				g_ptr_array_unref (transferred);
			}

			if (!camel_folder_refresh_info_sync (receipts, NULL, &error)) {
				g_object_unref (receipts);
				return functional_fail ("refresh-after-transfer-back-receipts", error);
			}
			uids = folder_dup_uids (receipts);
			g_print ("receipts-count-after-transfer-back=%u\n", uids->len);
			folder_free_uids (receipts, uids);

			if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
				g_object_unref (receipts);
				return functional_fail ("refresh-after-transfer-back-inbox", error);
			}
			uids = folder_dup_uids (inbox);
			g_print ("inbox-count-after-transfer-back=%u\n", uids->len);
			folder_free_uids (inbox, uids);

			g_object_unref (receipts);
		}
		g_free (appended_uid);

		/* `rename_folder_sync` changes the last component of the new
		 * folder under the same root parent. The `manage.rs` module
		 * doc reads this as the name the user typed, not as a path
		 * encoding of the store. Check that the store listing shows
		 * "Invoices", not only the return value. */
		if (!camel_store_rename_folder_sync (store, "Receipts", "Invoices", NULL, &error))
			return functional_fail ("rename-folder", error);

		info = camel_store_get_folder_info_sync (store, NULL,
							 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
							 NULL, &error);
		if (!info)
			return functional_fail ("folder-info-after-rename", error);

		names = g_ptr_array_new_with_free_func (g_free);
		collect_folder_names (info, names);
		camel_folder_info_free (info);

		report_sorted ("folders-after-rename", names);
		g_ptr_array_unref (names);

		/* `delete_folder_sync` on the renamed folder. Check that the
		 * store listing no longer shows it, not only the return value. */
		if (!camel_store_delete_folder_sync (store, "Invoices", NULL, &error))
			return functional_fail ("delete-folder", error);

		info = camel_store_get_folder_info_sync (store, NULL,
							 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
							 NULL, &error);
		if (!info)
			return functional_fail ("folder-info-after-delete", error);

		names = g_ptr_array_new_with_free_func (g_free);
		collect_folder_names (info, names);
		camel_folder_info_free (info);

		report_sorted ("folders-after-delete", names);
		g_ptr_array_unref (names);
	}

	/* `expunge_sync` is "Expunge" or "Empty Trash". `synchronize_sync`
	 * calls it when its `expunge` argument is TRUE (above, that argument
	 * is FALSE). This block uses a different message from the one that
	 * `synchronize_sync` flagged. That message stays on the server, so the
	 * Rust harness can check its keyword. This one moves into the trash,
	 * and the harness checks that the trash is its only mailbox. It is
	 * still in the inbox, because the folder sequence above did not touch
	 * it. */
	if (!deleted_uid)
		return functional_fail ("no-message-to-delete", NULL);

	g_print ("deleted-uid=%s\n", deleted_uid);
	{
		CamelMessageInfo *delete_info;

		delete_info = camel_folder_get_message_info (inbox, deleted_uid);
		if (!delete_info) {
			g_free (deleted_uid);
			return functional_fail ("get-message-info-for-delete", NULL);
		}
		camel_message_info_set_flags (delete_info, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DELETED);
		g_clear_object (&delete_info);
	}

	if (!camel_folder_expunge_sync (inbox, NULL, &error)) {
		g_free (deleted_uid);
		return functional_fail ("expunge", error);
	}

	uids = folder_dup_uids (inbox);
	g_print ("inbox-count-after-expunge=%u\n", uids->len);
	{
		gboolean still_listed = FALSE;

		for (index = 0; index < uids->len; index++) {
			if (g_strcmp0 (uids->pdata[index], deleted_uid) == 0) {
				still_listed = TRUE;
				break;
			}
		}
		g_print ("expunged-uid-still-listed=%d\n", still_listed ? 1 : 0);
	}
	folder_free_uids (inbox, uids);
	g_free (flagged_uid);
	g_free (deleted_uid);

	g_object_unref (inbox);
	g_object_unref (session);
	g_object_unref (source);
	g_object_unref (registry);

	return status;
}
