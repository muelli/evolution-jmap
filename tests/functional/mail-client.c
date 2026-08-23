/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the mail functional test: an ordinary libcamel
 * consumer, the way Evolution's mail view is one. It knows nothing about
 * JMAP and nothing about the mock server — it opens a store for a source
 * uid, lists the folder tree, opens the inbox and reads a message out of
 * it, which is the whole of what receiving mail is.
 *
 * The two halves either side of it are the same as the address book's:
 * `rust/crates/jmap-functional/tests/mail.rs` builds the scratch tree, the
 * `.source` keyfile, the private bus and the mock, and holds every opinion
 * about what the output should say. This program reports what Camel told
 * it, one `key=value` line per observation, and exits non-zero the moment a
 * call fails.
 *
 * What it does *not* share with book-client.c is the host process, and that
 * is why this test exists at all. An address book backend is dlopened by a
 * factory daemon EDS ships; a Camel provider is dlopened by the mail client
 * itself, out of a directory Camel finds by reading the `.urls` file beside
 * each object. So this program is that host: it is the process the provider
 * is loaded into, and reaching `store-connected=1` already proves the
 * `.urls` file, the protocol name and the entry point agree — three things
 * that live in three files and that no unit test can hold together, because
 * a unit test links the provider rather than letting Camel find it.
 *
 * A CamelSession is instantiated directly rather than subclassed. Evolution
 * subclasses it (EMailSession) to answer the vfuncs that need a user —
 * passwords, trust prompts, filters — and the source below deliberately
 * names no user, so nothing the base class's defaults do can matter. One of
 * them says so on stderr: the provider's connect_sync asks the session to
 * authenticate it, as every Camel provider does, and the default
 * authenticate_sync warns that it "is not intended for production use"
 * before doing the one round it takes for a service that needs no password.
 * That warning is expected output here, and is itself evidence the
 * authenticate path ran.
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

/* The folder tree, flattened into a list of full names. A tree is what the
 * store returns and a flat sorted list is what the test can hold to; the
 * nesting is not what this test is about — jmap-mail's own tests own the
 * shape of the tree. */
static void
collect_folder_names (CamelFolderInfo *info,
		      GPtrArray *names)
{
	for (; info; info = info->next) {
		g_ptr_array_add (names, g_strdup (info->full_name));
		collect_folder_names (info->child, names);
	}
}

/* One `key=a,b,c` line, sorted. Sorted because every list this program
 * reports is a set: the order Camel hands folders or message uids over is
 * the provider's business, and a test that compared it as given would be
 * asserting an order nobody promised. Consumes nothing; `values` is left
 * sorted and owned by the caller. */
static void
report_sorted (const gchar *key,
	       GPtrArray *values)
{
	gchar *joined;

	g_ptr_array_sort_values (values, (GCompareFunc) g_strcmp0);
	/* g_strjoinv wants a NULL-terminated vector, and the terminator has
	 * to go on after the sort so it is not sorted into the middle. */
	g_ptr_array_add (values, NULL);
	joined = g_strjoinv (",", (gchar **) values->pdata);
	g_print ("%s=%s\n", key, joined);
	g_free (joined);
	/* Leave the array as the caller handed it over. */
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
	CamelFolderInfo *info;
	CamelMimeMessage *message;
	GPtrArray *names;
	GPtrArray *uids;
	GPtrArray *subjects;
	GPtrArray *bodies;
	gchar *joined;
	gchar *appended_uid = NULL;
	gchar *flagged_uid = NULL;
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

	/* The scratch tree the harness built. Camel keeps a summary database
	 * and a message cache per service under these, and a session that
	 * fell back to the real XDG directories would write into the
	 * developer's own Evolution store. */
	data_dir = g_get_user_data_dir ();
	cache_dir = g_get_user_cache_dir ();

	/* Reads the `.urls` files out of EDS_CAMEL_PROVIDER_DIR, which the
	 * harness points at a directory holding this repository's provider
	 * and nothing else. Nothing is dlopened yet: a `.urls` file only
	 * tells Camel which protocols the object beside it claims. */
	camel_init (data_dir, FALSE);
	camel_provider_init ();

	/* Generates the ESourceCamel subtype each provider's settings live
	 * under, so that the `[JMAP Backend]` group in the keyfile parses
	 * into a CamelSettings object rather than being ignored. This is
	 * what Evolution calls, and skipping it gives a store that connects
	 * to whatever the settings defaults are — which for a host and port
	 * is nothing at all. */
	e_source_camel_register_types ();

	/* Activates evolution-source-registry on the private session bus,
	 * which reads the scratch sources directory the harness wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* The protocol comes off the source rather than being spelled here.
	 * `BackendName=jmap` in the keyfile, the protocol in
	 * libcameljmap.urls and the one camel_provider_module_init registers
	 * are three spellings that have to agree, and a program that
	 * hardcoded the middle one would agree with itself. */
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

	/* Where the provider is dlopened: Camel looks the protocol up in the
	 * table the `.urls` files built, and opens the object beside the file
	 * that claimed it. A protocol no file claims fails here, with "no
	 * provider available" — which is the failure mode the manual recipe
	 * warns about, arriving at exactly this call. */
	service = camel_session_add_service (session, source_uid, protocol,
					     CAMEL_PROVIDER_STORE, &error);
	if (!service)
		return fail ("add-service", error);

	/* Copies the keyfile's settings onto the service — the host, the port
	 * and the security method the provider reads in connect_sync. */
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

	/* The inbox by its role rather than by its name: Camel asks the store
	 * which folder is the inbox, and the provider answers from the
	 * mailbox's JMAP role. A store that got that wrong hands back a
	 * folder that merely happens to be called one. */
	inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
	if (!inbox)
		return fail ("inbox", error);

	g_print ("inbox-full-name=%s\n", camel_folder_get_full_name (inbox));

	if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
		return fail ("refresh", error);

	uids = camel_folder_get_uids (inbox);
	g_print ("inbox-count=%u\n", uids->len);

	/* Every message, twice over — once out of the summary and once as a
	 * whole message — because those are two different requests. The
	 * summaries come from Email/query and Email/get; a body comes from a
	 * blob download, which is a plain HTTP GET rather than a method call,
	 * and a provider that lists mail it cannot open is a common enough
	 * failure to be worth fetching every one of them.
	 *
	 * All of them rather than the first, because a folder's uid order is
	 * the provider's business and not this program's: picking one by
	 * position would make the test assert whatever order happened to come
	 * out. Each list is sorted, so the reported set is the set. */
	subjects = g_ptr_array_new_with_free_func (g_free);
	bodies = g_ptr_array_new_with_free_func (g_free);

	for (index = 0; index < uids->len; index++) {
		const gchar *uid = uids->pdata[index];
		CamelMessageInfo *message_info;
		GByteArray *body;
		CamelStream *stream;

		/* Whichever row Camel hands over first, held aside for the
		 * `synchronize_sync` block below — which one it is does not
		 * matter, since that block only reads back the same row it
		 * wrote a flag onto. */
		if (index == 0)
			flagged_uid = g_strdup (uid);

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
			camel_folder_free_uids (inbox, uids);
			return fail ("get-message", error);
		}

		body = g_byte_array_new ();
		stream = camel_stream_mem_new_with_byte_array (body);
		if (camel_data_wrapper_decode_to_stream_sync (
				camel_medium_get_content (CAMEL_MEDIUM (message)),
				stream, NULL, &error) < 0) {
			g_object_unref (stream);
			g_object_unref (message);
			camel_folder_free_uids (inbox, uids);
			return fail ("message-body", error);
		}

		/* The decoded bytes are not NUL-terminated, and the trailing
		 * newline the MIME body carries is the transfer's, not the
		 * text's. */
		joined = g_strndup ((const gchar *) body->data, body->len);
		g_ptr_array_add (bodies, g_strdup (g_strstrip (joined)));
		g_free (joined);

		/* The stream owns `body`. */
		g_object_unref (stream);
		g_object_unref (message);
	}

	camel_folder_free_uids (inbox, uids);

	report_sorted ("inbox-subjects", subjects);
	report_sorted ("message-bodies", bodies);
	g_ptr_array_unref (subjects);
	g_ptr_array_unref (bodies);

	/* `synchronize_sync`: the write half of what everything above only
	 * reads. Marking a message important is a local Camel flag until a
	 * synchronise carries it to the server as a keyword; nothing else in
	 * this program ever writes to a message's flags, so this is the only
	 * place that vfunc is reached. */
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

	/* The flag survives a fresh read of the row — which a synchronise
	 * that swallowed a write failure instead of reporting one would
	 * still show, so this alone does not prove the server was asked;
	 * the mock's own method log and stored keywords, checked from the
	 * Rust harness, are what prove that. */
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
	g_free (flagged_uid);

	/* `append_message_sync`: a message Camel is already holding — dragged
	 * out of another account, dropped as a `.eml`, saved by a filter —
	 * that this account has never seen, as opposed to `Email/set`'s
	 * transfer of a message the account already has. Parsed from raw RFC
	 * 5322 bytes rather than built header-by-header, the same way
	 * `jmap-mail`'s own append tests construct one: the parse on the way
	 * in has to be Camel's, or the write on the way out could disagree
	 * with it. */
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

		/* The row is the listing's to write, not the append's — the
		 * message appears only once the folder is next refreshed. */
		if (!camel_folder_refresh_info_sync (inbox, NULL, &error))
			return fail ("refresh-after-append", error);

		uids = camel_folder_get_uids (inbox);
		g_print ("inbox-count-after-append=%u\n", uids->len);

		reread = camel_folder_get_message_sync (inbox, appended_uid, NULL, &error);
		camel_folder_free_uids (inbox, uids);
		if (!reread)
			return fail ("get-appended-message", error);

		g_print ("appended-subject=%s\n", camel_mime_message_get_subject (reread));
		g_object_unref (reread);

		/* `appended_uid` stays alive: `transfer_messages_to_sync`,
		 * below, moves this same message once "Receipts" exists to
		 * move it into. */
	}

	/* "New Folder" at the account root: `create_folder_sync` on the live
	 * store, not the plain decision function `jmap-mail`'s own unit tests
	 * call directly. A NULL parent is the account itself, the same
	 * convention `manage.rs`'s own tests use. */
	{
		CamelFolderInfo *created;

		created = camel_store_create_folder_sync (store, NULL, "Receipts", NULL, &error);
		if (!created)
			return fail ("create-folder", error);

		g_print ("create-folder-name=%s\n", created->full_name);
		camel_folder_info_free (created);

		/* The property `manage.rs`'s own module doc names as the point of
		 * the vfunc: the store's own listing has to be in step with what
		 * it just told Camel it made, not merely return success. */
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

		/* `transfer_messages_to_sync`: dragging the message appended
		 * earlier out of the inbox and into the folder just created.
		 * `camel_folder_transfer_messages_to_sync` dispatches straight
		 * to the vfunc here because both folders belong to this one
		 * store — the cross-store path through `get_message`/
		 * `append_message` that `transfer.rs`'s own doc comment
		 * describes is not what this exercises.
		 * `delete_originals=TRUE`, a move: checked from both folders,
		 * not merely that the call answered — the row has to leave the
		 * inbox as well as land in "Receipts". */
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

			/* RFC 8621 gives an `Email` one immutable id per account, so
			 * the uid reported for the transferred message is the same
			 * uid that went in, not a fresh one the destination minted —
			 * `transfer.rs`'s own `Reported` doc makes the same point. */
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
			uids = camel_folder_get_uids (inbox);
			g_print ("inbox-count-after-transfer=%u\n", uids->len);
			camel_folder_free_uids (inbox, uids);

			if (!camel_folder_refresh_info_sync (receipts, NULL, &error)) {
				g_object_unref (receipts);
				return fail ("refresh-after-transfer-receipts", error);
			}
			uids = camel_folder_get_uids (receipts);
			g_print ("receipts-count-after-transfer=%u\n", uids->len);
			camel_folder_free_uids (receipts, uids);

			/* Moved back, the mirror image: a JMAP server refuses to
			 * destroy a mailbox that still holds a message
			 * (`mailboxHasEmail`), which the rename/delete sequence
			 * below exercises on an empty "Receipts" — so the message
			 * goes back to the inbox first, the same way a user who
			 * dropped a message into a folder would have to move it
			 * back out before deleting that folder. */
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
			uids = camel_folder_get_uids (receipts);
			g_print ("receipts-count-after-transfer-back=%u\n", uids->len);
			camel_folder_free_uids (receipts, uids);

			if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
				g_object_unref (receipts);
				return fail ("refresh-after-transfer-back-inbox", error);
			}
			uids = camel_folder_get_uids (inbox);
			g_print ("inbox-count-after-transfer-back=%u\n", uids->len);
			camel_folder_free_uids (inbox, uids);

			g_object_unref (receipts);
		}
		g_free (appended_uid);

		/* `rename_folder_sync` on the folder just made: a changed last
		 * component under the same (root) parent, which `manage.rs`'s own
		 * module doc reads as the name the user typed rather than this
		 * store's own path encoding of one. Checked the same way as create:
		 * not merely that the call answered, but that the store's own
		 * listing agrees the folder is now called "Invoices". */
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

		/* The mirror image: `delete_folder_sync` on the renamed folder,
		 * checked the same way — not merely that the call answered, but
		 * that the store's own listing agrees the folder is gone. */
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

	g_object_unref (inbox);
	g_object_unref (session);
	g_object_unref (source);
	g_object_unref (registry);

	return status;
}
