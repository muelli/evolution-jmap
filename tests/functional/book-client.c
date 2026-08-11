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

/* The nickname, which EDS keeps on one NICKNAME line and JSContact keeps as
 * an entry of the `nicknames` map. The comma is the point of this one: RFC
 * 2426 §3.1.3 states the nicknames as a comma-separated list, so a comma the
 * user typed is the character that decides whether one nickname stays one
 * nickname across real EDS, our emitter, and the parser reading it back — or
 * quietly becomes two entries the server is then told about. */
#define TEST_NICKNAME "Vee, the tall one"

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

/* The same address written out as it would be printed on an envelope, which
 * EDS keeps on a LABEL line of its own and JSContact keeps as the address's
 * `full`. Set here because the pairing is what is worth checking on real
 * EDS: E_CONTACT_ADDRESS_LABEL_WORK is a synthetic field, so EDS rebuilds
 * the line from the text alone and the X-JMAP-KEY naming the address it
 * belongs to does not survive — leaving the TYPE as the only thing that says
 * the label and the ADR above are two views of one address. The newlines are
 * the second half of the point: they are what makes it a label rather than a
 * street, and vCard states them escaped. */
#define TEST_LABEL TEST_STREET "\n" TEST_CODE " " TEST_LOCALITY "\n" TEST_COUNTRY

/* The job title and the role played, which EDS keeps on separate TITLE and
 * ROLE lines and JSContact keeps in one `titles` map told apart by `kind`.
 * Set here so the test says whether the two halves of that map survive a
 * crossing through real EDS, and come back on the right line each. */
#define TEST_TITLE "Research Scientist"
#define TEST_ROLE "Project Lead"

/* The free-text note, which EDS keeps on one NOTE line and JSContact keeps
 * as an entry of the `notes` map. The semicolon and the comma are here on
 * purpose: they are what vCard gives structural meaning to, and a note is
 * the one mapped property a user types prose into, so this is where the
 * escaping is shown to survive the EDS that reads the line back. */
#define TEST_NOTE "met at FOSDEM; owes me a beer, apparently"

/* The home page, which EDS keeps on the first URL line and JSContact keeps as
 * an entry of the `links` map. The comma in the query string is the point of
 * this one: a URI may hold the characters a vCard value gives structural
 * meaning to, and EDS escapes them on the way out — so this is where the URI
 * the user typed is shown to survive real EDS whole, rather than arriving at
 * the server cut off at the comma or carrying a backslash EDS put there. */
#define TEST_HOMEPAGE "https://dana.example/profile?tags=x-files,ufo"

/* The instant-messaging handle, which EDS keeps on an X-JABBER line and
 * JSContact keeps as an entry of the `onlineServices` map. Set through the
 * per-slot field rather than the multi-valued one, because the slot is what
 * Evolution's contact editor writes and what decides the TYPE the line carries:
 * a line with no TYPE reaches none of these fields at all, so this is where the
 * parameter our emitter writes is shown to be the one real EDS reads. The comma
 * in the handle is the second point — a JSContact `user` is free text, and vCard
 * gives the comma structural meaning — so a handle arriving at the server cut
 * off at it, or carrying the backslash EDS wrote, would show up here. */
#define TEST_IM_HANDLE "dana,scully@jabber.example"

/* The categories the contact is filed under, which EDS keeps as a list on one
 * CATEGORIES line and JSContact keeps as the `keywords` Set. Two of them,
 * because the crossing is a cardinality rather than a value — one line here,
 * two members there — and the comma inside the second is the point: it is the
 * character that separates the items, so it decides whether two tags stay two
 * tags across real EDS, our emitter and the reader. A tag arriving split would
 * tell the server the contact is filed under three. */
#define TEST_CATEGORY_ONE "Friends"
#define TEST_CATEGORY_TWO "beer, in Berlin"
/* What the two are reported as, joined on a character neither of them holds:
 * the harness reads stdout as one `key=value` per line and the comma is the
 * very thing under test, so it cannot also be the delimiter. */
#define TEST_CATEGORY_SEPARATOR "|"

/* The birthday, which EDS keeps in a structured E_CONTACT_BIRTH_DATE field
 * and JSContact keeps as an `anniversaries` entry dated by a PartialDate.
 * Set here because the crossing is a change of shape rather than of name —
 * three numbers on this side, one date line in between, three JSON members
 * on the other — and because EDS rebuilds that line out of its own field,
 * dropping the X-JMAP-KEY the mapping writes on it. Whether the date still
 * arrives as the birthday it is, on the entry it belongs to, is a claim
 * about real EDS that the mapping's own tests cannot make. */
#define TEST_BIRTH_YEAR 1964
#define TEST_BIRTH_MONTH 3
#define TEST_BIRTH_DAY 27

/* Report one observation whose value has line breaks in it. The harness
 * reads stdout as one `key=value` per line, so a raw newline would end the
 * observation early and lose the rest; only the newlines are rewritten, so
 * what is asserted on the other side is still the text EDS gave back rather
 * than some encoding of it. */
static void
report_multiline (const gchar *key,
                  const gchar *value)
{
	gchar **lines = g_strsplit (value ? value : "", "\n", -1);
	gchar *joined = g_strjoinv ("\\n", lines);

	g_print ("%s=%s\n", key, joined);

	g_free (joined);
	g_strfreev (lines);
}

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
	EContactDate birthday = { TEST_BIRTH_YEAR, TEST_BIRTH_MONTH, TEST_BIRTH_DAY };
	EContactDate *read_back_birthday;
	gchar *read_back_birthday_text;
	GList *categories = NULL;
	GList *read_back_categories;
	GPtrArray *category_texts;
	gchar *read_back_categories_text;
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
	e_contact_set (contact, E_CONTACT_NICKNAME, TEST_NICKNAME);
	e_contact_set (contact, E_CONTACT_ORG, TEST_ORG);
	e_contact_set (contact, E_CONTACT_ORG_UNIT, TEST_ORG_UNIT);
	e_contact_set (contact, E_CONTACT_TITLE, TEST_TITLE);
	e_contact_set (contact, E_CONTACT_ROLE, TEST_ROLE);
	e_contact_set (contact, E_CONTACT_NOTE, TEST_NOTE);
	e_contact_set (contact, E_CONTACT_HOMEPAGE_URL, TEST_HOMEPAGE);
	e_contact_set (contact, E_CONTACT_BIRTH_DATE, &birthday);
	e_contact_set (contact, E_CONTACT_IM_JABBER_HOME_1, TEST_IM_HANDLE);

	/* Set as the list rather than as the comma-joined E_CONTACT_CATEGORIES
	 * string, because that string cannot say which comma is a separator:
	 * EDS splits it and would file TEST_CATEGORY_TWO as two categories
	 * before this test had begun. The list is also what Evolution's
	 * Categories dialogue writes. */
	categories = g_list_append (categories, (gchar *) TEST_CATEGORY_ONE);
	categories = g_list_append (categories, (gchar *) TEST_CATEGORY_TWO);
	e_contact_set (contact, E_CONTACT_CATEGORY_LIST, categories);
	g_list_free (categories);

	address = e_contact_address_new ();
	address->street = g_strdup (TEST_STREET);
	address->locality = g_strdup (TEST_LOCALITY);
	address->code = g_strdup (TEST_CODE);
	address->country = g_strdup (TEST_COUNTRY);
	e_contact_set (contact, E_CONTACT_ADDRESS_WORK, address);
	e_contact_address_free (address);
	e_contact_set (contact, E_CONTACT_ADDRESS_LABEL_WORK, TEST_LABEL);

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
	g_print ("read-back-nickname=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_NICKNAME));
	g_print ("read-back-org=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_ORG));
	g_print ("read-back-org-unit=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_ORG_UNIT));
	g_print ("read-back-title=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_TITLE));
	g_print ("read-back-role=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_ROLE));
	g_print ("read-back-note=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_NOTE));
	g_print ("read-back-homepage=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_HOMEPAGE_URL));
	/* Read out of the same slot it was written to: a handle that reached the
	 * server but came back on a line without a TYPE would be missing here
	 * while sitting in the vCard, which is exactly the failure the parameter
	 * exists to prevent. */
	g_print ("read-back-jabber=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_IM_JABBER_HOME_1));

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

	/* The birthday, likewise boxed, and reported as the text
	 * e_contact_date_to_string() writes — which is the same spelling the
	 * date line carries, so the two ends of this leg compare like with
	 * like without either of them re-deriving it. */
	read_back_birthday = e_contact_get (read_back, E_CONTACT_BIRTH_DATE);
	read_back_birthday_text = read_back_birthday ? e_contact_date_to_string (read_back_birthday) : NULL;
	g_print ("read-back-birthday=%s\n", read_back_birthday_text ? read_back_birthday_text : "");
	g_free (read_back_birthday_text);
	if (read_back_birthday)
		e_contact_date_free (read_back_birthday);

	/* The categories, which come back as a list: reported joined, so that
	 * how many there are is as visible on the other side as what they say.
	 * A tag that had been split on its comma would show up here as an
	 * extra item rather than as a mangled one. */
	read_back_categories = e_contact_get (read_back, E_CONTACT_CATEGORY_LIST);
	category_texts = g_ptr_array_new ();
	for (GList *link = read_back_categories; link; link = link->next)
		g_ptr_array_add (category_texts, link->data);
	g_ptr_array_add (category_texts, NULL);
	read_back_categories_text = g_strjoinv (TEST_CATEGORY_SEPARATOR, (gchar **) category_texts->pdata);
	g_print ("read-back-categories=%s\n", read_back_categories_text);
	g_free (read_back_categories_text);
	g_ptr_array_free (category_texts, TRUE);
	g_list_free_full (read_back_categories, g_free);

	/* The LABEL line, which is a field of its own rather than part of the
	 * boxed address above — and the one observation that has to be
	 * escaped, because a label is written across several lines. */
	report_multiline ("read-back-address-label",
			  e_contact_get_const (read_back, E_CONTACT_ADDRESS_LABEL_WORK));
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
