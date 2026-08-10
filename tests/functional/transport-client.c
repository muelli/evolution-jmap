/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the transport functional test: the send path, walked from
 * the account the way Evolution walks it.
 *
 * mail-client.c is the receiving half and opens a store. This program never
 * opens one. A `CamelTransport` is a second `CamelService`, instantiated from a
 * second `ESource` out of the same provider's `object_types` table, and the
 * only thing that joins it to the account is two hops of uid indirection
 * through a third source:
 *
 *   [Mail Account] IdentityUid  -> the identity source
 *   [Mail Identity] / [Mail Submission] TransportUid -> the transport source
 *
 * Camel does not walk that chain — it knows nothing about ESource. Evolution
 * walks it, in evolution-mail, out of libedataserver accessors, and this
 * program walks the same one with the same accessors. It is handed the
 * *account* uid and finds the rest, which is the thing being tested: every link
 * is a string in a file that nothing else in this repository can hold to the
 * file it names.
 *
 * The message it sends is built from the arguments rather than from constants
 * of its own, so that what the test asserts and what goes on the wire are one
 * string. Everything else follows mail-client.c: one `key=value` line per
 * observation, exit non-zero the moment a call fails, and no opinion about what
 * any of it should say — `rust/crates/jmap-functional/tests/transport.rs` holds
 * those.
 *
 * The CamelSession is instantiated directly, with mail-client.c's caveat in
 * full: the base class's authenticate_sync warns on stderr that it "is not
 * intended for production use", the provider's connect_sync asks for it as
 * every Camel provider does, and the source names no user for it to resolve.
 * That warning is expected output.
 *
 *   usage: functional-transport-client <account-source-uid> <recipient> <subject> <body>
 */

#include <string.h>

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

/* One hop of the chain: the source `uid` names, or NULL with the failure
 * already reported. A missing source is not the same failure as a source that
 * says nothing, and the caller cannot tell them apart from a NULL string. */
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

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *account = NULL;
	ESource *identity = NULL;
	ESource *transport_source = NULL;
	ESourceMailAccount *account_extension;
	ESourceMailIdentity *identity_extension;
	ESourceMailSubmission *submission_extension;
	ESourceBackend *backend_extension;
	CamelSession *session = NULL;
	CamelService *service;
	CamelMimeMessage *message;
	CamelInternetAddress *from;
	CamelInternetAddress *recipients;
	const gchar *account_uid;
	const gchar *recipient;
	const gchar *subject;
	const gchar *body;
	const gchar *identity_uid;
	const gchar *transport_uid;
	const gchar *sender_name;
	const gchar *sender_address;
	const gchar *protocol;
	const gchar *data_dir;
	const gchar *cache_dir;
	gboolean sent_copy_saved = FALSE;
	int status = 1;

	if (argc != 5) {
		g_printerr ("usage: %s <account-source-uid> <recipient> <subject> <body>\n", argv[0]);
		return 2;
	}

	account_uid = argv[1];
	recipient = argv[2];
	subject = argv[3];
	body = argv[4];

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

	/* Hop one. An account's identity is what it sends *as*, and Evolution
	 * offers the user no way to send from an account that names none. */
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

	/* Hop two, and it hangs off the *identity* rather than off the account.
	 * That is not an accident of the file format: one account may send
	 * through several identities and each may leave by a different route, so
	 * the transport is a property of who the mail is from. */
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

	/* The protocol off the transport source, not off the account's: they are
	 * two `BackendName` lines in two files, and a transport that named
	 * another provider is a perfectly valid configuration that this account
	 * does not have. */
	backend_extension = e_source_get_extension (transport_source, E_SOURCE_EXTENSION_MAIL_TRANSPORT);
	protocol = e_source_backend_get_backend_name (backend_extension);
	g_print ("protocol=%s\n", protocol ? protocol : "");

	session = g_object_new (CAMEL_TYPE_SESSION,
				"user-data-dir", data_dir,
				"user-cache-dir", cache_dir,
				NULL);

	/* CAMEL_PROVIDER_TRANSPORT, which is a different entry of the same
	 * registered provider struct than mail-client.c's store came out of. A
	 * provider that left the transport slot G_TYPE_INVALID loads, receives
	 * mail, and fails here. */
	service = camel_session_add_service (session, transport_uid, protocol,
					     CAMEL_PROVIDER_TRANSPORT, &error);
	if (!service) {
		status = fail ("add-service", error);
		goto out;
	}

	e_source_camel_configure_service (transport_source, service);

	if (!camel_service_connect_sync (service, NULL, &error)) {
		status = fail ("connect", error);
		goto out;
	}

	g_print ("transport-connected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_CONNECTED ? 1 : 0);

	/* What the composer would have handed over. The `From` header and the
	 * envelope sender are the same address here, which is the ordinary case;
	 * that they *may* differ is jmap-mail's own tests' business. */
	message = camel_mime_message_new ();
	camel_mime_message_set_subject (message, subject);

	from = camel_internet_address_new ();
	camel_internet_address_add (from, sender_name, sender_address);
	camel_mime_message_set_from (message, from);

	recipients = camel_internet_address_new ();
	camel_internet_address_add (recipients, NULL, recipient);
	camel_mime_message_set_recipients (message, CAMEL_RECIPIENT_TYPE_TO, recipients);

	camel_mime_part_set_content (CAMEL_MIME_PART (message), body, strlen (body),
				     "text/plain; charset=UTF-8");

	/* The two address lists are passed separately from the message on
	 * purpose: they are the envelope, which is what the message is
	 * *delivered* by, and Evolution fills them in from the account and the
	 * composer's recipient fields rather than from the headers. */
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
	/* Camel's one out-parameter besides the error: whether the transport has
	 * already saved the sent copy. Evolution appends one of its own when it
	 * is told FALSE, so this line is the difference between one copy in Sent
	 * and two. */
	g_print ("sent-copy-saved=%d\n", sent_copy_saved ? 1 : 0);

	g_object_unref (recipients);
	g_object_unref (from);
	g_object_unref (message);

	/* Evolution disconnects a transport when it is done with the outbox, and
	 * a transport that held its connection open would keep an HTTP client —
	 * and its socket — alive for the life of the account. */
	if (!camel_service_disconnect_sync (service, TRUE, NULL, &error)) {
		status = fail ("disconnect", error);
		goto out;
	}

	g_print ("transport-disconnected=%d\n",
		 camel_service_get_connection_status (service) == CAMEL_SERVICE_DISCONNECTED ? 1 : 0);

	status = 0;

out:
	g_clear_object (&session);
	g_clear_object (&transport_source);
	g_clear_object (&identity);
	g_clear_object (&account);
	g_clear_object (&registry);

	return status;
}
