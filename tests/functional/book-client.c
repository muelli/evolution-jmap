/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the address book functional test: an ordinary
 * libebook consumer, the way Evolution is one. It knows nothing about JMAP
 * and nothing about the mock server — it opens a book by source UID, reads
 * it, writes one contact to it and reads that back, which is the whole of
 * what an address book is for.
 *
 * Everything around it — the scratch XDG tree, the `.source` keyfile, the
 * private D-Bus session, the mock server and every assertion — belongs to
 * `rust/crates/jmap-functional/tests/address-book.rs`, which runs this
 * program and reads its output. So this file has no test framework in it
 * and no notion of what "correct" is: it reports what EDS told it on
 * stdout, one `key=value` line per observation, and exits non-zero the
 * moment a call fails.
 *
 * C rather than Rust because this is deliberately the *client* API, which
 * the FFI crates in this repository do not bind: eds-sys carries what the
 * backends implement. Binding a second surface just to call it from a test
 * would put a layer of our own between EDS and the thing under test.
 *
 *   usage: functional-book-client <source-uid>
 */

#include <libebook/libebook.h>

#include "connection-status.h"

/* The contact this test writes. The Rust side looks for exactly this name
 * in the mock's store, so the two spellings have to agree; it is passed in
 * rather than hardcoded on both sides. */
#define TEST_EMAIL "dana@example.com"

/* The employer and the department within it, which EDS keeps in the first
 * two components of one ORG line. Written here so the test says what EDS
 * makes of the JSContact `organizations` map, rather than only what our own
 * mapping tests say it should. */
#define TEST_ORG "Acme Ltd"
#define TEST_ORG_UNIT "Research"

/* The postal address, which EDS keeps in the fields of one ADR line and
 * JSContact keeps as a list of named components. Only some of RFC 9553's
 * component kinds have an ADR field, so what this leg says is that the ones
 * that do make the crossing through real EDS, and land in the fields that
 * mean what they meant. TYPE=WORK is what makes it E_CONTACT_ADDRESS_WORK
 * rather than one of the other two synthetic fields. */
#define TEST_STREET "Hauptstrasse 1"
#define TEST_LOCALITY "Berlin"
#define TEST_CODE "10115"
#define TEST_COUNTRY "Germany"

/* The job title and the role played, which EDS keeps on separate TITLE and
 * ROLE lines and JSContact keeps in one `titles` map told apart by `kind`.
 * Set here so the test says whether the two halves of that map survive a
 * crossing through real EDS, and come back on the right line each. */
#define TEST_TITLE "Research Scientist"
#define TEST_ROLE "Project Lead"

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *source;
	EClient *client;
	EBookClient *book;
	EBookQuery *query;
	gchar *query_string;
	GSList *contacts = NULL;
	EContact *contact;
	EContact *read_back = NULL;
	EContactAddress *address;
	EContactAddress *read_back_address;
	gchar *added_uid = NULL;
	const gchar *source_uid;
	const gchar *full_name;

	if (argc != 3) {
		g_printerr ("usage: %s <source-uid> <full-name>\n", argv[0]);
		return 2;
	}

	source_uid = argv[1];
	full_name = argv[2];

	/* Activates evolution-source-registry on the session bus, which reads
	 * the scratch sources directory the harness wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* Activates evolution-addressbook-factory, which is what dlopens
	 * libebookbackendjmap.so out of EDS_ADDRESS_BOOK_MODULES and picks
	 * the factory matching the keyfile's BackendName. A failure here is
	 * usually one of those two steps, not the backend's logic.
	 *
	 * (guint32) -1 is EDS's "do not wait for connected". The wait is not
	 * skipped because the status does not matter — it is asserted, just
	 * below — but because asking for it here cannot work in a program
	 * with no main loop, whatever the backend does: see
	 * connection-status.c. The open itself is synchronous, so everything
	 * it established is already true on the far side by the time this
	 * call returns. */
	client = e_book_client_connect_sync (source, (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	book = E_BOOK_CLIENT (client);

	/* EDS's own verdict on the connect, waited for properly. */
	functional_report_connection_status (source, 10);

	/* Read the properties back over the bus rather than trusting the
	 * client's cached copy. EClient updates those from D-Bus property
	 * notifications delivered on a main context, so a program that does
	 * not run a main loop — this one — would be reading whatever had
	 * happened to arrive: a race, and one that would have hidden the very
	 * bug this test was written for. */
	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return fail ("retrieve-properties", error);

	/* Whether the book accepts writes at all. EDS derives this from what
	 * the backend said during its connect, so a backend that connects
	 * happily and never says it can be written to gives a book that is
	 * silently read-only in the UI — which is a state the write below
	 * would report only as "Permission denied". Reported separately so
	 * the harness can name the cause rather than the symptom. */
	g_print ("readonly=%d\n", e_client_is_readonly (client) ? 1 : 0);

	query = e_book_query_any_field_contains ("");
	query_string = e_book_query_to_string (query);
	e_book_query_unref (query);

	if (!e_book_client_get_contacts_sync (book, query_string, &contacts, NULL, &error)) {
		g_free (query_string);
		return fail ("query", error);
	}

	g_print ("contacts-before=%u\n", g_slist_length (contacts));
	g_slist_free_full (contacts, g_object_unref);
	contacts = NULL;

	contact = e_contact_new ();
	e_contact_set (contact, E_CONTACT_FULL_NAME, full_name);
	e_contact_set (contact, E_CONTACT_EMAIL_1, TEST_EMAIL);
	e_contact_set (contact, E_CONTACT_ORG, TEST_ORG);
	e_contact_set (contact, E_CONTACT_ORG_UNIT, TEST_ORG_UNIT);
	e_contact_set (contact, E_CONTACT_TITLE, TEST_TITLE);
	e_contact_set (contact, E_CONTACT_ROLE, TEST_ROLE);

	address = e_contact_address_new ();
	address->street = g_strdup (TEST_STREET);
	address->locality = g_strdup (TEST_LOCALITY);
	address->code = g_strdup (TEST_CODE);
	address->country = g_strdup (TEST_COUNTRY);
	e_contact_set (contact, E_CONTACT_ADDRESS_WORK, address);
	e_contact_address_free (address);

	if (!e_book_client_add_contact_sync (book, contact, E_BOOK_OPERATION_FLAG_NONE,
					     &added_uid, NULL, &error)) {
		g_object_unref (contact);
		g_free (query_string);
		return fail ("add", error);
	}

	g_object_unref (contact);
	g_print ("added=%s\n", added_uid ? added_uid : "");

	/* Out of the meta backend's cache rather than off the server, which
	 * is the point: EDS is meant to have kept what it just wrote. */
	if (!e_book_client_get_contact_sync (book, added_uid, &read_back, NULL, &error)) {
		g_free (added_uid);
		g_free (query_string);
		return fail ("read-back", error);
	}

	g_print ("read-back-full-name=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_FULL_NAME));
	g_print ("read-back-email=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_EMAIL_1));
	g_print ("read-back-org=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_ORG));
	g_print ("read-back-org-unit=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_ORG_UNIT));
	g_print ("read-back-title=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_TITLE));
	g_print ("read-back-role=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_ROLE));

	/* Structured, so it comes back as a boxed struct rather than a
	 * string: one report per field, because which field a part of the
	 * address landed in is the whole question here. */
	read_back_address = e_contact_get (read_back, E_CONTACT_ADDRESS_WORK);
	g_print ("read-back-street=%s\n",
		 read_back_address && read_back_address->street ? read_back_address->street : "");
	g_print ("read-back-locality=%s\n",
		 read_back_address && read_back_address->locality ? read_back_address->locality : "");
	g_print ("read-back-code=%s\n",
		 read_back_address && read_back_address->code ? read_back_address->code : "");
	g_print ("read-back-country=%s\n",
		 read_back_address && read_back_address->country ? read_back_address->country : "");
	if (read_back_address)
		e_contact_address_free (read_back_address);
	g_object_unref (read_back);

	if (!e_book_client_get_contacts_sync (book, query_string, &contacts, NULL, &error)) {
		g_free (added_uid);
		g_free (query_string);
		return fail ("query-after", error);
	}

	g_print ("contacts-after=%u\n", g_slist_length (contacts));

	g_slist_free_full (contacts, g_object_unref);
	g_free (added_uid);
	g_free (query_string);
	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
