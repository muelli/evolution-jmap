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
 * There are eight phases, chosen on the command line, because they need
 * different books: `write` starts from an empty address book and puts a
 * contact into it, while `edit`, `rename`, `repicture`, `recalendar`,
 * `respouse`, `unspouse` and `renote` each start from one the mock was seeded
 * with before EDS connected — a card that came from the *server*, which is the
 * only way to have EDS read something no vCard this program could write would
 * produce. Those seven differ only in which field the user changes, and that is
 * the whole distinction under test: `edit` touches a field beside the name,
 * `rename` retypes the name itself, `repicture` replaces the photo,
 * `recalendar` retypes the calendar address, `respouse` retypes who the contact
 * is married to, `unspouse` empties that field instead, and `renote` retypes the
 * note on a card carrying two of them. They are modes of one program rather
 * than separate programs because they open the same book the same way and differ
 * only in what they then ask of it.
 *
 *   usage: functional-book-client <source-uid> write <full-name> <photo-base64>
 *          functional-book-client <source-uid> edit <contact-uid> <email>
 *          functional-book-client <source-uid> rename <contact-uid> \
 *                                 <full-name> <given-name>
 *          functional-book-client <source-uid> repicture <contact-uid> \
 *                                 <photo-base64>
 *          functional-book-client <source-uid> recalendar <contact-uid> \
 *                                 <calendar-uri>
 *          functional-book-client <source-uid> respouse <contact-uid> <spouse>
 *          functional-book-client <source-uid> unspouse <contact-uid>
 *          functional-book-client <source-uid> renote <contact-uid> <note>
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

/* The contact's own calendar and the free/busy data drawn from it, which EDS
 * keeps on CALURI and FBURL — the two fields Evolution shows as Calendar and
 * Free/Busy — and JSContact keeps in one `calendars` map told apart by a `kind`
 * neither line carries. Both are set because the crossing is which line a kind
 * chose: a mapping that put both URIs under one kind would still produce two
 * lines with the right two URIs, and only setting both and reading both back
 * out of their own fields can say they did not swap. */
#define TEST_CALENDAR_URI "https://dana.example/cal/dana.ics"
#define TEST_FREEBUSY_URI "https://dana.example/fb/dana.ifb"

/* Who the contact is married to, which EDS keeps on an X-EVOLUTION-SPOUSE line —
 * vCard 3.0 has no RELATED — and JSContact keeps as the *key* of a `relatedTo`
 * entry stating the type `spouse`. The one mapped property whose key crosses
 * rather than its value, so what this checks against real EDS is that the field
 * Evolution shows as Spouse is the line the emitter writes, and that the name on
 * it reaches the server as an entity of its own rather than as a value hung off
 * one. */
#define TEST_SPOUSE "Fox Mulder"

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

/* What the picture this test sets is. The bytes themselves come in on the
 * command line — they are what is compared on the other side, so they are
 * spelled once, there — but the media type is set here beside every other
 * property, because it is what EDS turns into the `TYPE` parameter on the line:
 * measured against libebook-contacts 3.52, EDS writes the subtype alone
 * (`TYPE=png`) and rebuilds `image/png` out of it on the way back, and this is
 * the leg that says our emitter and reader agree with that. */
#define TEST_PHOTO_MEDIA_TYPE "image/png"

/* How long the `edit` phase waits for the contact the mock was seeded with to
 * become gettable, and how often it asks. EBookMetaBackend answers a get for
 * a contact its cache has never heard of by asking the backend to load it, so
 * the first try is expected to succeed — but the backend also schedules a
 * refresh of its own during the connect, and a get that arrives while that is
 * still running can be answered out of a cache the refresh has not filled
 * yet. Polling turns that ordering into a wait rather than a flake. Ten
 * seconds in total, which is far beyond what a local mock needs and still
 * well inside the CTest timeout, so a genuinely absent contact fails the test
 * rather than hanging it. */
#define EDIT_WAIT_TRIES 200
#define EDIT_WAIT_INTERVAL_US 50000

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

/* Report the parts of the name EDS parsed out of the N line. Both are
 * synthetic fields: EDS splits the line's fields into them on the way in and
 * rebuilds the line from them on the way out, so they are what a save hands
 * back to the backend — and, for the given name, the string a JSContact name
 * with more than one component of a kind was flattened into. */
static void
report_name_fields (const gchar *prefix,
                    EContact *contact)
{
	const gchar *given = e_contact_get_const (contact, E_CONTACT_GIVEN_NAME);
	const gchar *family = e_contact_get_const (contact, E_CONTACT_FAMILY_NAME);

	g_print ("%s-given-name=%s\n", prefix, given ? given : "");
	g_print ("%s-family-name=%s\n", prefix, family ? family : "");
}

/* Report the picture EDS holds for a contact.
 *
 * Several observations rather than one, because a photo can go wrong in ways
 * that look alike from the outside. `type` is EDS's own verdict on the line: a
 * picture the card carries is `inlined`, one it merely points at is `uri`, and a
 * line EDS could not make either of is no photo at all — which is what the user
 * would be shown, so it is reported as `absent` rather than by leaving the
 * observation out.
 *
 * A picture read back through a book is expected to be a `uri` one, which is
 * not what was written and not this program's doing: `EBookMetaBackend` puts
 * every contact it caches through `store_inline_photos`, which writes the bytes
 * into a file of its own under the book's cache directory and rewrites the line
 * to point at it. So the bytes are reported for either kind — read out of the
 * struct for an inlined photo, and read out of that file for a `file:` URI —
 * because "did the picture survive" is a question about the bytes and not about
 * which of the two shapes EDS chose to keep them in. */
static void
report_photo (const gchar *prefix,
              EContact *contact)
{
	EContactPhoto *photo = e_contact_get (contact, E_CONTACT_PHOTO);
	const gchar *uri;
	gchar *path;
	gchar *contents = NULL;
	gsize length = 0;
	gchar *base64;

	if (!photo) {
		g_print ("%s-photo-type=absent\n", prefix);
		return;
	}

	if (photo->type == E_CONTACT_PHOTO_TYPE_INLINED) {
		base64 = g_base64_encode (photo->data.inlined.data,
					  photo->data.inlined.length);

		g_print ("%s-photo-type=inlined\n", prefix);
		g_print ("%s-photo-media-type=%s\n", prefix,
			 photo->data.inlined.mime_type ? photo->data.inlined.mime_type : "");
		g_print ("%s-photo-base64=%s\n", prefix, base64);
		g_free (base64);
		e_contact_photo_free (photo);
		return;
	}

	uri = photo->data.uri ? photo->data.uri : "";
	g_print ("%s-photo-type=uri\n", prefix);
	g_print ("%s-photo-uri=%s\n", prefix, uri);

	/* Followed only for a local file, which is the one kind EDS itself
	 * writes. A picture the *server* pointed at over the network is a URI
	 * this program has no business fetching. */
	path = g_filename_from_uri (uri, NULL, NULL);
	if (path && g_file_get_contents (path, &contents, &length, NULL)) {
		base64 = g_base64_encode ((const guchar *) contents, length);
		g_print ("%s-photo-file-base64=%s\n", prefix, base64);
		g_free (base64);
	}

	g_free (contents);
	g_free (path);
	e_contact_photo_free (photo);
}

/* Report the two calendaring addresses EDS holds for a contact, each out of the
 * field it belongs to.
 *
 * Both together and never one alone: the mapping tells them apart by a `kind`
 * that neither CALURI nor FBURL carries, so the failure worth catching is the
 * two having swapped — which is invisible to whichever of them is asked about on
 * its own. An unset field is reported as the empty string rather than left out,
 * so a leg can say a resource is absent. */
static void
report_calendars (const gchar *prefix,
                  EContact *contact)
{
	const gchar *calendar = e_contact_get_const (contact, E_CONTACT_CALENDAR_URI);
	const gchar *freebusy = e_contact_get_const (contact, E_CONTACT_FREEBUSY_URL);

	g_print ("%s-calendar-uri=%s\n", prefix, calendar ? calendar : "");
	g_print ("%s-freebusy-uri=%s\n", prefix, freebusy ? freebusy : "");
}

/* Report who EDS says the contact is married to.
 *
 * One field and one line, but the observation the harness cares about most is a
 * cardinality it cannot see from here: EDS hands back the value of the *first*
 * X-EVOLUTION-SPOUSE line, so a set that left the old line standing beside the
 * new one is invisible at this end and shows up only in what reaches the
 * server. An unset field is reported as the empty string rather than left out,
 * so a leg can say the contact has no spouse. */
static void
report_spouse (const gchar *prefix,
               EContact *contact)
{
	const gchar *spouse = e_contact_get_const (contact, E_CONTACT_SPOUSE);

	g_print ("%s-spouse=%s\n", prefix, spouse ? spouse : "");
}

/* Report the X-EVOLUTION-SPOUSE line itself, which is the one thing about this
 * property report_spouse above cannot say: e_contact_get reads a line naming
 * nobody and a card carrying no such line alike, as no spouse.
 *
 * Which of those two a cleared field produces is libebook-contacts' business and
 * is what the `unspouse` phase is here to look at. The mapping withdraws the
 * marriage either way — a line naming nobody is refused on the way in — so this
 * is reported rather than judged: the harness holds the *field* to being empty,
 * which is true whichever EDS does, and reads this to know which it did. */
static void
report_spouse_line (const gchar *prefix,
                    EContact *contact)
{
	EVCardAttribute *attribute;
	gchar *value;

	attribute = e_vcard_get_attribute (E_VCARD (contact), "X-EVOLUTION-SPOUSE");
	g_print ("%s-spouse-line=%s\n", prefix, attribute ? "present" : "absent");

	if (!attribute)
		return;

	/* NULL for an attribute holding no value at all, which is a different
	 * shape from one holding the empty string and worth telling apart. */
	value = e_vcard_attribute_get_value (attribute);
	g_print ("%s-spouse-line-value=%s\n", prefix, value ? value : "");
	g_free (value);
}

/* Report the note EDS holds for a contact, and how many NOTE lines the card
 * carries.
 *
 * The count is the observation the field cannot give, and the reason this
 * function exists at all: e_contact_get hands back the value of the *first* NOTE
 * line, so a card holding one note and a card holding five read alike here — and
 * a set that replaced every line of the name, or appended beside the first,
 * reads back correctly while the card underneath says something else. The
 * harness holds it to a number; this end only counts.
 *
 * An unset field is reported as the empty string rather than left out, so a leg
 * can say the contact has no note. */
static void
report_notes (const gchar *prefix,
              EContact *contact)
{
	const gchar *note = e_contact_get_const (contact, E_CONTACT_NOTE);
	GList *attribute;
	guint lines = 0;

	g_print ("%s-note=%s\n", prefix, note ? note : "");

	for (attribute = e_vcard_get_attributes (E_VCARD (contact));
	     attribute;
	     attribute = attribute->next) {
		const gchar *name = e_vcard_attribute_get_name (attribute->data);

		/* Case-insensitively, because that is what RFC 2426 §5 says a
		 * property name is compared as — a card whose lines came back
		 * spelled `Note` would otherwise be counted as carrying none. */
		if (name && g_ascii_strcasecmp (name, EVC_NOTE) == 0)
			lines++;
	}

	g_print ("%s-note-lines=%u\n", prefix, lines);
}

/* The first phase: an empty book, one contact written into it with every
 * mapped property set, and that contact read back. */
static int
write_phase (EBookClient *book,
             const gchar *full_name,
             const gchar *photo_base64)
{
	GError *error = NULL;
	EBookQuery *query;
	gchar *query_string;
	GSList *contacts = NULL;
	EContact *contact;
	EContact *read_back = NULL;
	EContactPhoto photo = { 0 };
	guchar *photo_bytes;
	gsize photo_length = 0;
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
	e_contact_set (contact, E_CONTACT_CALENDAR_URI, TEST_CALENDAR_URI);
	e_contact_set (contact, E_CONTACT_FREEBUSY_URL, TEST_FREEBUSY_URI);
	e_contact_set (contact, E_CONTACT_SPOUSE, TEST_SPOUSE);
	e_contact_set (contact, E_CONTACT_BIRTH_DATE, &birthday);
	e_contact_set (contact, E_CONTACT_IM_JABBER_HOME_1, TEST_IM_HANDLE);

	/* The picture, inlined: the form Evolution's contact editor writes when
	 * the user picks an image file, and the only form a media type can be
	 * read off. Set through the boxed struct because that is the only way to
	 * state the bytes and what they are together; EDS takes the copies it
	 * needs, so the decoded bytes are freed as soon as the call returns. */
	photo_bytes = g_base64_decode (photo_base64, &photo_length);
	photo.type = E_CONTACT_PHOTO_TYPE_INLINED;
	photo.data.inlined.mime_type = (gchar *) TEST_PHOTO_MEDIA_TYPE;
	photo.data.inlined.data = photo_bytes;
	photo.data.inlined.length = photo_length;
	e_contact_set (contact, E_CONTACT_PHOTO, &photo);
	g_free (photo_bytes);

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
	report_calendars ("read-back", read_back);
	report_spouse ("read-back", read_back);
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

	/* The picture, likewise boxed, and out of EDS's own cache: the bytes the
	 * backend re-rendered onto a `PHOTO` line after the write, read back
	 * through the field Evolution shows the contact's photo from. */
	report_photo ("read-back", read_back);

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

	return 0;
}

/* Ask for one contact until EDS has it, or give up. A miss is reported as
 * E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND rather than as a failure of the call,
 * so it is cleared and retried; any other error would be retried too, and the
 * caller says what it was waiting for when the tries run out. */
static EContact *
wait_for_contact (EBookClient *book,
                  const gchar *uid)
{
	guint try;

	for (try = 0; try < EDIT_WAIT_TRIES; try++) {
		EContact *contact = NULL;
		GError *error = NULL;

		if (e_book_client_get_contact_sync (book, uid, &contact, NULL, &error)) {
			g_print ("waited-tries=%u\n", try);
			return contact;
		}

		g_clear_error (&error);
		g_usleep (EDIT_WAIT_INTERVAL_US);
	}

	return NULL;
}

/* What EDS made of the card the mock was seeded with, before anything is
 * changed. The name fields are the load-bearing ones: they are the text the
 * backend's save compares against to decide whether the user retyped a field,
 * so a phase that did not report them could not say what that comparison was
 * given. */
static void
report_seeded_contact (EContact *contact)
{
	g_print ("read-full-name=%s\n",
		 (const gchar *) e_contact_get_const (contact, E_CONTACT_FULL_NAME));
	g_print ("read-email=%s\n",
		 (const gchar *) e_contact_get_const (contact, E_CONTACT_EMAIL_1));
	report_name_fields ("read", contact);
	/* The picture the server filed on the card, as EDS read it off the line
	 * the emitter wrote — the direction the `write` phase cannot ask about,
	 * where that line came from EDS in the first place. */
	report_photo ("read", contact);
	/* Likewise the two calendaring addresses, and for the same reason: what
	 * EDS made of lines the emitter wrote rather than of lines it wrote
	 * itself. */
	report_calendars ("read", contact);
	report_spouse ("read", contact);
	/* And the notes, which are the one property of which the card holds more
	 * than the user is shown — hence the line count beside the field. */
	report_notes ("read", contact);
}

/* Save an edited contact and report what EDS holds for it afterwards. Takes
 * ownership of `contact`, so a phase can end on this call.
 *
 * The read-back is a fresh get rather than the contact just handed over,
 * because what is wanted is EDS's own copy — the vCard the backend re-rendered
 * after the save and EDS put back in its cache — not the one this program
 * filled in. */
static int
save_and_report (EBookClient *book,
                 EContact *contact,
                 const gchar *uid)
{
	GError *error = NULL;
	EContact *read_back = NULL;

	if (!e_book_client_modify_contact_sync (book, contact, E_BOOK_OPERATION_FLAG_NONE,
						NULL, &error)) {
		g_object_unref (contact);
		return fail ("modify", error);
	}

	g_object_unref (contact);

	if (!e_book_client_get_contact_sync (book, uid, &read_back, NULL, &error))
		return fail ("read-back", error);

	g_print ("read-back-full-name=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_FULL_NAME));
	g_print ("read-back-email=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_EMAIL_1));
	report_name_fields ("read-back", read_back);
	/* What EDS holds for the picture after the save, which for a phase that
	 * did not touch it is the other half of "the save left it alone": a
	 * picture the backend rewrote would come back out of the cache changed
	 * even though nobody asked. */
	report_photo ("read-back", read_back);
	report_calendars ("read-back", read_back);
	report_spouse ("read-back", read_back);
	report_notes ("read-back", read_back);
	g_object_unref (read_back);

	return 0;
}

/* The second phase: a contact that came from the server, edited the way a user
 * edits one — read what EDS has, change a single field that has nothing to do
 * with the name, save it back.
 *
 * The point is what is *not* touched. A JSContact name can hold several
 * components of one kind, and the N line has one field per kind, so EDS is
 * handed those components joined into one string; the backend restores them on
 * the way back by recognising that the field still reads as its parts joined.
 * That recognition is a string comparison against the text EDS hands back,
 * which is exactly what no test below this file can measure. */
static int
edit_phase (EBookClient *book,
            const gchar *uid,
            const gchar *email)
{
	EContact *contact;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	/* The edit itself: one field, and not the name. Whatever the save does
	 * to the name is therefore something nobody asked for. */
	e_contact_set (contact, E_CONTACT_EMAIL_1, email);

	return save_and_report (book, contact, uid);
}

/* The third phase: the other half of `edit`, on the same seeded card. Here the
 * user *does* retype the name — the one field that holds both halves of the
 * double-barrelled given name — and the parts the server stated separately are
 * the ones that must NOT come back.
 *
 * Nothing in the text the user typed says which half it replaced, so the two
 * old parts are gone and the field states one component. That is the branch the
 * `edit` phase cannot reach, and reaching it through real EDS is the point: EDS
 * rebuilds the N line out of its own synthetic fields, so the string the save
 * compares against is EDS's rendering of what the user typed rather than
 * anything this repository wrote.
 *
 * Both fields are set because both are what Evolution's contact editor writes:
 * the given-name field is the edit, and the FN line it keeps in step with it is
 * a separate attribute that would otherwise still spell the old name. */
static int
rename_phase (EBookClient *book,
              const gchar *uid,
              const gchar *full_name,
              const gchar *given_name)
{
	EContact *contact;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	e_contact_set (contact, E_CONTACT_GIVEN_NAME, given_name);
	e_contact_set (contact, E_CONTACT_FULL_NAME, full_name);

	return save_and_report (book, contact, uid);
}

/* The fourth phase: the same seeded card, and the user picks a new picture for
 * it — the one edit that reaches the photo field itself.
 *
 * This is where the picture stops being something the round trip carries and
 * becomes something the save has to place. `e_contact_set` rewrites the `PHOTO`
 * line out of the photo it is given and drops the parameters that were on it,
 * the `X-JMAP-KEY` among them, so the entry the new picture belongs to cannot be
 * found by its key and has to be paired with the one it replaced. Nothing below
 * the daemons can say whether that is what really happens: EDS hands a save the
 * cached card with the picture written back into it, and whether *that* line
 * keeps its key while a *set* one does not is EDS's behaviour, not ours.
 *
 * Inlined, because that is what Evolution's contact editor sets when the user
 * picks an image file. */
static int
repicture_phase (EBookClient *book,
                 const gchar *uid,
                 const gchar *photo_base64)
{
	EContact *contact;
	EContactPhoto photo = { 0 };
	guchar *bytes;
	gsize length = 0;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	bytes = g_base64_decode (photo_base64, &length);
	photo.type = E_CONTACT_PHOTO_TYPE_INLINED;
	photo.data.inlined.mime_type = (gchar *) TEST_PHOTO_MEDIA_TYPE;
	photo.data.inlined.data = bytes;
	photo.data.inlined.length = length;
	e_contact_set (contact, E_CONTACT_PHOTO, &photo);
	g_free (bytes);

	return save_and_report (book, contact, uid);
}

/* The fifth phase: the user moves their calendar, on a card the server filed
 * two calendaring resources on.
 *
 * The distinction from `repicture` beside it is the whole point.
 * E_CONTACT_CALENDAR_URI is a plain vCard attribute rather than a synthetic
 * field, so a set on it rewrites the value of the first CALURI line in place and
 * leaves the parameters — X-JMAP-KEY included — where they were, which means the
 * save can patch by key instead of having to pair a keyless URI with the one it
 * replaced. That is a claim about libebook-contacts, and this is the phase that
 * makes it through the daemons Evolution actually goes through.
 *
 * Only the calendar is set. The free/busy address beside it is what says a patch
 * aimed at one entry did not land on the map. */
static int
recalendar_phase (EBookClient *book,
                  const gchar *uid,
                  const gchar *calendar_uri)
{
	EContact *contact;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	e_contact_set (contact, E_CONTACT_CALENDAR_URI, calendar_uri);

	return save_and_report (book, contact, uid);
}

/* The sixth phase: the user retypes who they are married to, on a card the
 * server relates to two people.
 *
 * The one property whose *key* is what the line shows, so what the daemons are
 * asked here is not whether a key survived a set — there is no key — but whether
 * a set leaves one X-EVOLUTION-SPOUSE line or two. E_CONTACT_SPOUSE is a plain
 * vCard attribute, so a set should rewrite the value of the first line of that
 * name in place; if it appended instead, the card would state two marriages and
 * the mapping would have no way to tell that from a card that really did, since
 * the lines are all it has. Nothing below the daemons can answer that, and this
 * end cannot see it either — e_contact_get hands back the first line's value
 * whichever it is — so the answer is over in what reaches the server.
 *
 * Only the spouse is set. The brother the server also relates the card to
 * reaches no line at all, which is what says the save withdrew a marriage rather
 * than replacing everyone the card relates to. */
static int
respouse_phase (EBookClient *book,
                const gchar *uid,
                const gchar *spouse)
{
	EContact *contact;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	e_contact_set (contact, E_CONTACT_SPOUSE, spouse);

	return save_and_report (book, contact, uid);
}

/* The seventh phase: the user empties the Spouse field on the same seeded card,
 * rather than retyping it.
 *
 * The other half of `respouse`, and the branch of the save nothing else reaches:
 * a card handed back to the backend stating no relations at all, where the
 * marriage has to be withdrawn with nothing put in its place. The brother is
 * what says the withdrawal was of one entity rather than of the property — he
 * reaches no line, so a save that answered an emptied field by taking the whole
 * of `relatedTo` back would delete a relation the user never saw.
 *
 * The empty string rather than NULL, because that is what Evolution's contact
 * editor writes: it hands e_contact_set the text of the entry, and the text of an
 * entry the user emptied is "". What EDS then does to the line is the one thing
 * about this path that was inferred rather than measured, so it is reported —
 * see report_spouse_line. */
static int
unspouse_phase (EBookClient *book,
                const gchar *uid)
{
	EContact *contact;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	e_contact_set (contact, E_CONTACT_SPOUSE, "");

	/* The card as the save is about to be handed it, before the modify: what
	 * the field reads as, and what is left of the line it came off. */
	report_spouse ("cleared", contact);
	report_spouse_line ("cleared", contact);

	return save_and_report (book, contact, uid);
}

/* The eighth phase: the user retypes their note, on a card the server filed two
 * notes on.
 *
 * The one property of which Evolution shows the user *part of a map* and lets
 * them edit it anyway: E_CONTACT_NOTE is the first NOTE line, and every `notes`
 * entry writes a line, so the second note sits behind the field with nothing in
 * the UI saying it is there. What the daemons are asked is therefore two
 * questions at once, and neither can be answered from below them — whether a set
 * rewrites the first line in place with its parameters (the X-JMAP-KEY that says
 * which entry this is) still on it, and whether the second line is still on the
 * card afterwards. A set that took every line of the name with it would make a
 * user editing their note delete one they were never shown.
 *
 * Only the note is set. Everything else the seeded card carries is what says a
 * patch aimed at one entry did not land on the map. */
static int
renote_phase (EBookClient *book,
              const gchar *uid,
              const gchar *note)
{
	EContact *contact;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	e_contact_set (contact, E_CONTACT_NOTE, note);

	/* The card as the save is about to be handed it, before the modify: what
	 * the field reads as, and how many lines of that name the set left. */
	report_notes ("retyped", contact);

	return save_and_report (book, contact, uid);
}

static void
usage (const gchar *program)
{
	g_printerr ("usage: %s <source-uid> write <full-name> <photo-base64>\n"
		    "       %s <source-uid> edit <contact-uid> <email>\n"
		    "       %s <source-uid> rename <contact-uid> <full-name> <given-name>\n"
		    "       %s <source-uid> repicture <contact-uid> <photo-base64>\n"
		    "       %s <source-uid> recalendar <contact-uid> <calendar-uri>\n"
		    "       %s <source-uid> respouse <contact-uid> <spouse>\n"
		    "       %s <source-uid> unspouse <contact-uid>\n"
		    "       %s <source-uid> renote <contact-uid> <note>\n",
		    program, program, program, program, program, program, program,
		    program);
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
	const gchar *source_uid;
	const gchar *phase;
	int status;

	if (argc < 3) {
		usage (argv[0]);
		return 2;
	}

	source_uid = argv[1];
	phase = argv[2];

	if (!((g_str_equal (phase, "write") && argc == 5) ||
	      (g_str_equal (phase, "edit") && argc == 5) ||
	      (g_str_equal (phase, "rename") && argc == 6) ||
	      (g_str_equal (phase, "repicture") && argc == 5) ||
	      (g_str_equal (phase, "recalendar") && argc == 5) ||
	      (g_str_equal (phase, "respouse") && argc == 5) ||
	      (g_str_equal (phase, "unspouse") && argc == 4) ||
	      (g_str_equal (phase, "renote") && argc == 5))) {
		usage (argv[0]);
		return 2;
	}

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

	if (g_str_equal (phase, "write"))
		status = write_phase (book, argv[3], argv[4]);
	else if (g_str_equal (phase, "edit"))
		status = edit_phase (book, argv[3], argv[4]);
	else if (g_str_equal (phase, "rename"))
		status = rename_phase (book, argv[3], argv[4], argv[5]);
	else if (g_str_equal (phase, "repicture"))
		status = repicture_phase (book, argv[3], argv[4]);
	else if (g_str_equal (phase, "recalendar"))
		status = recalendar_phase (book, argv[3], argv[4]);
	else if (g_str_equal (phase, "respouse"))
		status = respouse_phase (book, argv[3], argv[4]);
	else if (g_str_equal (phase, "unspouse"))
		status = unspouse_phase (book, argv[3]);
	else
		status = renote_phase (book, argv[3], argv[4]);

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return status;
}
