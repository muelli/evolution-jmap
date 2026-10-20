/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the transport functional test. It follows the send path
 * from the account, as Evolution does.
 *
 * mail-client.c is the receive half and opens a store. This program opens no
 * store. A `CamelTransport` is a second `CamelService`, made from a second
 * `ESource` through the `object_types` table of the same provider. Only two
 * UID links through a third source connect it to the account:
 *
 *   [Mail Account] IdentityUid  -> the identity source
 *   [Mail Identity] / [Mail Submission] TransportUid -> the transport source
 *
 * Camel does not follow this chain, because it knows nothing about ESource.
 * Evolution follows it in evolution-mail with libedataserver accessors, and
 * this program uses the same accessors. It gets the *account* UID and finds
 * the rest. This is the subject of the test: each link is a string in a file,
 * and nothing else in this repository checks it against the file that it
 * names.
 *
 * The program builds the message from its arguments, not from its own
 * constants. Thus the test checks the same strings that go on the wire.
 * Everything else is as in mail-client.c: one `key=value` line for each
 * observation, and a non-zero exit when a call fails.
 * `rust/crates/jmap-functional/tests/transport.rs` checks the output.
 *
 * The program creates the CamelSession directly, as mail-client.c does. The
 * provider's connect_sync calls authenticate_sync, as every Camel provider
 * does, and the source names no user. The base class then writes a warning on
 * stderr that it "is not intended for production use". This warning is
 * expected.
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

/* One link of the chain: the source that `uid` names, or NULL after it reports
 * the failure. A missing source and an empty UID are different failures, and
 * the caller cannot see the difference in a NULL return. */
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

	/* The harness points these at the scratch tree. The real XDG
	 * directories would put data into the developer's own store. */
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

	/* Link one. The identity of an account is the sender. Evolution does
	 * not let the user send from an account with no identity. */
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

	/* Link two comes from the *identity*, not from the account. One
	 * account can send through many identities, and each identity can use
	 * a different route. Thus the transport is a property of the sender. */
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

	/* Read the protocol from the transport source, not from the account.
	 * These are two `BackendName` lines in two files. A transport for a
	 * different provider is a valid configuration, but this account does
	 * not have one. */
	backend_extension = e_source_get_extension (transport_source, E_SOURCE_EXTENSION_MAIL_TRANSPORT);
	protocol = e_source_backend_get_backend_name (backend_extension);
	g_print ("protocol=%s\n", protocol ? protocol : "");

	session = g_object_new (CAMEL_TYPE_SESSION,
				"user-data-dir", data_dir,
				"user-cache-dir", cache_dir,
				NULL);

	/* CAMEL_PROVIDER_TRANSPORT is a different entry in the provider struct
	 * than the store in mail-client.c. A provider with G_TYPE_INVALID in
	 * the transport entry loads and receives mail, but fails here. */
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

	/* The message from the composer. Here the `From` header and the
	 * envelope sender are the same address, which is usual. The jmap-mail
	 * tests check the case where they are different. */
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

	/* Give the two address lists separately from the message. They are the
	 * envelope, which controls the delivery. Evolution fills them from the
	 * account and the composer recipient fields, not from the headers. */
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
	/* The only Camel out-parameter other than the error: TRUE if the
	 * transport already saved the sent copy. On FALSE, Evolution appends
	 * its own copy, so this value gives one or two copies in Sent. */
	g_print ("sent-copy-saved=%d\n", sent_copy_saved ? 1 : 0);

	g_object_unref (recipients);
	g_object_unref (from);
	g_object_unref (message);

	/* Evolution disconnects a transport after the outbox is empty. A
	 * transport that kept its connection would keep an HTTP client and its
	 * socket for the life of the account. */
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
