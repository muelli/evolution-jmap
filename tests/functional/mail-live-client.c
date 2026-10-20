/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client of the mail tests against a real server. The other live-server
 * clients are the `write` phase of book-client.c, cal-live-client.c and
 * collection-client.c.
 *
 * `mail-client.c` beside this file is the client of the mock-based suite, and
 * it stays unchanged. One run of it subscribes and unsubscribes two named
 * mailboxes, flags a message, creates, renames and deletes a folder, moves a
 * message out of the inbox and back, and expunges another. It asserts nothing,
 * so each of these calls stops the program when a real server gives a
 * different answer. It also assumes the seed data of the mock: two messages in
 * the inbox, and mailboxes with the names "Sent", "Trash" and "Junk". A new
 * Stalwart account has none of these. Thus this file uses the approach of
 * cal-live-client.c (the simplest phase first, the larger phases after it),
 * and argv selects the phase:
 *
 *   - `receive` opens the store, lists the tree, gets the three special
 *     folders by role, appends one message and reads it back;
 *   - `flags` does the same steps without the reports, then writes the
 *     flags of the appended message, clears them, and expunges it;
 *   - `transfer` creates its own folder, moves the appended message into
 *     it, copies it back to the inbox, and then restores the account: it
 *     empties and deletes the folder and expunges the message;
 *   - `password-prompt` does not do the shared preamble. It opens the store
 *     of the account two times: first with no password (as for a new
 *     account), then with the real one. It checks with real Camel, not the
 *     mock, that the first attempt ends in the authentication-required
 *     state, where `test_session_authenticate_sync` calls
 *     `test_session_get_password`. See `run_password_prompt_phase`;
 *   - `scale` does only the connect. Then it measures the first open of a
 *     folder in Evolution, on an inbox that the harness filled: the open
 *     (the first sync), a delta refresh, and a summary read for each
 *     message, as the message list does next. It appends nothing, so the
 *     count comes only from the server. See `run_scale_phase`.
 *
 * One program, not many, because `receive` already creates the setup that
 * `flags` and `transfer` need: a connected store with one known message in the
 * inbox.
 *
 * ## Reading a write back
 *
 * Camel writes a flag into its own summary database immediately, also when the
 * server did not get it. Thus the `flags` phase asks a second `CamelStore` on
 * its own scratch tree (`report_reopened`). That store has no summary, so its
 * results come from the server.
 *
 * `rust/crates/jmap-functional/tests/live-stalwart-mail.rs` controls all other
 * parts: the scratch XDG tree, the `.source` keyfile, the private D-Bus
 * session, the proxy to the real server and all assertions. It runs this
 * program and reads its output. Thus this file has no test framework and no
 * definition of "correct". It writes on stdout one `key=value` line for each
 * observation, and exits with a non-zero status when a call fails.
 *
 * ## The password
 *
 * The password mechanism of the other live-server clients does not apply here.
 * They store the password on the `ESource` with
 * `e_source_store_password_sync`, because an EDS backend gets credentials from
 * EDS. A Camel provider never reads the secret store. It uses the password
 * that its `CamelSession` put on the `CamelService`
 * (`camel_service_get_password`, read in the jmap-mail `authenticate_sync`).
 * Only a session can put a password there. Thus this program does the two
 * things that the Evolution `EMailSession` does:
 *
 *   - it puts the password on the service before the connect, which is the
 *     state that `EMailSession` gets from the stored credential, and
 *   - it subclasses `CamelSession` to answer `get_password`, which its
 *     `authenticate_sync` calls when an attempt is REJECTED.
 *
 * It does both, because the output shows which of the two gave the password:
 * `password-prompts` is 0 when the server accepted the seeded password on the
 * first attempt. It is not zero when the server refused it one time and the
 * session gave it again. The mock checks no password, so it cannot show this.
 *
 *   usage: functional-mail-live-client <source-uid> <subject> <phase>
 */

#include <camel/camel.h>
#include <libedataserver/libedataserver.h>

/* The body of the message that this program appends to the inbox. One line, no
 * MIME structure: the test checks that a blob download from a real server
 * decodes to the uploaded text. */
#define TEST_BODY "Found on the floor."

/* The password from the harness, and the number of times the authenticate loop
 * asked for it. They are static because the process has one session, as for
 * the counters in mail-stale-token-client.c. */
static const gchar *seed_password = NULL;
static guint password_prompts = 0;

/* The one account of this program, and the two values that open a store on it.
 * They are static because the process has one of each. Arguments would add
 * three parameters to each helper that opens a second store. */
static ESource *account_source = NULL;
static const gchar *account_uid = NULL;
static const gchar *account_protocol = NULL;

/* The phase of one run (see the file header). All phases share the preamble (a
 * connected store and one known message appended to the inbox), except
 * PHASE_PASSWORD_PROMPT and PHASE_SCALE. PHASE_PASSWORD_PROMPT opens its own
 * store two times and returns before the preamble. PHASE_SCALE uses only the
 * connected store. */
typedef enum {
	PHASE_RECEIVE,
	PHASE_FLAGS,
	PHASE_TRANSFER,
	PHASE_PASSWORD_PROMPT,
	PHASE_SCALE
} MailPhase;

typedef struct _TestSession {
	CamelSession parent;
} TestSession;

typedef struct _TestSessionClass {
	CamelSessionClass parent_class;
} TestSessionClass;

GType test_session_get_type (void) G_GNUC_CONST;

G_DEFINE_TYPE (TestSession, test_session, CAMEL_TYPE_SESSION)

/* The `EMailSession` vfunc, with the environment value from the harness
 * instead of the credential lookup. Returns NULL and sets
 * `CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE` when the test seeded no password
 * (see the file header). */
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

/* `CamelSessionClass::authenticate_sync`, which
 * `camel_session_authenticate_sync` calls. Each provider calls that from its
 * `connect_sync` (see the module docs of `jmap-mail/src/service.rs`). The base
 * class has a default, but upstream `camel-session.c` calls it "a rough
 * example ... not intended to be used as is". On
 * `CAMEL_AUTHENTICATION_REJECTED`, it calls `camel_service_authenticate_sync`
 * again without end and never calls `get_password`. Here that is a busy loop,
 * not the credentials-required state that the `password-prompt` phase checks.
 * `EMailSession` replaces it with a full prompt loop. This is a minimal
 * replacement: one attempt, and on `REJECTED` one more attempt with the value
 * from `get_password`. For the first attempt of `PHASE_PASSWORD_PROMPT`, that
 * value is empty. */
static gboolean
test_session_authenticate_sync (CamelSession *session,
				CamelService *service,
				const gchar *mechanism,
				GCancellable *cancellable,
				GError **error)
{
	CamelAuthenticationResult result;

	result = camel_service_authenticate_sync (service, mechanism, cancellable, error);

	if (result == CAMEL_AUTHENTICATION_REJECTED) {
		GError *password_error = NULL;
		gchar *password;

		g_clear_error (error);

		password = test_session_get_password (session, service, NULL, NULL, 0, &password_error);
		if (!password) {
			g_propagate_error (error, password_error);
			return FALSE;
		}

		camel_service_set_password (service, password);
		g_free (password);

		result = camel_service_authenticate_sync (service, mechanism, cancellable, error);
	}

	return result == CAMEL_AUTHENTICATION_ACCEPTED;
}

static void
test_session_class_init (TestSessionClass *klass)
{
	CAMEL_SESSION_CLASS (klass)->get_password = test_session_get_password;
	CAMEL_SESSION_CLASS (klass)->authenticate_sync = test_session_authenticate_sync;
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

/* From 3.58, `camel_folder_dup_uids` replaces the borrowed
 * `camel_folder_get_uids`/`camel_folder_free_uids` pair. It returns a
 * reference-counted `GPtrArray` that the caller owns. This is a copy of the
 * mail-client.c wrappers, because this client links no crate from this
 * repository and cannot use the `eds-sys` compat wrappers. */
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

/* Flattens the folder tree into a list of full names. A copy of the
 * mail-client.c helper. */
static void
collect_folder_names (CamelFolderInfo *info,
		      GPtrArray *names)
{
	for (; info; info = info->next) {
		g_ptr_array_add (names, g_strdup (info->full_name));
		collect_folder_names (info->child, names);
	}
}

/* Writes one sorted `key=a,b,c` line, because the provider controls the folder
 * order. A copy of the mail-client.c helper: `values` stays sorted and the
 * caller keeps ownership. */
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
	g_ptr_array_remove_index (values, values->len - 1);
}

/* The decoded body text of a message, without the trailing newline of the MIME
 * transfer. mail-client.c does the same inline for each message in the inbox. */
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

/* The `scale` phase: the first open of a folder by the Evolution message list,
 * on an inbox that the harness filled before this process started.
 *
 * Three steps, timed separately because their costs are different. The first
 * sync occurs when the inbox opens: the jmap-mail `get_folder_sync` refreshes
 * a folder with no summary state, so it fetches each row and writes it into
 * the summary. The next `refresh_info_sync` is the delta refresh of each later
 * open, with nothing new on the server. The loop then reads one
 * `CamelMessageInfo` for each UID, as the message list does to show its rows.
 * `prefix` is the subject prefix of this run. The harness checks the count of
 * messages with this prefix, because a throwaway account can also hold mail
 * from other runs. */
static gboolean
run_scale_phase (CamelStore *store,
		 const gchar *prefix)
{
	GError *error = NULL;
	CamelFolder *inbox;
	GPtrArray *uids;
	gint64 started;
	guint index;
	guint matching = 0;

	started = g_get_monotonic_time ();
	inbox = camel_store_get_inbox_folder_sync (store, NULL, &error);
	if (!inbox) {
		fail ("inbox", error);
		return FALSE;
	}
	g_print ("open-ms=%" G_GINT64_FORMAT "\n",
		 (g_get_monotonic_time () - started) / 1000);

	started = g_get_monotonic_time ();
	if (!camel_folder_refresh_info_sync (inbox, NULL, &error)) {
		g_object_unref (inbox);
		fail ("refresh", error);
		return FALSE;
	}
	g_print ("refresh-ms=%" G_GINT64_FORMAT "\n",
		 (g_get_monotonic_time () - started) / 1000);

	started = g_get_monotonic_time ();
	uids = folder_dup_uids (inbox);
	for (index = 0; index < uids->len; index++) {
		CamelMessageInfo *message_info;
		const gchar *message_subject;

		message_info = camel_folder_get_message_info (inbox, uids->pdata[index]);
		if (!message_info) {
			g_printerr ("summary: no message info for listed uid '%s'\n",
				    (const gchar *) uids->pdata[index]);
			folder_free_uids (inbox, uids);
			g_object_unref (inbox);
			return FALSE;
		}

		message_subject = camel_message_info_get_subject (message_info);
		if (message_subject && g_str_has_prefix (message_subject, prefix))
			matching++;
		g_clear_object (&message_info);
	}
	g_print ("summary-ms=%" G_GINT64_FORMAT "\n",
		 (g_get_monotonic_time () - started) / 1000);
	g_print ("inbox-count=%u\n", uids->len);
	g_print ("inbox-matching=%u\n", matching);
	folder_free_uids (inbox, uids);

	/* Camel's queued `folder_changed` emissions refer to the folder, so
	 * process them while it exists (see the end of main). */
	while (g_main_context_iteration (NULL, FALSE))
		;
	g_object_unref (inbox);

	return TRUE;
}

/* A connected store on this account, with its summary database and message
 * cache in `data_dir`/`cache_dir`.
 *
 * The function also returns the session, because a `CamelService` has only a
 * weak reference to its session. If the caller released the session, the
 * service would lose it.
 *
 * Returns NULL with `error` set on failure, and releases all objects. */
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

	/* Copies the keyfile settings to the service: the host, the port, the
	 * user name and the security method that connect_sync reads. */
	e_source_camel_configure_service (account_source, service);

	/* `EMailSession` gets this state from the stored credential. Here, the
	 * harness gives it. See the file header. */
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

/* The fix for `StoreError::Unauthenticated` (in jmap-mail/src/connect.rs) had
 * tests only against the mock, which checks no password. This phase tests it
 * on a real server: first `open_store` with no password (as for a new account
 * before the user types one), then with the real password.
 *
 * The first attempt must fail, and it must fail with a password request. The
 * global `seed_password` is cleared first, so `test_session_get_password`
 * (this file's replacement for the EDS credentials-required prompt) answers
 * with no password. Stalwart opens an anonymous session with 200 OK. A
 * provider that handles this incorrectly finds no primary account and fails in
 * a different way, without a password request.
 * `unauthenticated-password-prompts` shows that.
 * `unauthenticated-connect-succeeded` shows an account that connected anyway.
 *
 * Both attempts use the same data and cache directories of the account,
 * intentionally: this is one real account at two times, not two independent
 * stores. */
static int
run_password_prompt_phase (const gchar *data_dir,
			   const gchar *cache_dir,
			   const gchar *real_password)
{
	GError *error = NULL;
	CamelSession *session = NULL;
	CamelStore *store;
	guint prompts_before;

	if (!real_password || !*real_password) {
		g_printerr ("password-prompt: JMAP_FUNCTIONAL_STORE_PASSWORD was not set\n");
		return 2;
	}

	seed_password = NULL;
	prompts_before = password_prompts;
	store = open_store (data_dir, cache_dir, &session, &error);

	g_print ("unauthenticated-connect-succeeded=%d\n", store ? 1 : 0);
	g_print ("unauthenticated-password-prompts=%u\n", password_prompts - prompts_before);

	if (store) {
		g_printerr ("password-prompt: an account with no stored password connected anyway\n");
		g_object_unref (store);
		g_object_unref (session);
		return 1;
	}

	g_print ("unauthenticated-error-domain=%s\n",
		 (error && error->domain == CAMEL_SERVICE_ERROR) ? "camel-service-error" : "other");
	g_print ("unauthenticated-error-code=%d\n", error ? error->code : -1);
	g_clear_error (&error);

	/* The same account, now with the password that a real
	 * credentials-required prompt would store. */
	seed_password = real_password;
	prompts_before = password_prompts;
	session = NULL;
	store = open_store (data_dir, cache_dir, &session, &error);
	if (!store)
		return fail ("authenticated-connect", error);

	g_print ("authenticated-connect-succeeded=1\n");
	g_print ("authenticated-password-prompts=%u\n", password_prompts - prompts_before);
	g_print ("store-connected=%d\n",
		 camel_service_get_connection_status (CAMEL_SERVICE (store)) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	g_object_unref (store);
	g_object_unref (session);

	return 0;
}

/* The view of one message from a store that has not seen this account before:
 * if the inbox holds it, and its read and important marks.
 *
 * The output keys are `<tag>-listed`, `<tag>-seen` and `<tag>-flagged`. The
 * last two show only when the message is there. The scratch tree belongs to
 * `tag`, so no earlier summary database can supply the values. Thus all three
 * values come from the real server. */
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

/* Sets `mask` to `set` on one message of `folder`, then sends the change with
 * `synchronize_sync`. This is the only call in this program that sends flag
 * changes, and a real server can answer it differently from the mock.
 *
 * `set` is a flags word, not a boolean, because clearing a mark is a different
 * `Email/set` patch from setting it. */
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

/* The read and important marks of one message in the current summary row, as
 * `seen-<tag>` and `flagged-<tag>`.
 *
 * This is weaker than report_reopened, but intentionally kept: this is the row
 * that the program wrote. Thus it shows the difference between a write that
 * did not reach the server and a write that Camel did not try. */
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

/* The two counts of the folder, as `inbox-count-<tag>` and
 * `inbox-summary-<tag>`.
 *
 * Two counts, because each can be wrong independently. The message list uses
 * the UID listing, and the total column of the folder tree uses
 * `camel_folder_summary_count`. A provider that removed a row from only one of
 * them shows a message in the folder that nobody can open. */
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

/* Transfers one message from `from` to `to`: a move when `delete_originals` is
 * TRUE, otherwise a copy. Writes the UID from the vfunc as `key`. RFC 8621
 * gives an Email one immutable id for each account, and a transfer changes
 * only its `mailboxIds`. Thus the UID must be the UID that went in, not a new
 * one from the destination. */
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

/* The UID listing of `folder` after a refresh: its length as `count-<tag>`,
 * and if it holds `uid`, as `holds-<tag>`. Because of the refresh, both values
 * come from the server, not from the summary rows of the last transfer. */
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

/* The view of the message location from a store that has not seen this account
 * before: if the inbox lists it, if `folder_name` lists it, and its subject in
 * the summary of `folder_name`. The output keys are `<tag>-inbox-listed`,
 * `<tag>-folder-listed` and `<tag>-subject`. The last shows only when the
 * folder holds the row. As in report_reopened, each tag has its own scratch
 * tree, so no earlier summary database can supply the values. */
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
	} else if (g_strcmp0 (phase, "password-prompt") == 0) {
		phase_id = PHASE_PASSWORD_PROMPT;
	} else if (g_strcmp0 (phase, "scale") == 0) {
		phase_id = PHASE_SCALE;
	} else {
		g_printerr ("%s: unknown phase '%s'\n", argv[0], phase);
		return 2;
	}

	/* The harness points these at the scratch tree. Camel keeps a summary
	 * database and a message cache for each service in them. The real XDG
	 * directories would put data into the developer's own Evolution store. */
	data_dir = g_get_user_data_dir ();
	cache_dir = g_get_user_cache_dir ();

	camel_init (data_dir, FALSE);
	camel_provider_init ();

	/* Registers the ESourceCamel subtype for the settings of each
	 * provider, so that the keyfile groups parse into a CamelSettings
	 * object. */
	e_source_camel_register_types ();

	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	account_source = e_source_registry_ref_source (registry, account_uid);
	if (!account_source) {
		g_printerr ("registry: no source with UID '%s'\n", account_uid);
		return 1;
	}

	/* Read the protocol from the source, not from a literal. See
	 * mail-client.c for the reason. */
	if (!e_source_has_extension (account_source, E_SOURCE_EXTENSION_MAIL_ACCOUNT)) {
		g_printerr ("source '%s' is not a mail account\n", account_uid);
		return 1;
	}

	backend_extension = e_source_get_extension (account_source, E_SOURCE_EXTENSION_MAIL_ACCOUNT);
	account_protocol = e_source_backend_get_backend_name (backend_extension);
	g_print ("protocol=%s\n", account_protocol ? account_protocol : "");

	if (phase_id == PHASE_PASSWORD_PROMPT)
		return run_password_prompt_phase (data_dir, cache_dir, seed_password);

	store = open_store (data_dir, cache_dir, &session, &error);
	if (!store)
		return fail ("connect", error);

	g_print ("store-connected=%d\n",
		 camel_service_get_connection_status (CAMEL_SERVICE (store)) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	/* 0 when the server accepted the seeded password on the first attempt.
	 * The harness decides if the value is correct. */
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

	if (phase_id == PHASE_SCALE) {
		gboolean scaled = run_scale_phase (store, subject);

		/* The same drain and teardown as at the end of this function. */
		while (g_main_context_iteration (NULL, FALSE))
			;

		g_object_unref (store);
		g_object_unref (session);
		g_object_unref (account_source);
		g_object_unref (registry);

		return scaled ? 0 : 1;
	}

	/* Get the three special folders by JMAP role, not by name. Stalwart
	 * gives the trash and junk roles the names "Deleted Items" and "Junk
	 * Mail", but the mock uses "Trash" and "Junk". A provider that finds a
	 * folder by name passes against the mock and fails here. */
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

	/* Report the count, do not assert zero. A throwaway account stays
	 * after the run that seeded it, so the test can only check that the
	 * listing increased by exactly one. */
	uids = folder_dup_uids (inbox);
	count_before = uids->len;
	folder_free_uids (inbox, uids);
	g_print ("inbox-count-before=%u\n", count_before);

	/* `append_message_sync` adds a message that Camel holds into the inbox
	 * of this account from outside it, with `Email/import` over an
	 * uploaded blob. Parse the message from raw RFC 5322 bytes, as
	 * mail-client.c does. Then Camel does the parse, and the write cannot
	 * disagree with it.
	 *
	 * The subject comes from the harness and is unique for each run. Thus
	 * a second run on the same throwaway account cannot read the message
	 * of a previous run as its own. */
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

	/* The folder listing writes the row, not the append. The message shows
	 * only after the next refresh. */
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
		/* Read the message back two times, because these are two
		 * different requests. The summary comes from Email/query and
		 * Email/get. The body comes from a blob download, which is a
		 * plain HTTP GET, not a method call. A provider that lists
		 * mail that it cannot open is a frequent failure. */
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
		/* The flags of the message after the append. The two writes
		 * below are measured as differences from these values. Thus
		 * the output shows any keywords that the server added to an
		 * imported message. */
		if (!report_local_flags (inbox, appended_uid, "after-append")) {
			g_free (appended_uid);
			g_printerr ("summary: no message info for the appended uid\n");
			return 1;
		}

		/* Read and important together, because they are one patch:
		 * `synchronize_sync` sends the full difference between the
		 * keywords of the last listing and the keywords of the row,
		 * not one request for each bit. */
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

		/* The other direction. A provider that only adds keywords
		 * passes the check above, but fails here. */
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

		/* `expunge_sync` does the deletion. CAMEL_MESSAGE_DELETED is a
		 * Camel flag (JMAP has no deleted keyword), so the mark sends
		 * nothing to the server. The expunge sends the request. */
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

		/* No refresh before this: the rows must go with the expunge,
		 * not at the next listing. A provider that keeps them until a
		 * later refresh must fail here. */
		report_counts (inbox, "after-expunge");

		listed = FALSE;
		uids = folder_dup_uids (inbox);
		for (index = 0; index < uids->len && !listed; index++)
			listed = g_strcmp0 (uids->pdata[index], appended_uid) == 0;
		folder_free_uids (inbox, uids);
		g_print ("expunged-still-listed=%d\n", listed ? 1 : 0);

		/* Ask the server the same question. The two counts above
		 * cannot answer it: they are also correct for a provider that
		 * removed the message only locally. */
		if (!report_reopened ("reopened-expunged", appended_uid, &error)) {
			g_free (appended_uid);
			return fail ("reopen-after-expunge", error);
		}
	} else {
		CamelFolderInfo *created;
		CamelFolder *scratch;
		CamelMessageInfo *filed_info;
		gchar *scratch_name;

		/* A folder for this run only, with the unique subject as its
		 * name. Thus a second run on the same throwaway account cannot
		 * open the folder of a previous run. A NULL parent is the
		 * account root, as in mail-client.c. */
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

		/* Count the folder, so that the transfers below start from a
		 * known empty folder. The inbox counts can only be
		 * differences, because the account stays after the run. */
		if (!report_refreshed_listing (scratch, appended_uid,
					       "scratch-before", &error)) {
			g_object_unref (scratch);
			return fail ("relist-scratch-before", error);
		}

		/* The move: `delete_originals=TRUE`, one `Email/set` that adds
		 * the destination `mailboxIds` member and removes the inbox
		 * member. Check both folders: the row must leave the inbox and
		 * arrive in the scratch folder. */
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

		/* The summary row of the destination. The refresh above wrote
		 * it, because a transfer only removes rows (see "What is not
		 * decided here" in `transfer.rs`). */
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
		 * adds a `mailboxIds` member. After it, one message is in two
		 * folders with one UID. */
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

		/* A third transfer type: a move into a folder that already
		 * holds the message. The patch adds a `mailboxIds` member that
		 * is already there and removes the scratch folder member. The
		 * inbox must then list the message once, not two times. */
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

		/* Now the folder is empty and can be deleted. A JMAP server
		 * refuses to destroy a mailbox that holds a message
		 * (`mailboxHasEmail`), so the cleanup move occurs first.
		 * Release the folder before the delete, as mail-client.c
		 * releases "Receipts" before the rename. */
		g_object_unref (scratch);

		if (!camel_store_delete_folder_sync (store, scratch_name,
						     NULL, &error))
			return fail ("delete-folder", error);

		/* The store listing must also show that the folder is gone,
		 * not only the return value. This is the point of manage.rs,
		 * here on a listing from the real server. */
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

		/* Restore the account: mark the message deleted and expunge
		 * it, the same sequence that the flags phase measures. */
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

	/* Camel adds its `folder_changed` emissions to the default main
	 * context, and this program runs no main loop, so some are still
	 * pending here. The dispose of `ESourceRegistry` iterates that
	 * context. Without this drain, the emissions run during the teardown
	 * below. Their closure unref then reaches a `CamelStore` whose
	 * finalizer already holds the folder bag lock that it needs, and the
	 * process stops (a hang on EDS 3.60.2, and a crash after this file
	 * added a second store). Process them here while all their objects
	 * exist, as a program with a main loop would do. */
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
