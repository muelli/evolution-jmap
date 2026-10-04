/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Item 82 batch 4: the send path against a real server, and the delivery the
 * mock structurally cannot answer for.
 *
 * transport-client.c beside this file is the mock leg: it walks the chain
 * from the account through the identity to the transport, sends once, and
 * the test then reads the submission out of the mock's own state. Against a
 * real Stalwart there is no state to reach into, and no need: the server
 * actually delivers. So this program walks the same chain with the same
 * accessors, sends the same way, and then does what the mock leg never
 * could -- it opens the sender's account afresh and finds the sent copy the
 * submission filed into Sent, and it opens a SECOND account, the one the
 * envelope named, and waits for the message itself to arrive in that inbox.
 *
 * It is not a phase of mail-live-client.c because it shares no preamble with
 * one: that program's setup is a connected store with a message appended to
 * the inbox, and a send starts from no store at all -- a `CamelTransport` is
 * a second `CamelService` out of the same provider's `object_types` table,
 * joined to the account only by two hops of uid indirection that Evolution
 * walks out of libedataserver, and walking them is half of what is tested.
 *
 * The stores this program does open are all for reading writes back, and
 * they follow mail-live-client.c's discipline: each on a scratch tree of its
 * own, so there is never a summary database of this process's own writing to
 * answer for the server. The password mechanism is mail-live-client.c's too
 * (seed on the service, subclass `CamelSession` for the REJECTED retry loop),
 * with one addition: there are two accounts here, so the session vfunc
 * answers whatever `current_password` was set to before the connect, and the
 * recipient's store is opened with the recipient's.
 *
 * Like every client in this directory it asserts nothing: one `key=value`
 * line per observation, exit non-zero the moment a call fails, and every
 * judgement in `rust/crates/jmap-functional/tests/live-stalwart-mail.rs`.
 *
 *   usage: functional-transport-live-client <account-source-uid>
 *          <recipient-source-uid> <recipient-address> <subject>
 *          <sent-folder-name> <drafts-folder-name>
 *
 * The two folder names are arguments because they are the harness's
 * constants: a `CamelStore` resolves inbox, trash and junk by role, but has
 * no by-role getter for sent or drafts, and a client that hardcoded a real
 * server's names would only agree with itself.
 */

#include <string.h>

#include <camel/camel.h>
#include <libedataserver/libedataserver.h>

/* The body of the message this program sends. One line, no MIME structure:
 * what is measured is whether the bytes Camel's emitter uploaded come back
 * out of a different account's blob download. Matched by `SEND_BODY` in the
 * test file. */
#define TEST_BODY "Posted from the corner box."

/* How long to wait for the server to move the message from the submission
 * queue into the recipient's inbox. Same budget as
 * jmap-mail-sync/tests/live_server_send.rs: 20 polls, a second apart. */
#define DELIVERY_POLLS 20

/* What the session subclass answers a REJECTED retry with. Two accounts
 * authenticate in this process, so main sets this before each connect; a
 * static because there is one session type and one question it answers. */
static const gchar *current_password = NULL;

typedef struct _TestSession {
	CamelSession parent;
} TestSession;

typedef struct _TestSessionClass {
	CamelSessionClass parent_class;
} TestSessionClass;

GType test_session_get_type (void) G_GNUC_CONST;

G_DEFINE_TYPE (TestSession, test_session, CAMEL_TYPE_SESSION)

/* mail-live-client.c's vfunc, answering for whichever account is currently
 * connecting -- see this file's header. */
static gchar *
test_session_get_password (CamelSession *session,
			   CamelService *service,
			   const gchar *prompt,
			   const gchar *item,
			   guint32 flags,
			   GError **error)
{
	if (!current_password || !*current_password) {
		g_set_error_literal (error, CAMEL_SERVICE_ERROR,
				     CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
				     "no password was seeded for this test");
		return NULL;
	}

	return g_strdup (current_password);
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
 * pair with `camel_folder_dup_uids`. Repeated here as plain C for the reason
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

/* One hop of the chain -- transport-client.c's helper, unchanged: a missing
 * source is not the same failure as a source that says nothing. */
static ESource *
ref_source (ESourceRegistry *registry,
	    const gchar *what,
	    const gchar *uid)
{
	ESource *source;

	if (!uid || !*uid) {
		g_printerr ("registry: the chain names no %s\n", what);
		return NULL;
	}

	source = e_source_registry_ref_source (registry, uid);
	if (!source)
		g_printerr ("registry: no %s source with UID '%s'\n", what, uid);

	return source;
}

/* A message's decoded body text, stripped of the trailing newline the MIME
 * transfer carries and the text does not -- mail-live-client.c's helper. */
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

/* A connected store on `source`, its summary database and message cache on a
 * scratch tree of `tag`'s own -- so what any folder of it reports came off
 * the server, never out of an earlier call's summary. The session comes back
 * too, for the reason mail-live-client.c gives: a `CamelService` only holds
 * its session weakly. */
static CamelStore *
open_account_store (ESource *source,
		    const gchar *uid,
		    const gchar *password,
		    const gchar *tag,
		    CamelSession **out_session,
		    GError **error)
{
	ESourceBackend *backend_extension;
	CamelSession *session;
	CamelService *service;
	const gchar *protocol;
	gchar *data_dir;
	gchar *cache_dir;

	if (!e_source_has_extension (source, E_SOURCE_EXTENSION_MAIL_ACCOUNT)) {
		g_set_error (error, CAMEL_ERROR, CAMEL_ERROR_GENERIC,
			     "source '%s' is not a mail account", uid);
		return NULL;
	}

	backend_extension = e_source_get_extension (source, E_SOURCE_EXTENSION_MAIL_ACCOUNT);
	protocol = e_source_backend_get_backend_name (backend_extension);

	data_dir = g_build_filename (g_get_user_data_dir (), "reopened", tag, NULL);
	cache_dir = g_build_filename (g_get_user_cache_dir (), "reopened", tag, NULL);
	g_mkdir_with_parents (data_dir, 0700);
	g_mkdir_with_parents (cache_dir, 0700);

	session = g_object_new (test_session_get_type (),
				"user-data-dir", data_dir,
				"user-cache-dir", cache_dir,
				NULL);
	g_free (data_dir);
	g_free (cache_dir);

	service = camel_session_add_service (session, uid, protocol,
					     CAMEL_PROVIDER_STORE, error);
	if (!service) {
		g_object_unref (session);
		return NULL;
	}

	e_source_camel_configure_service (source, service);

	current_password = password;
	if (password && *password)
		camel_service_set_password (service, password);

	if (!camel_service_connect_sync (service, NULL, error)) {
		g_object_unref (service);
		g_object_unref (session);
		return NULL;
	}

	*out_session = session;

	return CAMEL_STORE (service);
}

/* The uid of the one message in `folder` whose subject is `subject`, or NULL.
 * By subject rather than by uid because the messages this program looks for
 * were minted by a send, which hands no uid back: the subject is the run's
 * own nonce, so it identifies the message as surely as a uid would. */
static gchar *
find_by_subject (CamelFolder *folder,
		 const gchar *subject)
{
	GPtrArray *uids;
	gchar *found = NULL;
	guint index;

	uids = folder_dup_uids (folder);
	for (index = 0; index < uids->len && !found; index++) {
		CamelMessageInfo *info;

		info = camel_folder_get_message_info (folder, uids->pdata[index]);
		if (!info)
			continue;

		if (g_strcmp0 (camel_message_info_get_subject (info), subject) == 0)
			found = g_strdup (uids->pdata[index]);

		g_clear_object (&info);
	}
	folder_free_uids (folder, uids);

	return found;
}

/* Marks `uid` deleted and expunges it -- the cleanup both accounts get, the
 * same sequence mail-live-client.c's phases end with. */
static gboolean
delete_and_expunge (CamelFolder *folder,
		    const gchar *uid,
		    GError **error)
{
	CamelMessageInfo *info;

	info = camel_folder_get_message_info (folder, uid);
	if (!info) {
		g_set_error_literal (error, CAMEL_ERROR, CAMEL_ERROR_GENERIC,
				     "the summary has no row for the message to expunge");
		return FALSE;
	}

	camel_message_info_set_flags (info, CAMEL_MESSAGE_DELETED,
				      CAMEL_MESSAGE_DELETED);
	g_clear_object (&info);

	return camel_folder_expunge_sync (folder, NULL, error);
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *account = NULL;
	ESource *identity = NULL;
	ESource *transport_source = NULL;
	ESource *recipient_account = NULL;
	ESourceMailAccount *account_extension;
	ESourceMailIdentity *identity_extension;
	ESourceMailSubmission *submission_extension;
	ESourceBackend *backend_extension;
	CamelSession *session = NULL;
	CamelSession *sender_session = NULL;
	CamelSession *recipient_session = NULL;
	CamelService *service = NULL;
	CamelStore *sender_store;
	CamelStore *recipient_store;
	CamelFolder *sent_folder;
	CamelFolder *drafts_folder;
	CamelFolder *recipient_inbox;
	CamelMimeMessage *message;
	CamelInternetAddress *from;
	CamelInternetAddress *recipients;
	const gchar *account_uid;
	const gchar *recipient_uid;
	const gchar *recipient_address;
	const gchar *subject;
	const gchar *sent_name;
	const gchar *drafts_name;
	const gchar *identity_uid;
	const gchar *transport_uid;
	const gchar *sender_name;
	const gchar *sender_address;
	const gchar *protocol;
	const gchar *sender_password;
	const gchar *recipient_password;
	const gchar *data_dir;
	const gchar *cache_dir;
	gchar *sent_copy_uid = NULL;
	gchar *staged_uid = NULL;
	gchar *delivered_uid = NULL;
	gchar *recheck;
	gboolean sent_copy_saved = FALSE;
	guint poll;
	int status = 1;

	if (argc != 7) {
		g_printerr ("usage: %s <account-source-uid> <recipient-source-uid> "
			    "<recipient-address> <subject> <sent-folder-name> "
			    "<drafts-folder-name>\n", argv[0]);
		return 2;
	}

	account_uid = argv[1];
	recipient_uid = argv[2];
	recipient_address = argv[3];
	subject = argv[4];
	sent_name = argv[5];
	drafts_name = argv[6];
	sender_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");
	recipient_password = g_getenv ("JMAP_FUNCTIONAL_RECIPIENT_PASSWORD");

	/* The scratch tree the harness built; a session that fell back to the
	 * real XDG directories would write into the developer's own store. */
	data_dir = g_get_user_data_dir ();
	cache_dir = g_get_user_cache_dir ();

	camel_init (data_dir, FALSE);
	camel_provider_init ();
	e_source_camel_register_types ();

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	account = ref_source (registry, "account", account_uid);
	if (!account)
		goto out;

	if (!e_source_has_extension (account, E_SOURCE_EXTENSION_MAIL_ACCOUNT)) {
		g_printerr ("source '%s' is not a mail account\n", account_uid);
		goto out;
	}

	/* The chain, hop for hop what transport-client.c walks against the
	 * mock -- see that file for why each link hangs where it does. */
	account_extension = e_source_get_extension (account, E_SOURCE_EXTENSION_MAIL_ACCOUNT);
	identity_uid = e_source_mail_account_get_identity_uid (account_extension);
	g_print ("identity-uid=%s\n", identity_uid ? identity_uid : "");

	identity = ref_source (registry, "identity", identity_uid);
	if (!identity)
		goto out;

	if (!e_source_has_extension (identity, E_SOURCE_EXTENSION_MAIL_IDENTITY)) {
		g_printerr ("source '%s' is not a mail identity\n", identity_uid);
		goto out;
	}

	identity_extension = e_source_get_extension (identity, E_SOURCE_EXTENSION_MAIL_IDENTITY);
	sender_name = e_source_mail_identity_get_name (identity_extension);
	sender_address = e_source_mail_identity_get_address (identity_extension);
	g_print ("identity-address=%s\n", sender_address ? sender_address : "");

	if (!sender_address || !*sender_address) {
		g_printerr ("identity '%s' carries no address to send from\n", identity_uid);
		goto out;
	}

	if (!e_source_has_extension (identity, E_SOURCE_EXTENSION_MAIL_SUBMISSION)) {
		g_printerr ("identity '%s' has no submission extension\n", identity_uid);
		goto out;
	}

	submission_extension = e_source_get_extension (identity, E_SOURCE_EXTENSION_MAIL_SUBMISSION);
	transport_uid = e_source_mail_submission_get_transport_uid (submission_extension);
	g_print ("transport-uid=%s\n", transport_uid ? transport_uid : "");

	transport_source = ref_source (registry, "transport", transport_uid);
	if (!transport_source)
		goto out;

	if (!e_source_has_extension (transport_source, E_SOURCE_EXTENSION_MAIL_TRANSPORT)) {
		g_printerr ("source '%s' is not a mail transport\n", transport_uid);
		goto out;
	}

	backend_extension = e_source_get_extension (transport_source, E_SOURCE_EXTENSION_MAIL_TRANSPORT);
	protocol = e_source_backend_get_backend_name (backend_extension);
	g_print ("protocol=%s\n", protocol ? protocol : "");

	/* The one source the mock leg has no counterpart for: the account the
	 * envelope names, resolved up front so a typo in the harness fails
	 * here rather than as a delivery that never comes. */
	recipient_account = ref_source (registry, "recipient account", recipient_uid);
	if (!recipient_account)
		goto out;

	session = g_object_new (test_session_get_type (),
				"user-data-dir", data_dir,
				"user-cache-dir", cache_dir,
				NULL);

	service = camel_session_add_service (session, transport_uid, protocol,
					     CAMEL_PROVIDER_TRANSPORT, &error);
	if (!service) {
		status = fail ("add-service", error);
		goto out;
	}

	e_source_camel_configure_service (transport_source, service);

	/* The state EMailSession reaches by looking the stored credential up,
	 * reached here by being told it -- mail-live-client.c's mechanism,
	 * now on the transport slot's service. */
	current_password = sender_password;
	if (sender_password && *sender_password)
		camel_service_set_password (service, sender_password);

	if (!camel_service_connect_sync (service, NULL, &error)) {
		status = fail ("connect", error);
		goto out;
	}

	g_print ("transport-connected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	/* What the composer would have handed over, built the way
	 * transport-client.c builds it: the envelope passed separately from
	 * the message, because the envelope is what the message is delivered
	 * by and Evolution fills it in from the account and the composer's
	 * recipient fields rather than from the headers. */
	message = camel_mime_message_new ();
	camel_mime_message_set_subject (message, subject);

	from = camel_internet_address_new ();
	camel_internet_address_add (from, sender_name, sender_address);
	camel_mime_message_set_from (message, from);

	recipients = camel_internet_address_new ();
	camel_internet_address_add (recipients, NULL, recipient_address);
	camel_mime_message_set_recipients (message, CAMEL_RECIPIENT_TYPE_TO, recipients);

	camel_mime_part_set_content (CAMEL_MIME_PART (message), TEST_BODY,
				     strlen (TEST_BODY),
				     "text/plain; charset=UTF-8");

	if (!camel_transport_send_to_sync (CAMEL_TRANSPORT (service), message,
					   CAMEL_ADDRESS (from), CAMEL_ADDRESS (recipients),
					   &sent_copy_saved, NULL, &error)) {
		g_object_unref (recipients);
		g_object_unref (from);
		g_object_unref (message);
		status = fail ("send", error);
		goto out;
	}

	g_print ("sent=1\n");
	g_print ("sent-copy-saved=%d\n", sent_copy_saved ? 1 : 0);

	g_object_unref (recipients);
	g_object_unref (from);
	g_object_unref (message);

	if (!camel_service_disconnect_sync (service, TRUE, NULL, &error)) {
		status = fail ("disconnect", error);
		goto out;
	}

	g_print ("transport-disconnected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_DISCONNECTED ? 1 : 0);

	/* The sent copy, asked of a store that has never seen this account:
	 * in the sent folder the submission's own `onSuccessUpdateEmail`
	 * filed it into, no longer carrying the draft mark the staging gave
	 * it, and not still sitting in the drafts folder it was staged in. */
	sender_store = open_account_store (account, account_uid, sender_password,
					   "sender", &sender_session, &error);
	if (!sender_store) {
		status = fail ("reopen-sender", error);
		goto out;
	}

	sent_folder = camel_store_get_folder_sync (sender_store, sent_name,
						   CAMEL_STORE_FOLDER_NONE,
						   NULL, &error);
	if (!sent_folder || !camel_folder_refresh_info_sync (sent_folder, NULL, &error)) {
		g_clear_object (&sent_folder);
		g_object_unref (sender_store);
		g_object_unref (sender_session);
		status = fail ("sent-folder", error);
		goto out;
	}

	sent_copy_uid = find_by_subject (sent_folder, subject);
	g_print ("sent-copy-listed=%d\n", sent_copy_uid ? 1 : 0);

	if (sent_copy_uid) {
		CamelMessageInfo *info;

		info = camel_folder_get_message_info (sent_folder, sent_copy_uid);
		if (info) {
			g_print ("sent-copy-draft=%d\n",
				 (camel_message_info_get_flags (info) & CAMEL_MESSAGE_DRAFT) ? 1 : 0);
			g_clear_object (&info);
		}
	}

	drafts_folder = camel_store_get_folder_sync (sender_store, drafts_name,
						     CAMEL_STORE_FOLDER_NONE,
						     NULL, &error);
	if (!drafts_folder || !camel_folder_refresh_info_sync (drafts_folder, NULL, &error)) {
		g_clear_object (&drafts_folder);
		g_object_unref (sent_folder);
		g_object_unref (sender_store);
		g_object_unref (sender_session);
		status = fail ("drafts-folder", error);
		goto out;
	}

	staged_uid = find_by_subject (drafts_folder, subject);
	g_print ("staged-copy-listed=%d\n", staged_uid ? 1 : 0);

	/* Leave the sender where the run found it -- and measure the expunge
	 * against a relisting, the way every other cleanup in this directory
	 * is. A copy unexpectedly still in drafts is cleaned too: the
	 * observation above already recorded it. */
	if (staged_uid &&
	    !delete_and_expunge (drafts_folder, staged_uid, &error)) {
		g_object_unref (drafts_folder);
		g_object_unref (sent_folder);
		g_object_unref (sender_store);
		g_object_unref (sender_session);
		status = fail ("cleanup-drafts", error);
		goto out;
	}
	g_object_unref (drafts_folder);

	if (sent_copy_uid) {
		if (!delete_and_expunge (sent_folder, sent_copy_uid, &error)) {
			g_object_unref (sent_folder);
			g_object_unref (sender_store);
			g_object_unref (sender_session);
			status = fail ("cleanup-sent", error);
			goto out;
		}

		if (!camel_folder_refresh_info_sync (sent_folder, NULL, &error)) {
			g_object_unref (sent_folder);
			g_object_unref (sender_store);
			g_object_unref (sender_session);
			status = fail ("relist-sent", error);
			goto out;
		}

		recheck = find_by_subject (sent_folder, subject);
		g_print ("sender-cleaned=%d\n", recheck ? 0 : 1);
		g_free (recheck);
	}

	g_object_unref (sent_folder);
	g_object_unref (sender_store);
	g_object_unref (sender_session);

	/* The other end: the account the envelope named, opened as itself.
	 * The server moves the message from the submission queue to this
	 * inbox on its own schedule, so the listing is polled rather than
	 * trusted to be there already -- same budget as
	 * live_server_send.rs. */
	recipient_store = open_account_store (recipient_account, recipient_uid,
					      recipient_password, "recipient",
					      &recipient_session, &error);
	if (!recipient_store) {
		status = fail ("reopen-recipient", error);
		goto out;
	}

	recipient_inbox = camel_store_get_inbox_folder_sync (recipient_store, NULL, &error);
	if (!recipient_inbox) {
		g_object_unref (recipient_store);
		g_object_unref (recipient_session);
		status = fail ("recipient-inbox", error);
		goto out;
	}

	for (poll = 0; poll < DELIVERY_POLLS && !delivered_uid; poll++) {
		if (poll > 0)
			g_usleep (G_USEC_PER_SEC);

		if (!camel_folder_refresh_info_sync (recipient_inbox, NULL, &error)) {
			g_object_unref (recipient_inbox);
			g_object_unref (recipient_store);
			g_object_unref (recipient_session);
			status = fail ("refresh-recipient-inbox", error);
			goto out;
		}

		delivered_uid = find_by_subject (recipient_inbox, subject);
	}

	g_print ("delivered-listed=%d\n", delivered_uid ? 1 : 0);

	if (delivered_uid) {
		CamelMessageInfo *info;
		CamelMimeMessage *delivered;
		gchar *body;

		info = camel_folder_get_message_info (recipient_inbox, delivered_uid);
		if (info) {
			g_print ("delivered-subject=%s\n", camel_message_info_get_subject (info));
			g_print ("delivered-from=%s\n", camel_message_info_get_from (info));
			g_clear_object (&info);
		}

		delivered = camel_folder_get_message_sync (recipient_inbox, delivered_uid,
							   NULL, &error);
		if (!delivered) {
			g_object_unref (recipient_inbox);
			g_object_unref (recipient_store);
			g_object_unref (recipient_session);
			status = fail ("get-delivered-message", error);
			goto out;
		}

		body = message_body (delivered, &error);
		g_object_unref (delivered);
		if (!body) {
			g_object_unref (recipient_inbox);
			g_object_unref (recipient_store);
			g_object_unref (recipient_session);
			status = fail ("delivered-body", error);
			goto out;
		}

		g_print ("delivered-body=%s\n", body);
		g_free (body);

		/* And leave the recipient where the run found it too. */
		if (!delete_and_expunge (recipient_inbox, delivered_uid, &error)) {
			g_object_unref (recipient_inbox);
			g_object_unref (recipient_store);
			g_object_unref (recipient_session);
			status = fail ("cleanup-recipient", error);
			goto out;
		}

		if (!camel_folder_refresh_info_sync (recipient_inbox, NULL, &error)) {
			g_object_unref (recipient_inbox);
			g_object_unref (recipient_store);
			g_object_unref (recipient_session);
			status = fail ("relist-recipient", error);
			goto out;
		}

		recheck = find_by_subject (recipient_inbox, subject);
		g_print ("recipient-cleaned=%d\n", recheck ? 0 : 1);
		g_free (recheck);
	}

	g_object_unref (recipient_inbox);
	g_object_unref (recipient_store);
	g_object_unref (recipient_session);

	status = 0;

out:
	/* Camel schedules its own `folder_changed` emissions onto the default
	 * main context, and this program runs no main loop -- drain them
	 * while everything they refer to is still alive, for the reason
	 * mail-live-client.c records at its own teardown. */
	while (g_main_context_iteration (NULL, FALSE))
		;

	g_free (delivered_uid);
	g_free (staged_uid);
	g_free (sent_copy_uid);
	g_clear_object (&service);
	g_clear_object (&session);
	g_clear_object (&recipient_account);
	g_clear_object (&transport_source);
	g_clear_object (&identity);
	g_clear_object (&account);
	g_clear_object (&registry);

	return status;
}
