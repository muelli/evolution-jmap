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

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

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

	if (argc != 2) {
		g_printerr ("usage: %s <source-uid>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];

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
		return fail ("registry", error);

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

	/* Camel dlopens the provider here. It finds the protocol in the table
	 * from the `.urls` files and opens the object beside the file that
	 * claims it. An unclaimed protocol fails here with "no provider
	 * available". */
	service = camel_session_add_service (session, source_uid, protocol,
					     CAMEL_PROVIDER_STORE, &error);
	if (!service)
		return fail ("add-service", error);

	/* Copies the keyfile settings to the service: the host, the port and
	 * the security method that connect_sync reads. */
	e_source_camel_configure_service (source, service);

	if (!camel_service_connect_sync (service, NULL, &error))
		return fail ("connect", error);

	g_print ("store-connected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

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

	/* `CamelSubscribable` controls the tick beside a folder in the
	 * subscription editor. The two folders change in opposite directions,
	 * so no write sets a value that is already there. "Sent" starts
	 * subscribed, as all mailboxes from the mock do, and this block
	 * unsubscribes it. "Drafts" starts unsubscribed (the Rust harness
	 * seeds it so), and this block subscribes it. `folder_is_subscribed`
	 * is a non-blocking method with no cancellable or error, so it needs
	 * no fail() check. */
	{
		CamelSubscribable *subscribable = CAMEL_SUBSCRIBABLE (store);

		g_print ("sent-subscribed-initially=%d\n",
			 camel_subscribable_folder_is_subscribed (subscribable, "Sent") ? 1 : 0);
		if (!camel_subscribable_unsubscribe_folder_sync (subscribable, "Sent", NULL, &error))
			return fail ("unsubscribe-sent", error);
		g_print ("sent-subscribed-after-unsubscribe=%d\n",
			 camel_subscribable_folder_is_subscribed (subscribable, "Sent") ? 1 : 0);

		g_print ("drafts-subscribed-initially=%d\n",
			 camel_subscribable_folder_is_subscribed (subscribable, "Drafts") ? 1 : 0);
		if (!camel_subscribable_subscribe_folder_sync (subscribable, "Drafts", NULL, &error))
			return fail ("subscribe-drafts", error);
		g_print ("drafts-subscribed-after-subscribe=%d\n",
			 camel_subscribable_folder_is_subscribed (subscribable, "Drafts") ? 1 : 0);
	}

	/* Get the inbox by role, not by name. The provider answers from the
	 * JMAP role of the mailbox. A wrong answer gives a folder that only
	 * has the name of an inbox. */
	inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
	if (!inbox)
		return fail ("inbox", error);

	g_print ("inbox-full-name=%s\n", camel_folder_get_full_name (inbox));

	/* Trash and junk use the same lookup by role, not the inherited vTrash
	 * and vJunk virtual folders. */
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
		GByteArray *body;
		CamelStream *stream;

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
			return fail ("get-message", error);
		}

		body = g_byte_array_new ();
		stream = camel_stream_mem_new_with_byte_array (body);
		if (camel_data_wrapper_decode_to_stream_sync (
				camel_medium_get_content (CAMEL_MEDIUM (message)),
				stream, NULL, &error) < 0) {
			g_object_unref (stream);
			g_object_unref (message);
			folder_free_uids (inbox, uids);
			return fail ("message-body", error);
		}

		/* The decoded bytes have no NUL terminator. The trailing
		 * newline is part of the MIME transfer, not of the text. */
		joined = g_strndup ((const gchar *) body->data, body->len);
		g_ptr_array_add (bodies, g_strdup (g_strstrip (joined)));
		g_free (joined);

		/* The stream owns `body`. */
		g_object_unref (stream);
		g_object_unref (message);
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
		return fail ("no-message-to-flag", NULL);

	g_print ("flagged-uid=%s\n", flagged_uid);
	{
		CamelMessageInfo *flag_info;

		flag_info = camel_folder_get_message_info (inbox, flagged_uid);
		if (!flag_info) {
			g_free (flagged_uid);
			return fail ("get-message-info-for-flag", NULL);
		}
		camel_message_info_set_flags (flag_info, CAMEL_MESSAGE_FLAGGED, CAMEL_MESSAGE_FLAGGED);
		g_clear_object (&flag_info);
	}

	if (!camel_folder_synchronize_sync (inbox, FALSE, NULL, &error)) {
		g_free (flagged_uid);
		return fail ("synchronize", error);
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
			return fail ("get-message-info-after-sync", NULL);
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
			return fail ("parse-outside-message", error);
		}

		if (!camel_folder_append_message_sync (inbox, outside, NULL,
							&appended_uid, NULL, &error)) {
			g_object_unref (outside);
			return fail ("append-message", error);
		}
		g_object_unref (outside);

		g_print ("append-uid=%s\n", appended_uid ? appended_uid : "");

		/* The folder listing writes the row, not the append. The
		 * message shows only after the next refresh. */
		if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
			return fail ("refresh-after-append", error);

		uids = folder_dup_uids (inbox);
		g_print ("inbox-count-after-append=%u\n", uids->len);

		reread = camel_folder_get_message_sync (inbox, appended_uid, NULL, &error);
		folder_free_uids (inbox, uids);
		if (!reread)
			return fail ("get-appended-message", error);

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
			return fail ("create-folder", error);

		g_print ("create-folder-name=%s\n", created->full_name);
		camel_folder_info_free (created);

		/* The `manage.rs` module doc gives the purpose of the vfunc:
		 * the store listing must agree with what the store told Camel
		 * it made. A success return alone is not sufficient. */
		info = camel_store_get_folder_info_sync (store, NULL,
							 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
							 NULL, &error);
		if (!info)
			return fail ("folder-info-after-create", error);

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
				return fail ("get-receipts-folder", error);

			transfer_uids = g_ptr_array_new ();
			g_ptr_array_add (transfer_uids, appended_uid);

			if (!camel_folder_transfer_messages_to_sync (inbox, transfer_uids,
								      receipts, TRUE,
								      &transferred, NULL, &error)) {
				g_ptr_array_unref (transfer_uids);
				g_object_unref (receipts);
				return fail ("transfer-message", error);
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
				return fail ("refresh-after-transfer-inbox", error);
			}
			uids = folder_dup_uids (inbox);
			g_print ("inbox-count-after-transfer=%u\n", uids->len);
			folder_free_uids (inbox, uids);

			if (!camel_folder_refresh_info_sync (receipts, NULL, &error)) {
				g_object_unref (receipts);
				return fail ("refresh-after-transfer-receipts", error);
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
				return fail ("transfer-message-back", error);
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
				return fail ("refresh-after-transfer-back-receipts", error);
			}
			uids = folder_dup_uids (receipts);
			g_print ("receipts-count-after-transfer-back=%u\n", uids->len);
			folder_free_uids (receipts, uids);

			if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
				g_object_unref (receipts);
				return fail ("refresh-after-transfer-back-inbox", error);
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
			return fail ("rename-folder", error);

		info = camel_store_get_folder_info_sync (store, NULL,
							 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
							 NULL, &error);
		if (!info)
			return fail ("folder-info-after-rename", error);

		names = g_ptr_array_new_with_free_func (g_free);
		collect_folder_names (info, names);
		camel_folder_info_free (info);

		report_sorted ("folders-after-rename", names);
		g_ptr_array_unref (names);

		/* `delete_folder_sync` on the renamed folder. Check that the
		 * store listing no longer shows it, not only the return value. */
		if (!camel_store_delete_folder_sync (store, "Invoices", NULL, &error))
			return fail ("delete-folder", error);

		info = camel_store_get_folder_info_sync (store, NULL,
							 CAMEL_STORE_FOLDER_INFO_RECURSIVE,
							 NULL, &error);
		if (!info)
			return fail ("folder-info-after-delete", error);

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
		return fail ("no-message-to-delete", NULL);

	g_print ("deleted-uid=%s\n", deleted_uid);
	{
		CamelMessageInfo *delete_info;

		delete_info = camel_folder_get_message_info (inbox, deleted_uid);
		if (!delete_info) {
			g_free (deleted_uid);
			return fail ("get-message-info-for-delete", NULL);
		}
		camel_message_info_set_flags (delete_info, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DELETED);
		g_clear_object (&delete_info);
	}

	if (!camel_folder_expunge_sync (inbox, NULL, &error)) {
		g_free (deleted_uid);
		return fail ("expunge", error);
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
