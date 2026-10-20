/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client part of the address-book functional test. It is an ordinary
 * libebook consumer, as Evolution is. It knows nothing about JMAP or the mock
 * server. It opens a book by source UID, reads it, writes contacts and reads
 * them back.
 *
 * `rust/crates/jmap-functional/tests/address-book.rs` supplies everything
 * else: the temporary XDG tree, the `.source` keyfile, the private D-Bus
 * session, the mock server and all assertions. It runs this program and reads
 * its output. This program has no test framework. It writes what EDS returns
 * to stdout, one `key=value` line per observation, and exits non-zero when a
 * call fails.
 *
 * This file is C, not Rust, because it uses the EDS client API. The FFI crates
 * in this repository do not bind that API: eds-sys contains only the backend
 * API. A second binding would put a layer of this project between EDS and the
 * code under test.
 *
 * The command line selects one of twelve phases, because they need different
 * books. `write` starts from an empty book and adds a contact. `edit`,
 * `rename`, `repicture`, `recalendar`, `respouse`, `unspouse`, `renote`,
 * `unnote` and `rehandle` each start from a card that the mock has before EDS
 * connects. Thus EDS reads a card that came from the server, which this
 * program cannot write. These nine phases differ only in the field that the
 * user changes:
 *
 *   - `edit` changes a field that is not the name.
 *   - `rename` changes the name.
 *   - `repicture` replaces the photo.
 *   - `recalendar` changes the calendar address.
 *   - `respouse` changes the spouse, and `unspouse` empties that field.
 *   - `renote` changes the note on a card with two notes, and `unnote`
 *     empties that field.
 *   - `rehandle` changes an instant-messaging handle that the server gives
 *     as a URI.
 *
 * `remove` deletes a seeded card.
 *
 * `list` makes no change. It prints all contacts in the book, sorted by full
 * name. `rust/crates/jmap-functional/tests/book-changes.rs` runs it two times
 * on one on-disk cache, and adds a contact to the mock between the runs. This
 * shows if the refresh after the second connect uses
 * `EBookMetaBackendClass::get_changes_sync` and not `list_existing_sync`.
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
 *          functional-book-client <source-uid> unnote <contact-uid>
 *          functional-book-client <source-uid> rehandle <contact-uid> <handle>
 *          functional-book-client <source-uid> remove <contact-uid>
 *          functional-book-client <source-uid> list
 */

#include <libebook/libebook.h>

#include "connection-status.h"

/* The email address of the contact that the `write` phase creates. The Rust
 * test uses the same value, so the two must agree. */
#define TEST_EMAIL "dana@example.com"

/* The nickname. EDS keeps it on one NICKNAME line, JSContact in the
 * `nicknames` map. RFC 2426 §3.1.3 makes NICKNAME a comma-separated list, so
 * the comma tests that one nickname does not become two entries. */
#define TEST_NICKNAME "Vee, the tall one"

/* The organization and the unit. EDS keeps them in the first two components of
 * one ORG line. They test what real EDS does with the JSContact
 * `organizations` map. */
#define TEST_ORG "Acme Ltd"
#define TEST_ORG_UNIT "Research"

/* The postal address. EDS keeps it in the fields of one ADR line, JSContact as
 * a list of named components. Only some RFC 9553 component kinds have an ADR
 * field. This tests that those kinds go through real EDS into the correct
 * fields. TYPE=WORK selects E_CONTACT_ADDRESS_WORK, not one of the other two
 * synthetic fields. */
#define TEST_STREET "Hauptstrasse 1"
#define TEST_LOCALITY "Berlin"
#define TEST_CODE "10115"
#define TEST_COUNTRY "Germany"

/* The same address as printed on an envelope. EDS keeps it on a LABEL line,
 * JSContact in the `full` member of the address. E_CONTACT_ADDRESS_LABEL_WORK
 * is a synthetic field, so EDS rebuilds the line from the text only, and the
 * X-JMAP-KEY is lost. Then only the TYPE links the label to the ADR above.
 * The newlines make it a label, not a street, and vCard escapes them. */
#define TEST_LABEL TEST_STREET "\n" TEST_CODE " " TEST_LOCALITY "\n" TEST_COUNTRY

/* The job title and the role. EDS keeps them on TITLE and ROLE lines,
 * JSContact in one `titles` map with a `kind` for each. This tests that each
 * comes back on its correct line through real EDS. */
#define TEST_TITLE "Research Scientist"
#define TEST_ROLE "Project Lead"

/* The note. EDS keeps it on one NOTE line, JSContact in the `notes` map. The
 * semicolon and the comma have structural meaning in vCard, so they test the
 * escaping through real EDS. */
#define TEST_NOTE "met at FOSDEM; owes me a beer, apparently"

/* The home page. EDS keeps it on the first URL line, JSContact in the `links`
 * map. EDS escapes the comma in the query string. This tests that the server
 * gets the full URI, with no cut at the comma and no added backslash. */
#define TEST_HOMEPAGE "https://dana.example/profile?tags=x-files,ufo"

/* The calendar and free/busy addresses. EDS keeps them on CALURI and FBURL
 * (Calendar and Free/Busy in Evolution), JSContact in one `calendars` map with
 * a `kind` that neither line has. Both are set, to show that they do not swap. */
#define TEST_CALENDAR_URI "https://dana.example/cal/dana.ics"
#define TEST_FREEBUSY_URI "https://dana.example/fb/dana.ifb"

/* The spouse. EDS keeps it on an X-EVOLUTION-SPOUSE line (vCard 3.0 has no
 * RELATED). JSContact keeps it as the key of a `relatedTo` entry with the type
 * `spouse`. This tests that the Spouse field in Evolution is the line that the
 * emitter writes, and that the name becomes a `relatedTo` key on the server. */
#define TEST_SPOUSE "Fox Mulder"

/* The instant-messaging handle. EDS keeps it on an X-JABBER line, JSContact in
 * the `onlineServices` map. The per-slot field is set, as the Evolution contact
 * editor does. The slot selects the TYPE, and a line with no TYPE reaches no
 * field. This tests that real EDS reads the TYPE that the emitter writes. The
 * comma tests the escaping, because a JSContact `user` is free text. */
#define TEST_IM_HANDLE "dana,scully@jabber.example"

/* The categories. EDS keeps them as a list on one CATEGORIES line, JSContact
 * as the `keywords` set. The comma separates items, so the comma in the second
 * category tests that two categories do not become three. */
#define TEST_CATEGORY_ONE "Friends"
#define TEST_CATEGORY_TWO "beer, in Berlin"
/* The separator for the reported categories. The comma is under test, so it
 * cannot be the separator. */
#define TEST_CATEGORY_SEPARATOR "|"

/* The birthday. EDS keeps it in the structured E_CONTACT_BIRTH_DATE field,
 * JSContact as an `anniversaries` entry with a PartialDate. EDS rebuilds the
 * line from its field and drops the X-JMAP-KEY. This tests that the date
 * arrives on the correct entry through real EDS. */
#define TEST_BIRTH_YEAR 1964
#define TEST_BIRTH_MONTH 3
#define TEST_BIRTH_DAY 27

/* The media type of the photo. The bytes come from the command line. EDS
 * makes the `TYPE` parameter from the media type: libebook-contacts 3.52
 * writes the subtype only (`TYPE=png`) and rebuilds `image/png` when it reads.
 * This tests that the emitter and the reader agree with EDS. */
#define TEST_PHOTO_MEDIA_TYPE "image/png"

/* The separator for the reported NOTE line values. It must not occur in the
 * notes, and it must not be the semicolon or the comma that the notes test. */
#define TEST_NOTE_LINE_SEPARATOR "|"

/* How many times, and how often, wait_for_contact() asks for a seeded contact.
 * EBookMetaBackend loads an unknown contact from the backend, so the first try
 * usually succeeds. But a get during the refresh at connect time can read a
 * cache that is not full yet. The total is 10 seconds, less than the CTest
 * timeout, so a missing contact fails the test and does not hang it. */
#define EDIT_WAIT_TRIES 200
#define EDIT_WAIT_INTERVAL_US 50000

/* Report one observation with line breaks in its value. The test reads one
 * `key=value` per line, so each newline becomes the two characters "\n". The
 * remaining text stays as EDS gave it. */
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

/* Report the given name and family name that EDS parses from the N line. Both
 * are synthetic fields, so a save gives them back to the backend. The given
 * name can hold several JSContact components of one kind, joined. */
static void
report_name_fields (const gchar *prefix,
                    EContact *contact)
{
	const gchar *given = e_contact_get_const (contact, E_CONTACT_GIVEN_NAME);
	const gchar *family = e_contact_get_const (contact, E_CONTACT_FAMILY_NAME);

	g_print ("%s-given-name=%s\n", prefix, given ? given : "");
	g_print ("%s-family-name=%s\n", prefix, family ? family : "");
}

/* Report the photo that EDS has for a contact.
 *
 * `type` is the EDS result for the line: `inlined` for embedded bytes, `uri`
 * for a reference, and `absent` if EDS can make neither.
 *
 * A photo read back through a book is usually `uri`. `EBookMetaBackend` sends
 * each cached contact through `store_inline_photos`, which writes the bytes to
 * a file in the book cache directory and points the line at it. Thus the bytes
 * are reported for both kinds, because the test compares bytes, not form. */
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

	/* Read only a local file, which is what EDS writes. Do not fetch a
	 * network URI from the server. */
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

/* Report the two calendar addresses that EDS has for a contact, each from its
 * own field. Both are reported together, because the failure to find is a swap.
 * An unset field gives the empty string. */
static void
report_calendars (const gchar *prefix,
                  EContact *contact)
{
	const gchar *calendar = e_contact_get_const (contact, E_CONTACT_CALENDAR_URI);
	const gchar *freebusy = e_contact_get_const (contact, E_CONTACT_FREEBUSY_URL);

	g_print ("%s-calendar-uri=%s\n", prefix, calendar ? calendar : "");
	g_print ("%s-freebusy-uri=%s\n", prefix, freebusy ? freebusy : "");
}

/* Report the spouse that EDS has for a contact. EDS gives only the first
 * X-EVOLUTION-SPOUSE line, so a second line shows only on the server. An unset
 * field gives the empty string. */
static void
report_spouse (const gchar *prefix,
               EContact *contact)
{
	const gchar *spouse = e_contact_get_const (contact, E_CONTACT_SPOUSE);

	g_print ("%s-spouse=%s\n", prefix, spouse ? spouse : "");
}

/* Report the X-EVOLUTION-SPOUSE line itself. e_contact_get() gives "no spouse"
 * for an empty line and for no line, and report_spouse() cannot tell them apart.
 *
 * libebook-contacts decides which of the two a cleared field gives, and the
 * `unspouse` phase examines it. The mapping removes the spouse in both cases,
 * so this is only reported. The test checks that the field is empty. */
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

	/* NULL if the attribute has no value. This is different from an empty
	 * string. */
	value = e_vcard_attribute_get_value (attribute);
	g_print ("%s-spouse-line-value=%s\n", prefix, value ? value : "");
	g_free (value);
}

/* Report the note that EDS has for a contact, and the number of NOTE lines.
 *
 * e_contact_get() gives only the first NOTE line, so the count is necessary. It
 * shows a set that replaced all NOTE lines or added a line. The test checks the
 * number. An unset field gives the empty string. */
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

		/* RFC 2426 §5 compares property names case-insensitively, so
		 * `Note` also counts. */
		if (name && g_ascii_strcasecmp (name, EVC_NOTE) == 0)
			lines++;
	}

	g_print ("%s-note-lines=%u\n", prefix, lines);
}

/* Report the value of each NOTE line, joined with TEST_NOTE_LINE_SEPARATOR.
 *
 * As with report_spouse_line(), e_contact_get() reads only the first line. An
 * emptied first note and a deleted second note give the same field and the
 * same count. Thus the `unnote` phase reports the values.
 *
 * libebook-contacts decides if it removes an emptied line or keeps it with no
 * value. A line with no value gives the empty string, and the test compares
 * only the non-empty values. */
static void
report_note_line_values (const gchar *prefix,
                         EContact *contact)
{
	GPtrArray *values = g_ptr_array_new_with_free_func (g_free);
	GList *attribute;
	gchar *joined;
	gchar *key;

	for (attribute = e_vcard_get_attributes (E_VCARD (contact));
	     attribute;
	     attribute = attribute->next) {
		const gchar *name = e_vcard_attribute_get_name (attribute->data);
		gchar *value;

		if (!name || g_ascii_strcasecmp (name, EVC_NOTE) != 0)
			continue;

		/* NULL if the attribute has no value, which a cleared field
		 * usually gives. This is different from an empty string. */
		value = e_vcard_attribute_get_value (attribute->data);
		g_ptr_array_add (values, value ? value : g_strdup (""));
	}

	g_ptr_array_add (values, NULL);
	joined = g_strjoinv (TEST_NOTE_LINE_SEPARATOR, (gchar **) values->pdata);
	key = g_strconcat (prefix, "-note-line-values", NULL);
	/* Escape newlines, because a note can contain line breaks. */
	report_multiline (key, joined);

	g_free (key);
	g_free (joined);
	g_ptr_array_free (values, TRUE);
}

/* Report the instant-messaging handle from the per-slot field that the
 * Evolution contact editor writes, the number of X-JABBER lines, and the
 * X-JMAP-KEY of the first line.
 *
 * The field is what the user sees. E_CONTACT_IM_JABBER_HOME_1 is the HOME
 * slot, so a handle from a server URI shows only if the reader writes a line
 * with TYPE=HOME.
 *
 * e_contact_get() gives only the first X-JABBER line of the slot, so the count
 * shows a set that added a line and did not replace the old one.
 *
 * The X-JMAP-KEY identifies the server entry of the line. EDS drops parameters
 * when it rebuilds a line (as for PHOTO), and only real EDS shows if a set on
 * this field does the same. The save works with a key or without one, so this
 * is only reported. The test checks the result on the server. */
static void
report_im_handle (const gchar *prefix,
                  EContact *contact)
{
	const gchar *handle = e_contact_get_const (contact, E_CONTACT_IM_JABBER_HOME_1);
	EVCardAttribute *attribute;
	GList *attributes;
	GList *key;
	guint lines = 0;

	g_print ("%s-im-handle=%s\n", prefix, handle ? handle : "");

	for (attributes = e_vcard_get_attributes (E_VCARD (contact));
	     attributes;
	     attributes = attributes->next) {
		const gchar *name = e_vcard_attribute_get_name (attributes->data);

		/* Case-insensitive, as in report_notes() (RFC 2426 §5). */
		if (name && g_ascii_strcasecmp (name, EVC_X_JABBER) == 0)
			lines++;
	}

	g_print ("%s-im-handle-lines=%u\n", prefix, lines);

	attribute = e_vcard_get_attribute (E_VCARD (contact), EVC_X_JABBER);
	key = attribute ? e_vcard_attribute_get_param (attribute, "X-JMAP-KEY") : NULL;
	g_print ("%s-im-handle-key=%s\n", prefix,
		 key && key->data ? (const gchar *) key->data : "");
}

/* The `write` phase: add one contact with all mapped properties to an empty
 * book, then read it back. */
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

	/* An inlined photo, as the Evolution contact editor writes for an image
	 * file. Only this form has a media type. EDS copies the data, so free
	 * the decoded bytes after the call. */
	photo_bytes = g_base64_decode (photo_base64, &photo_length);
	photo.type = E_CONTACT_PHOTO_TYPE_INLINED;
	photo.data.inlined.mime_type = (gchar *) TEST_PHOTO_MEDIA_TYPE;
	photo.data.inlined.data = photo_bytes;
	photo.data.inlined.length = photo_length;
	e_contact_set (contact, E_CONTACT_PHOTO, &photo);
	g_free (photo_bytes);

	/* Set the list, not the comma-joined E_CONTACT_CATEGORIES string. EDS
	 * splits that string at each comma, which would make TEST_CATEGORY_TWO
	 * two categories. The Evolution Categories dialog also writes the list. */
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

	/* Read from the meta backend cache, not from the server: EDS must keep
	 * what it wrote. */
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
	/* Read from the slot that was written. A line with no TYPE is in the
	 * vCard but not in this field. */
	g_print ("read-back-jabber=%s\n",
		 (const gchar *) e_contact_get_const (read_back, E_CONTACT_IM_JABBER_HOME_1));

	/* A boxed struct. Report each field, because the test checks which
	 * field each part goes into. */
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

	/* Also boxed. Report the text from e_contact_date_to_string(), which
	 * has the same format as the date line. */
	read_back_birthday = e_contact_get (read_back, E_CONTACT_BIRTH_DATE);
	#if EDS_CHECK_VERSION(3, 59, 1)
	read_back_birthday_text = read_back_birthday ? e_contact_date_to_string (read_back_birthday, E_VCARD_VERSION_30) : NULL;
	#else
	read_back_birthday_text = read_back_birthday ? e_contact_date_to_string (read_back_birthday) : NULL;
	#endif
	g_print ("read-back-birthday=%s\n", read_back_birthday_text ? read_back_birthday_text : "");
	g_free (read_back_birthday_text);
	if (read_back_birthday)
		e_contact_date_free (read_back_birthday);

	/* A list. Report it joined, so that the test sees the count and the
	 * values. A category split at its comma shows as an extra item. */
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

	/* The photo from the EDS cache, as the backend wrote it on the `PHOTO`
	 * line after the add. */
	report_photo ("read-back", read_back);

	/* The LABEL line is a separate field. Escape it, because a label has
	 * several lines. */
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

/* Ask for one contact until EDS has it, or until EDIT_WAIT_TRIES is reached.
 * Each error is cleared and the get tries again. Returns NULL on timeout. */
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

/* Report what EDS reads from the seeded card, before a change. The name
 * fields are necessary: the backend save compares against them to learn if
 * the user changed a field. */
static void
report_seeded_contact (EContact *contact)
{
	g_print ("read-full-name=%s\n",
		 (const gchar *) e_contact_get_const (contact, E_CONTACT_FULL_NAME));
	g_print ("read-email=%s\n",
		 (const gchar *) e_contact_get_const (contact, E_CONTACT_EMAIL_1));
	report_name_fields ("read", contact);
	/* The photo from the server, as EDS reads the line that the emitter
	 * wrote. The `write` phase cannot examine this direction. */
	report_photo ("read", contact);
	/* The calendar addresses, for the same reason. */
	report_calendars ("read", contact);
	report_spouse ("read", contact);
	/* The notes. The card can have more notes than the user sees, so the
	 * line count is also reported. */
	report_notes ("read", contact);
	/* The instant-messaging handle. The server can give it as a URI, from
	 * which the reader must extract the handle. */
	report_im_handle ("read", contact);
}

/* Save an edited contact and report what EDS has for it after the save. Takes
 * ownership of `contact`.
 *
 * The read-back is a new get, so it reports the EDS cache copy that the
 * backend wrote after the save, not the copy from this program. */
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
	/* The photo after the save. For a phase that does not change the photo,
	 * this shows that the backend did not change it. */
	report_photo ("read-back", read_back);
	report_calendars ("read-back", read_back);
	report_spouse ("read-back", read_back);
	report_notes ("read-back", read_back);
	report_im_handle ("read-back", read_back);
	g_object_unref (read_back);

	return 0;
}

/* The `edit` phase: read a contact from the server, change one field that is
 * not the name, and save it.
 *
 * The test checks the name, which does not change. A JSContact name can have
 * several components of one kind, but the N line has one field per kind. So
 * EDS gets the components joined into one string. The backend restores them
 * if the field is still equal to the joined parts. Only real EDS can test
 * that string comparison. */
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

	/* Change one field that is not the name. Thus any change to the name is
	 * an error. */
	e_contact_set (contact, E_CONTACT_EMAIL_1, email);

	return save_and_report (book, contact, uid);
}

/* The `rename` phase: the opposite of `edit`, on the same seeded card. The user
 * changes the given name, which holds the two parts of a double given name.
 * The separate parts from the server must NOT come back.
 *
 * The new text does not show which part it replaced, so the field becomes one
 * component. EDS rebuilds the N line from its synthetic fields, so the save
 * compares against the EDS text, not text from this repository.
 *
 * The Evolution contact editor writes both fields: the given name, and the FN
 * line, which is a separate attribute. */
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

/* The `repicture` phase: the user selects a new photo on the same seeded card.
 *
 * `e_contact_set` rewrites the `PHOTO` line and drops its parameters, which
 * include the `X-JMAP-KEY`. Thus the save cannot find the entry by its key and
 * must pair the new photo with the old one. EDS gives a save the cached card,
 * and only real EDS shows if that line keeps its key.
 *
 * The photo is inlined, as the Evolution contact editor writes for an image
 * file. */
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

/* The `recalendar` phase: the user changes the calendar address, on a card
 * with two calendar resources from the server.
 *
 * Unlike `repicture`, E_CONTACT_CALENDAR_URI is a plain vCard attribute, not a
 * synthetic field. A set changes the value of the first CALURI line and keeps
 * its parameters, X-JMAP-KEY included. Thus the save can patch by key. This
 * phase tests that claim about libebook-contacts through the real daemons.
 *
 * Only the calendar is set. The free/busy address shows that the patch
 * changed one entry, not the full map. */
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

/* The `respouse` phase: the user changes the spouse, on a card that the server
 * relates to two people.
 *
 * The line shows the `relatedTo` key, so there is no X-JMAP-KEY. The question
 * is if a set leaves one X-EVOLUTION-SPOUSE line or two. E_CONTACT_SPOUSE is a
 * plain vCard attribute, so a set must change the first line. If it adds a
 * line, the mapping sees two spouses. e_contact_get() gives only the first
 * line, so the test checks the result on the server.
 *
 * Only the spouse is set. The server also relates the card to a brother, who
 * has no line. He shows that the save removes one spouse and not all
 * relations. */
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

/* The `unspouse` phase: the user empties the Spouse field on the same seeded
 * card.
 *
 * The backend gets a card with no relations and must remove the spouse with no
 * replacement. The brother has no line. A save that removed all of `relatedTo`
 * would delete a relation that the user did not see.
 *
 * Set the empty string, not NULL, as the Evolution contact editor does for an
 * empty entry. report_spouse_line() reports what EDS does to the line. */
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

	/* Before the modify: report the field and the line. */
	report_spouse ("cleared", contact);
	report_spouse_line ("cleared", contact);

	return save_and_report (book, contact, uid);
}

/* The `renote` phase: the user changes the note, on a card with two notes from
 * the server.
 *
 * E_CONTACT_NOTE is the first NOTE line, and each `notes` entry writes a line.
 * The UI does not show the second note. This phase tests two things: that a set
 * changes the first line and keeps its X-JMAP-KEY, and that the second line
 * stays. A set that removed all NOTE lines would delete a note that the user
 * did not see.
 *
 * Only the note is set. The other properties show that the patch changed one
 * entry, not the full map. */
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

	/* Before the modify: report the field and the NOTE line count. */
	report_notes ("retyped", contact);

	return save_and_report (book, contact, uid);
}

/* The `unnote` phase: the user empties the Notes field on the same seeded card.
 *
 * The save must remove one map entry through a field that shows only one
 * entry. The second note has its own line. A save that removed all of `notes`
 * would delete a note that the user did not see.
 *
 * Set the empty string, not NULL, as the Evolution contact editor does for an
 * empty field. The field alone cannot show which note was cleared, so
 * report_note_line_values() also reports each NOTE value. */
static int
unnote_phase (EBookClient *book,
              const gchar *uid)
{
	EContact *contact;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	e_contact_set (contact, E_CONTACT_NOTE, "");

	/* Before the modify: report the field, the NOTE line count and each
	 * value. */
	report_notes ("cleared", contact);
	report_note_line_values ("cleared", contact);

	return save_and_report (book, contact, uid);
}

/* The `rehandle` phase: the user changes an instant-messaging handle that the
 * server gives only as a URI.
 *
 * RFC 9553 §2.3.2 lets an entry give a `user`, a `uri`, or both. The Evolution
 * fields hold a handle. So the reader extracts the handle from an `xmpp:` URI
 * that contains only the JID. The save rebuilds the URI from the changed
 * handle.
 *
 * Only real EDS can show two things. First, that the extracted handle reaches
 * the field: the reader selects the slot through the TYPE, and the HOME slot
 * of E_CONTACT_IM_JABBER_HOME_1 must read it. Second, what a set leaves on the
 * card: one line or two, with or without the X-JMAP-KEY. That decides if the
 * save can patch by key. report_im_handle() reports both, and the test checks
 * the result on the server.
 *
 * Set the per-slot field, as the Evolution contact editor and the `write`
 * phase do. */
static int
rehandle_phase (EBookClient *book,
                const gchar *uid,
                const gchar *handle)
{
	EContact *contact;

	contact = wait_for_contact (book, uid);
	if (!contact) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	report_seeded_contact (contact);

	e_contact_set (contact, E_CONTACT_IM_JABBER_HOME_1, handle);

	/* Before the modify: report the field, the X-JABBER line count and the
	 * key on the first line. */
	report_im_handle ("retyped", contact);

	return save_and_report (book, contact, uid);
}

/* The `remove` phase: the user deletes a seeded card.
 *
 * This phase does not save an edit. It removes the card through
 * `e_book_client_remove_contact_by_uid_sync`, which reaches the
 * `remove_contact_sync` vfunc. `cal-client.c` does a similar delete of a
 * recurring occurrence for the calendar. The phase waits for the contact
 * first, as all seeded-card phases do, so a slow cache fill does not look like
 * a missing card. It reports the result of the call and if the EDS cache still
 * has the card. A backend that reports success but does not send the destroy
 * still shows the card on the next get. */
static int
remove_phase (EBookClient *book,
             const gchar *uid)
{
	GError *error = NULL;
	EContact *read_back = NULL;

	if (!wait_for_contact (book, uid)) {
		g_printerr ("wait: EDS never produced the contact '%s'\n", uid);
		return 1;
	}

	if (!e_book_client_remove_contact_by_uid_sync (book, uid, E_BOOK_OPERATION_FLAG_NONE,
						       NULL, &error))
		return fail ("remove", error);

	g_print ("removed=1\n");

	/* A removed contact gives `E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND`. Any
	 * other result, success included, shows that the cache still has it. */
	if (e_book_client_get_contact_sync (book, uid, &read_back, NULL, &error)) {
		g_object_unref (read_back);
		g_print ("gone=0\n");
		return 0;
	}

	g_print ("gone=%d\n",
		 g_error_matches (error, E_BOOK_CLIENT_ERROR,
				  E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND) ? 1 : 0);
	g_print ("gone-error-message=%s\n", error ? error->message : "");
	g_clear_error (&error);

	return 0;
}

/* Lists all contacts in the book, sorted by full name. The sort removes the
 * order difference between the full query of `list_existing_sync` and the
 * delta of `get_changes_sync`, because the test checks the cache contents
 * only. `rust/crates/jmap-functional/tests/book-changes.rs` and the
 * live-stalwart-book tests use this phase. */
static int
list_phase (EBookClient *book)
{
	GSList *contacts = NULL, *link;
	GError *error = NULL;
	EBookQuery *query;
	gchar *query_string;
	GList *names = NULL, *name_link;
	guint index = 0;

	query = e_book_query_any_field_contains ("");
	query_string = e_book_query_to_string (query);
	e_book_query_unref (query);

	if (!e_book_client_get_contacts_sync (book, query_string, &contacts, NULL, &error)) {
		g_free (query_string);
		return fail ("list", error);
	}
	g_free (query_string);

	for (link = contacts; link; link = link->next) {
		EContact *contact = link->data;
		const gchar *name = e_contact_get_const (contact, E_CONTACT_FULL_NAME);

		names = g_list_insert_sorted (names, g_strdup (name ? name : ""),
					      (GCompareFunc) g_strcmp0);
	}

	g_print ("contacts=%u\n", g_slist_length (contacts));
	for (name_link = names; name_link; name_link = name_link->next, index++)
		g_print ("contact-%u=%s\n", index, (const gchar *) name_link->data);

	g_list_free_full (names, g_free);
	g_slist_free_full (contacts, g_object_unref);

	return 0;
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
		    "       %s <source-uid> renote <contact-uid> <note>\n"
		    "       %s <source-uid> unnote <contact-uid>\n"
		    "       %s <source-uid> rehandle <contact-uid> <handle>\n"
		    "       %s <source-uid> remove <contact-uid>\n"
		    "       %s <source-uid> list\n",
		    program, program, program, program, program, program, program,
		    program, program, program, program, program);
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
	      (g_str_equal (phase, "renote") && argc == 5) ||
	      (g_str_equal (phase, "unnote") && argc == 4) ||
	      (g_str_equal (phase, "rehandle") && argc == 5) ||
	      (g_str_equal (phase, "remove") && argc == 4) ||
	      (g_str_equal (phase, "list") && argc == 3))) {
		usage (argv[0]);
		return 2;
	}

	/* Starts evolution-source-registry on the session bus. It reads the
	 * temporary sources directory that the test wrote. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	source = e_source_registry_ref_source (registry, source_uid);
	if (!source) {
		g_printerr ("registry: no source with UID '%s'\n", source_uid);
		return 1;
	}

	/* A real server needs a password, and this program has no GUI prompt.
	 * Store it where EDS looks for Basic auth, so that a keyfile with a real
	 * `User=` connects on the first attempt and does not fail with
	 * `CredentialsRequired`. Tests against the mock do not set it. */
	{
		const gchar *seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

		if (seed_password && *seed_password &&
		    !e_source_store_password_sync (source, seed_password, TRUE, NULL, &error))
			return fail ("store-password", error);
	}

	/* Starts evolution-addressbook-factory. It dlopens
	 * libebookbackendjmap.so from EDS_ADDRESS_BOOK_MODULES and selects the
	 * factory for the BackendName of the keyfile. A failure here usually
	 * comes from one of these two steps, not from the backend.
	 *
	 * (guint32) -1 tells EDS not to wait for connected. The status is
	 * checked below, but this wait cannot work in a program with no main
	 * loop (see connection-status.c). The open is synchronous, so its
	 * results are complete when this call returns. */
	client = e_book_client_connect_sync (source, (guint32) -1, NULL, &error);
	if (!client)
		return fail ("connect", error);

	book = E_BOOK_CLIENT (client);

	/* The EDS result of the connect, with a correct wait. */
	functional_report_connection_status (source, 10);

	/* Read the properties over D-Bus, not from the client cache. EClient
	 * updates the cache from notifications on a main context. This program
	 * has no main loop, so the cache can be out of date. */
	if (!e_client_retrieve_properties_sync (client, NULL, &error))
		return fail ("retrieve-properties", error);

	/* Report if the book accepts writes. EDS gets this from the backend at
	 * connect. A backend that does not report writable gives a read-only
	 * book, and a write then fails only with "Permission denied". This
	 * line lets the test identify the cause. */
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
	else if (g_str_equal (phase, "renote"))
		status = renote_phase (book, argv[3], argv[4]);
	else if (g_str_equal (phase, "unnote"))
		status = unnote_phase (book, argv[3]);
	else if (g_str_equal (phase, "rehandle"))
		status = rehandle_phase (book, argv[3], argv[4]);
	else if (g_str_equal (phase, "list"))
		status = list_phase (book);
	else
		status = remove_phase (book, argv[3]);

	g_object_unref (client);
	g_object_unref (source);
	g_object_unref (registry);

	return status;
}
