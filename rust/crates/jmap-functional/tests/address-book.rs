// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Functional test of the address book: `evolution-addressbook-factory` loads
//! `libebookbackendjmap.so`, opens a book from a `.source` keyfile, and sends
//! writes through it to the mock JMAP server.
//!
//! Each assertion examines one of the two ends. The client program reports what
//! EDS gave a libebook consumer. The mock reports what the backend asked the
//! server to do. The two ends do not know about each other, thus an assertion
//! that is true at both ends is a claim about the full path.
//!
//! There are twelve legs, each with its own book. The first leg starts with an
//! empty book and writes a contact into it. Ten legs start from a card that the
//! test seeds into the mock before EDS connects. That card has a shape that no
//! vCard can state, and only real EDS can show what it does to that shape. The
//! ten legs do these edits: edit a field next to the name, retype the name,
//! select a new picture, retype the calendar address, retype the spouse, clear
//! the spouse, retype the note on a card with two notes, clear that note, clear
//! the note on a card with one note, and retype an instant-messaging handle
//! that the server stated as a URI. The last leg removes a contact.

use std::collections::BTreeMap;

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_proto::Id;
use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, Media, Name, NameComponent,
    Note, OnlineService, OrgUnit, Organization, Relation, Title,
};

/// The contact that the client writes. The same constant goes on the client's
/// command line and into the check on the mock's store.
const FULL_NAME: &str = "Dana Scully";
const EMAIL: &str = "dana@example.com";
/// The nickname, as `book-client.c` writes it. RFC 2426 §3.1.3 states nicknames
/// as a comma-separated list. This value makes sure that the comma does not
/// split one nickname into two entries on the server.
const NICKNAME: &str = "Vee, the tall one";
/// The employer and the department, as `book-client.c` writes them. They are
/// one `ORG` line. Only real EDS can show that its two fields and the JSContact
/// `organizations` map agree.
const ORG: &str = "Acme Ltd";
const ORG_UNIT: &str = "Research";
/// The job title and the role, as `book-client.c` writes them. vCard puts them
/// on `TITLE` and `ROLE`, and JSContact puts both in the `titles` map. This leg
/// shows that the `kind` that tells them apart survives real EDS.
const TITLE: &str = "Research Scientist";
const ROLE: &str = "Project Lead";
/// The postal address, as `book-client.c` writes it. EDS keeps it in the fields
/// of one `ADR` line, and JSContact keeps it as a list of named components.
/// This leg checks which field means which kind against the EDS that writes the
/// line.
const STREET: &str = "Hauptstrasse 1";
const LOCALITY: &str = "Berlin";
const POSTCODE: &str = "10115";
const COUNTRY: &str = "Germany";
/// The same address formatted for an envelope. EDS keeps it on a `LABEL` line,
/// and JSContact keeps it as the address's `full`.
///
/// `E_CONTACT_ADDRESS_LABEL_WORK` is a synthetic field, so the `X-JMAP-KEY`
/// that links the label to its address does not survive EDS. Only the shared
/// `TYPE` shows that the label and the `ADR` are one address. If the pairing
/// fails, the server gets two addresses instead of one.
const ADDRESS_LABEL: &str = "Hauptstrasse 1\n10115 Berlin\nGermany";
/// The free-text note, as `book-client.c` writes it. vCard gives the semicolon
/// and the comma a structural meaning. This leg checks that EDS removes the
/// escapes that the emitter adds.
const NOTE: &str = "met at FOSDEM; owes me a beer, apparently";
/// The home page, as `book-client.c` writes it. EDS keeps it on the first `URL`
/// line, and JSContact keeps it in the `links` map. EDS escapes the comma in
/// the query string. This leg checks that the server gets the URI that the user
/// typed: not cut at the comma, and without the backslash that EDS wrote.
const HOMEPAGE: &str = "https://dana.example/profile?tags=x-files,ufo";
/// The contact's calendar and free/busy URIs, as `book-client.c` writes them.
/// EDS keeps them on `CALURI` and `FBURL`. JSContact keeps both in the
/// `calendars` map, and only a `kind` tells them apart. This leg checks that
/// each URI goes back to the correct kind. If not, Evolution shows each URI in
/// the field of the other resource.
const CALENDAR_URI: &str = "https://dana.example/cal/dana.ics";
const FREEBUSY_URI: &str = "https://dana.example/fb/dana.ifb";
/// The contact's spouse, as `book-client.c` writes it. EDS keeps it on an
/// `X-EVOLUTION-SPOUSE` line, because vCard 3.0 has no `RELATED`. JSContact
/// keeps it as the *key* of a `relatedTo` entry with the type `spouse`. This
/// leg checks that the name gets to the server as the key of the entry, not as
/// a value.
const SPOUSE: &str = "Fox Mulder";
/// The instant-messaging handle, as `book-client.c` writes it. EDS keeps it on
/// an `X-JABBER` line, and JSContact keeps it as an `onlineServices` entry.
/// This leg checks two things against real EDS:
/// - EDS reads the handle back from the `TYPE` that the emitter writes. A line
///   without a `TYPE` goes to no field.
/// - The comma in the handle survives. A JSContact `user` is free text, but
///   vCard gives the comma a structural meaning.
const IM_HANDLE: &str = "dana,scully@jabber.example";
/// The service of that handle, as the mapping table writes it. The line name
/// `X-JABBER` identifies the service.
const IM_SERVICE: &str = "Jabber";
/// The two categories of the contact, as `book-client.c` writes them, and the
/// separator that the client uses when it reports them. EDS keeps them as a
/// list on one `CATEGORIES` line, and JSContact as the `keywords` set. This leg
/// checks a count: two categories must be two members. The comma in the second
/// category splits it into two if the escaping fails.
const CATEGORY_ONE: &str = "Friends";
const CATEGORY_TWO: &str = "beer, in Berlin";
const CATEGORY_SEPARATOR: &str = "|";
/// The birthday, as `book-client.c` writes it. This is the text that
/// `e_contact_date_to_string()` makes from the three numbers that the client
/// sets, and the text on the `BDAY` line. This is the one mapped property whose
/// value changes shape: three numbers in EDS, a date in JSContact.
const BIRTHDAY: &str = "1964-03-27";
const BIRTH_YEAR: u64 = 1964;
const BIRTH_MONTH: u64 = 3;
const BIRTH_DAY: u64 = 27;
/// The contact's picture: the bytes of a real 69-byte PNG (one red pixel). The
/// client gets it on its command line, thus the two ends cannot disagree about
/// the bytes.
///
/// The bytes are not valid UTF-8, so calcard returns them as binary, not as
/// text. The slash in the base64 must go through a `data:` URI unchanged. The
/// value is long enough that EDS folds the `PHOTO` line into two lines.
const PHOTO_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";
/// The media type of the picture, as `book-client.c` writes it. EDS writes only
/// the subtype on the line (`TYPE=png`, measured against libebook-contacts
/// 3.52) and makes `image/png` again when it reads the line. This leg checks
/// from both ends that the parameter the emitter writes is the media type that
/// EDS means.
const PHOTO_MEDIA_TYPE: &str = "image/png";
/// The picture that the user selects in the leg that replaces the picture: a
/// second real PNG, one pixel of a different colour. The bytes are different
/// from [`PHOTO_BASE64`] but the length is the same. Thus a save that wrote
/// back the old picture fails, and a save that truncated the new picture also
/// fails.
const REPLACEMENT_PHOTO_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYPgPAAEDAQAIicLsAAAAAElFTkSuQmCC";

/// The keyfile from `docs/examples/jmap-mock.source`, with the mock's ephemeral
/// port. It is written here, not read from `docs/`, so that a change to the
/// documented recipe makes this test fail. `jmap-backend-book`'s `recipe.rs`
/// checks the documented file.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::AddressBook, port).to_string()
}

#[test]
fn evolution_opens_the_book_and_a_write_reaches_the_server() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // The keyfile has no `[Resource] Identity=`, so the backend asks the server
    // for the default address book of the account. Seed one, flagged default.
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        state
            .account_mut(&account_id)
            .expect("the mock's default account")
            .seed_address_book("Personal", true);
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/address-book"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(
        &client,
        &["jmap-functional", "write", FULL_NAME, PHOTO_BASE64],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Examine this before the exit status. A read-only book changes all later
    // failures into "Permission denied", which is about the connect, not about
    // the write. EDS gets this value from the backend during `connect_sync`. If
    // the backend does not declare the book writable, Evolution shows it as
    // read-only.
    //
    // If the client stopped before this step (for example, the module is not in
    // the factory's directory), the exit status shows the failure.
    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the book, with {}\n{report}",
            output.status
        )
    });
    // The connection status of the source. Evolution shows it as a connected
    // account, and EDS clients wait on it. `readonly=0` shows that the backend
    // declared the book writable. This shows that EDS also sees the connection
    // as connected, not `connecting` or `disconnected`. Assert it first, for
    // the reason that `calendar.rs` gives.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );

    assert_eq!(readonly, "0", "EDS opened the book read-only\n{report}");

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("contacts-before"),
        Some(&"0"),
        "a fresh cache against an empty address book should hold nothing\n{report}"
    );

    let added = seen
        .get("added")
        .unwrap_or_else(|| panic!("the client reported no added contact\n{report}"));
    assert!(
        !added.is_empty(),
        "EDS added a contact with no UID\n{report}"
    );

    // Read back through EDS: the data that the meta backend kept from the
    // write.
    assert_eq!(
        seen.get("read-back-full-name"),
        Some(&FULL_NAME),
        "the contact EDS handed back is not the one that went in\n{report}"
    );
    assert_eq!(
        seen.get("read-back-email"),
        Some(&EMAIL),
        "the contact EDS handed back lost its email address\n{report}"
    );
    assert_eq!(
        seen.get("read-back-org"),
        Some(&ORG),
        "the contact EDS handed back lost its employer\n{report}"
    );
    assert_eq!(
        seen.get("read-back-org-unit"),
        Some(&ORG_UNIT),
        "the contact EDS handed back lost its department\n{report}"
    );
    assert_eq!(
        seen.get("read-back-nickname"),
        Some(&NICKNAME),
        "the contact EDS handed back lost or split its nickname\n{report}"
    );
    assert_eq!(
        seen.get("read-back-title"),
        Some(&TITLE),
        "the contact EDS handed back lost its job title\n{report}"
    );
    assert_eq!(
        seen.get("read-back-role"),
        Some(&ROLE),
        "the contact EDS handed back lost its role\n{report}"
    );
    assert_eq!(
        seen.get("read-back-note"),
        Some(&NOTE),
        "the contact EDS handed back lost or mangled its note\n{report}"
    );
    assert_eq!(
        seen.get("read-back-homepage"),
        Some(&HOMEPAGE),
        "the contact EDS handed back lost or mangled its home page\n{report}"
    );
    // The two calendar fields. If they are swapped, the `kind` selected the
    // wrong line. A single-field assertion cannot find that failure.
    assert_eq!(
        seen.get("read-back-calendar-uri"),
        Some(&CALENDAR_URI),
        "the contact EDS handed back lost or moved its calendar address\n{report}"
    );
    assert_eq!(
        seen.get("read-back-freebusy-uri"),
        Some(&FREEBUSY_URI),
        "the contact EDS handed back lost or moved its free/busy address\n{report}"
    );
    assert_eq!(
        seen.get("read-back-spouse"),
        Some(&SPOUSE),
        "the contact EDS handed back lost or respelled its spouse\n{report}"
    );
    assert_eq!(
        seen.get("read-back-birthday"),
        Some(&BIRTHDAY),
        "the contact EDS handed back lost or moved its birthday\n{report}"
    );
    // Read from the same per-context slot that the client wrote. If it is
    // missing, the handle came back on a line without a `TYPE`.
    assert_eq!(
        seen.get("read-back-jabber"),
        Some(&IM_HANDLE),
        "the contact EDS handed back lost or cut off its Jabber handle\n{report}"
    );
    // Both categories, in the order of the EDS list, joined as the client joins
    // them. An extra item is a category split on its comma. A missing item is a
    // category that the round trip lost.
    let categories = [CATEGORY_ONE, CATEGORY_TWO].join(CATEGORY_SEPARATOR);
    assert_eq!(
        seen.get("read-back-categories"),
        Some(&categories.as_str()),
        "the contact EDS handed back lost or split its categories\n{report}"
    );
    for (field, expected) in [
        ("read-back-street", STREET),
        ("read-back-locality", LOCALITY),
        ("read-back-code", POSTCODE),
        ("read-back-country", COUNTRY),
    ] {
        assert_eq!(
            seen.get(field),
            Some(&expected),
            "the contact EDS handed back lost or misplaced its {field}\n{report}"
        );
    }
    // The picture from the EDS cache, and not as the bytes that went in.
    // `EBookMetaBackend` gives each cached contact to `store_inline_photos`,
    // which writes the picture to a file in the book's cache directory and
    // points the line at that file. Thus a libebook consumer reads a `file:`
    // URI. The check is that the file holds the bytes that went in.
    assert_eq!(
        seen.get("read-back-photo-type"),
        Some(&"uri"),
        "EDS did not cache the picture the way a meta backend caches one\n{report}"
    );
    let cached_photo = seen
        .get("read-back-photo-uri")
        .unwrap_or_else(|| panic!("the client reported no picture\n{report}"));
    assert!(
        cached_photo.starts_with("file://"),
        "EDS pointed the picture somewhere other than at its own cache: \
         {cached_photo}\n{report}"
    );
    // The file extension that EDS selected. It comes from the media type on the
    // line. If the `TYPE` was lost, the file gets a different suffix and
    // Evolution must guess the image type.
    assert!(
        cached_photo.ends_with(".png"),
        "EDS did not file the picture as the kind of image it is: \
         {cached_photo}\n{report}"
    );
    assert_eq!(
        seen.get("read-back-photo-file-base64"),
        Some(&PHOTO_BASE64),
        "the picture EDS cached is not the one that went in\n{report}"
    );
    // The client escapes the line breaks of this observation, because the test
    // reads the report one line at a time.
    let escaped_label = ADDRESS_LABEL.replace('\n', "\\n");
    assert_eq!(
        seen.get("read-back-address-label"),
        Some(&escaped_label.as_str()),
        "the contact EDS handed back lost or mangled its address label\n{report}"
    );
    assert_eq!(
        seen.get("contacts-after"),
        Some(&"1"),
        "the added contact is not in the book it was added to\n{report}"
    );

    // The other end: the requests that the server got. Do not assert the read
    // path here. `EBookMetaBackend` schedules its refresh, so
    // `ContactCard/query` may or may not have occurred yet. The write is
    // synchronous.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the write never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let account = state
        .account(&account_id)
        .expect("the mock's default account");
    let cards: Vec<_> = account.contact_cards.iter().collect();
    assert_eq!(
        cards.len(),
        1,
        "the server holds {} cards, not one",
        cards.len()
    );

    let (_, card) = cards[0];
    assert_eq!(
        card.name.as_ref().and_then(|name| name.full.as_deref()),
        Some(FULL_NAME),
        "the card on the server has the wrong name: {card:?}"
    );
    assert!(
        card.emails
            .as_ref()
            .is_some_and(|emails| emails.values().any(|email| email.address == EMAIL)),
        "the card on the server has no {EMAIL}: {card:?}"
    );
    assert!(
        card.address_book_ids
            .as_ref()
            .is_some_and(|books| books.values().any(|included| *included)),
        "the card on the server is in no address book: {card:?}"
    );
    // The ORG line on the server: one `organizations` entry. Its name is the
    // employer and its first unit is the department.
    let organization = card
        .organizations
        .as_ref()
        .and_then(|organizations| organizations.values().next())
        .unwrap_or_else(|| panic!("the card on the server has no organisation: {card:?}"));
    assert_eq!(organization.name.as_deref(), Some(ORG), "{card:?}");
    assert_eq!(
        organization
            .units
            .as_ref()
            .map(|units| units.iter().map(|unit| unit.name.as_str()).collect()),
        Some(vec![ORG_UNIT]),
        "{card:?}"
    );
    // The TITLE and the ROLE on the server: two entries of one `titles` map.
    // The `kind` shows the line that each entry came from.
    let titles = card
        .titles
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no titles: {card:?}"));
    // The default of RFC 9553 §2.2.5 is written here, not taken from the
    // mapping, so this end states the expected wire shape independently.
    let by_kind: Vec<(&str, &str)> = titles
        .values()
        .map(|title| {
            (
                title.kind.as_deref().unwrap_or("title"),
                title.name.as_str(),
            )
        })
        .collect();
    assert!(
        by_kind.contains(&("title", TITLE)),
        "the job title did not reach the server as a title: {by_kind:?}"
    );
    assert!(
        by_kind.contains(&("role", ROLE)),
        "the role did not reach the server as a role: {by_kind:?}"
    );
    // The ADR line on the server: one `addresses` entry whose components name
    // each field of the line. The kinds are written here, not taken from the
    // mapping, so this end states the expected wire shape independently.
    let addresses = card
        .addresses
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no address: {card:?}"));
    assert_eq!(
        addresses.len(),
        1,
        "the LABEL line was filed as an address of its own: {addresses:?}"
    );
    let address = addresses.values().next().expect("one address");
    assert_eq!(
        address.full.as_deref(),
        Some(ADDRESS_LABEL),
        "the written-out address did not reach the server: {card:?}"
    );
    let components: Vec<(&str, &str)> = address
        .components
        .iter()
        .flatten()
        .map(|component| (component.kind.as_str(), component.value.as_str()))
        .collect();
    assert_eq!(
        components,
        vec![
            ("name", STREET),
            ("locality", LOCALITY),
            ("postcode", POSTCODE),
            ("country", COUNTRY),
        ],
        "{card:?}"
    );
    // The client set the work address, so EDS wrote TYPE=WORK. On the server
    // that is the `contexts` member.
    assert_eq!(
        address.contexts,
        Some(serde_json::json!({"work": true})),
        "{card:?}"
    );
    // The NOTE line on the server: one `notes` entry with the text and its
    // punctuation. The client's read-back comes from the EDS cache, which
    // agrees with itself even if the line to the server was damaged. Thus check
    // here too.
    let notes = card
        .notes
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no notes: {card:?}"));
    assert_eq!(
        notes
            .values()
            .map(|note| note.note.as_str())
            .collect::<Vec<_>>(),
        vec![NOTE],
        "{card:?}"
    );
    // The NICKNAME line on the server: one `nicknames` entry with the full text
    // and its comma. Two entries show that the comma was read as the list
    // separator of RFC 2426 §3.1.3.
    let nicknames = card
        .nicknames
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no nicknames: {card:?}"));
    assert_eq!(
        nicknames
            .values()
            .map(|nickname| nickname.name.as_str())
            .collect::<Vec<_>>(),
        vec![NICKNAME],
        "{card:?}"
    );
    // The URL line on the server: one `links` entry with the full URI and no
    // `kind`, because a `URL` states none. A URI that stops at the comma shows
    // that the escaping failed between the EDS writer and the reader.
    let links = card
        .links
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no links: {card:?}"));
    assert_eq!(links.len(), 1, "{links:?}");
    let link = links.values().next().expect("one link");
    assert_eq!(link.uri, HOMEPAGE, "{card:?}");
    assert_eq!(link.kind, None, "{card:?}");
    // The CALURI and FBURL lines on the server: two `calendars` entries, each
    // with its kind. The kinds are written here, not taken from the mapping.
    // Assert them as a pair: a mapping that put both URIs under one kind still
    // gives two entries with the correct two URIs.
    let calendars = card
        .calendars
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no calendars: {card:?}"));
    let mut stated: Vec<(Option<&str>, &str)> = calendars
        .values()
        .map(|calendar| (calendar.kind.as_deref(), calendar.uri.as_str()))
        .collect();
    stated.sort();
    assert_eq!(
        stated,
        vec![
            (Some("calendar"), CALENDAR_URI),
            (Some("freeBusy"), FREEBUSY_URI),
        ],
        "{card:?}"
    );
    // The X-EVOLUTION-SPOUSE line on the server: one `relatedTo` entry, keyed
    // by the name that the user typed, with the type `spouse`. Both parts are
    // necessary. A name as a value under a key that the mapping made up is an
    // entity that the server does not know. A correct key with no type is the
    // relation of no kind of RFC 9555 §2.9.5, which does not state a marriage.
    let related = card
        .related_to
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server relates to nobody: {card:?}"));
    assert_eq!(
        related.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SPOUSE],
        "the spouse did not reach the server as the entry's own key: {card:?}"
    );
    assert_eq!(
        related[SPOUSE].relation,
        Some([("spouse".to_owned(), serde_json::json!(true))].into()),
        "{card:?}"
    );
    // The X-JABBER line on the server: one `onlineServices` entry with the
    // service and the handle. The client's read-back comes from the EDS cache,
    // which agrees with itself even if the handle got to the server cut at the
    // comma. The handle must be in `user`: a `uri` claims that the handle is an
    // RFC 3986 URI.
    let services = card
        .online_services
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no onlineServices: {card:?}"));
    let service = services.values().next().expect("one online service");
    assert_eq!(services.len(), 1, "{card:?}");
    assert_eq!(service.service.as_deref(), Some(IM_SERVICE), "{card:?}");
    assert_eq!(service.user.as_deref(), Some(IM_HANDLE), "{card:?}");
    assert_eq!(service.uri, None, "{card:?}");
    // The CATEGORIES line on the server: two `keywords` members, each `true`.
    // Three members show that the comma in the second category was read as the
    // list separator of RFC 2426 §3.6.1. The client's read-back comes from the
    // EDS cache, which agrees with itself even if the line to the server was
    // split. Thus check here too.
    let keywords = card
        .keywords
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no keywords: {card:?}"));
    assert_eq!(
        keywords.keys().map(String::as_str).collect::<Vec<_>>(),
        // Sorted in byte order, as the map keeps them: capital `F` comes before
        // lower-case `b`, not in the order that the client set them.
        vec![CATEGORY_ONE, CATEGORY_TWO],
        "{card:?}"
    );
    assert!(
        keywords.values().all(|set| *set == serde_json::json!(true)),
        "{card:?}"
    );
    // The PHOTO line on the server: one `media` entry of kind `photo` whose URI
    // holds the bytes. The client's read-back comes from the EDS cache, which
    // agrees with itself even if the server got the picture truncated at the
    // fold or encoded differently.
    //
    // The URI is written here, not taken from the mapping. It is an RFC 2397
    // `data:` URI with the media type before the payload, where a server that
    // does not know vCard reads the type of the bytes.
    let media = card
        .media
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no media: {card:?}"));
    assert_eq!(media.len(), 1, "{media:?}");
    let picture = media.values().next().expect("one media entry");
    assert_eq!(picture.kind.as_deref(), Some("photo"), "{card:?}");
    assert_eq!(
        picture.media_type.as_deref(),
        Some(PHOTO_MEDIA_TYPE),
        "{card:?}"
    );
    assert_eq!(
        picture.uri,
        format!("data:{PHOTO_MEDIA_TYPE};base64,{PHOTO_BASE64}"),
        "{card:?}"
    );
    // The BDAY line on the server: one `anniversaries` entry of kind `birth`
    // whose date is the three numbers that the client set. Written here, not
    // taken from the mapping.
    let anniversaries = card
        .anniversaries
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no anniversaries: {card:?}"));
    assert_eq!(anniversaries.len(), 1, "{anniversaries:?}");
    let anniversary = anniversaries.values().next().expect("one anniversary");
    assert_eq!(anniversary.kind, "birth", "{card:?}");
    assert_eq!(
        anniversary.date,
        Some(serde_json::json!({
            "@type": "PartialDate",
            "year": BIRTH_YEAR,
            "month": BIRTH_MONTH,
            "day": BIRTH_DAY,
        })),
        "{card:?}"
    );
}

/// The surname of the seeded card. Its given name has two parts, which RFC 9553
/// §2.2.1 states as **two** `given` components. The vCard `N` value has one
/// field for each kind, so the emitter joins them with a space and EDS gets
/// `N:Oldenburg;Jean Paul;;;`: one string for two parts on the server.
const EDITED_SURNAME: &str = "Oldenburg";
/// The two given-name parts, each with its pronunciation. A `phonetic` has no
/// `N` field, so the user cannot see or edit it. If it is gone, the save
/// deleted it.
const EDITED_GIVEN_PARTS: [(&str, &str); 2] = [("Jean", "zhon"), ("Paul", "pol")];
/// The given name that EDS must return: the two parts joined, byte for byte as
/// the emitter wrote them. The backend restores the components when the field
/// is equal to the joined parts, a *string* comparison against this text. If
/// EDS changed the whitespace, an unchanged name would look retyped and the
/// field's text would replace both parts.
const EDITED_GIVEN_FIELD: &str = "Jean Paul";
const EDITED_FULL_NAME: &str = "Jean Paul Oldenburg";
/// The one field that the user edits, before and after. It is not part of the
/// name, so any change that the save makes to the name is incorrect.
const EDITED_EMAIL_BEFORE: &str = "jp@example.com";
const EDITED_EMAIL_AFTER: &str = "jp@example.org";
/// The key that the *server* gave the seeded card's picture. The reader makes
/// keys by counting vCard entries (`m1`, `m2`, …), so a picture still under
/// this key after a save was paired with its line, not added again under a new
/// key.
///
/// Only the daemons can test this half of the picture mapping. EDS writes the
/// `PHOTO` line again, without the key, each time that the photo field is
/// *set*. Thus the need to restore the key depends on what real EDS does to a
/// card whose picture the user did not change.
const SEEDED_PHOTO_KEY: &str = "picture-1";
/// A logo on the seeded card under a key from the server. It has no vCard line
/// and must survive edits unchanged.
const SEEDED_LOGO_KEY: &str = "logo-1";
const SEEDED_LOGO_URI: &str = "https://oldenburg.example/logo.png";
/// The keys that the *server* gave the seeded card's two calendar resources,
/// and their URIs. Not like the keys that the reader makes (`c1`, `c2`, …), for
/// the same reason as [`SEEDED_PHOTO_KEY`].
const SEEDED_CALENDAR_KEY: &str = "calendar-1";
const SEEDED_FREEBUSY_KEY: &str = "freebusy-1";
const SEEDED_CALENDAR_URI: &str = "https://oldenburg.example/cal/jp.ics";
const SEEDED_FREEBUSY_URI: &str = "https://oldenburg.example/fb/jp.ifb";
/// The calendar preference from the server. A `CALURI` has no parameter for it,
/// so it stays in the entry's `extra`. It shows whether a save *patched* the
/// entry or replaced it: a replacement holds only the URI.
const SEEDED_CALENDAR_PREF: u32 = 1;
/// The seeded card's spouse according to the server. It is a key, not a value:
/// RFC 9553 §2.1.8 keys `relatedTo` by the related entity. It is free text, not
/// a `uid`, as RFC 9555 §2.9.5 allows, so a line can show the name.
const SEEDED_SPOUSE: &str = "Marie Oldenburg";
/// A second related person, of a type that Evolution has no field for. The
/// entry goes to no line, so the user cannot see or edit it, and a save that
/// deleted it is incorrect. It is the `relatedTo` equivalent of the `phonetic`,
/// and the reason the map has two entries.
const SEEDED_SIBLING: &str = "Klaus Oldenburg";
/// The keys that the *server* gave the seeded card's two notes. Evolution's
/// Notes field is the **first** `NOTE` line only. Thus a user edits one entry
/// of a map that they can see only partially. Only real EDS shows what a save
/// does to the part that the user cannot see.
const SEEDED_NOTE_KEY: &str = "note-1";
const SEEDED_SECOND_NOTE_KEY: &str = "note-2";
/// The semicolon and the comma are here for the same reason as in [`NOTE`]:
/// vCard gives both characters a structural meaning. A note on the server cut
/// at either character, or with the backslash that EDS wrote, fails the check.
const SEEDED_NOTE: &str = "met in Ghent; still owes me a beer, apparently";
const SEEDED_SECOND_NOTE: &str = "do not call before 10:00, ever";
/// The key, the service, and the URI of the seeded card's instant-messaging
/// handle. The important part is the shape: the entry states **only** a URI.
///
/// RFC 9553 §2.3.2 lets an `onlineServices` entry name the contact with a
/// `user`, a `uri`, or both. Evolution's instant-messaging fields hold a
/// handle. The `xmpp:` URI holds only the JID, so the reader can get
/// [`SEEDED_SERVICE_HANDLE`] from [`SEEDED_SERVICE_URI`].
///
/// Every seeded leg uses this card. The line that EDS returns states a `user`,
/// so a save that compares the *members* instead of the handle would write this
/// entry again each time the contact is saved.
const SEEDED_SERVICE_KEY: &str = "handle-1";
const SEEDED_SERVICE: &str = "Jabber";
const SEEDED_SERVICE_HANDLE: &str = "jp@jabber.example";
const SEEDED_SERVICE_URI: &str = "xmpp:jp@jabber.example";
const SEEDED_SKYPE_SERVICE_KEY: &str = "im-home-1";
const SEEDED_SKYPE_SERVICE: &str = "Skype";
const SEEDED_SKYPE_SERVICE_URI: &str = "skype:jp_skype";
const SEEDED_MATRIX_SERVICE_KEY: &str = "im-work-1";
const SEEDED_MATRIX_SERVICE: &str = "Matrix";
const SEEDED_MATRIX_SERVICE_HANDLE: &str = "@jp:matrix.example";
/// The handle that the user types into that field, and the URI that the save
/// must make from it. The host and the local part both change, so a partly
/// rewritten URI matches neither value.
const RETYPED_HANDLE: &str = "jp@xmpp.example";
const RETYPED_SERVICE_URI: &str = "xmpp:jp@xmpp.example";
/// The creation time of the first note. RFC 9553 §2.8.3's `created` has no
/// `NOTE` component or parameter, so it stays in the entry's `extra`. It shows
/// whether a save *patched* the entry or replaced it: a replacement holds only
/// the text.
const SEEDED_NOTE_CREATED: &str = "2026-02-01T09:30:00Z";
/// The keys and dates of the seeded card's anniversaries from the *server*.
///
/// Three entries of different shapes: a wedding anniversary that goes to the
/// `X-EVOLUTION-ANNIVERSARY` line, a year-only birthday that no vCard line can
/// state, and a death date that vCard 3.0 has no field for.
const SEEDED_WEDDING_KEY: &str = "wedding-1";
const SEEDED_YEAR_BIRTHDAY_KEY: &str = "birth-year-1";
const SEEDED_DEATHDAY_KEY: &str = "death-1";
const SEEDED_WEDDING_YEAR: u32 = 2005;
const SEEDED_WEDDING_MONTH: u32 = 6;
const SEEDED_WEDDING_DAY: u32 = 18;
const SEEDED_BIRTH_YEAR: u32 = 1980;
const SEEDED_DEATH_YEAR: u32 = 2021;
const SEEDED_DEATH_MONTH: u32 = 5;
const SEEDED_DEATH_DAY: u32 = 12;

/// The organizations, titles, and roles that the server put on the seeded card.
///
/// Evolution's contact editor shows only the first ORG line and the first TITLE
/// and ROLE lines, so the user cannot see the other entries. Every seeded leg
/// checks that these entries survive.
const SEEDED_ORG_KEY: &str = "org-1";
const SEEDED_SECOND_ORG_KEY: &str = "org-2";
const SEEDED_ORG_NAME: &str = "Acme Ltd";
const SEEDED_ORG_UNIT: &str = "Research";
const SEEDED_SECOND_ORG_NAME: &str = "Brauerei";
const SEEDED_SECOND_ORG_UNIT: &str = "Logistics";

const SEEDED_TITLE_KEY: &str = "title-1";
const SEEDED_SECOND_TITLE_KEY: &str = "title-2";
const SEEDED_ROLE_KEY: &str = "role-1";
const SEEDED_SECOND_ROLE_KEY: &str = "role-2";
const SEEDED_TITLE_NAME: &str = "Senior Research Scientist";
const SEEDED_SECOND_TITLE_NAME: &str = "Director of Engineering";
const SEEDED_ROLE_NAME: &str = "Lead Investigator";
const SEEDED_SECOND_ROLE_NAME: &str = "Project Manager";

const SEEDED_WORK_ADDR_KEY: &str = "addr-work-1";
const SEEDED_WORK_STREET: &str = "Hauptstrasse 1";
const SEEDED_WORK_LOCALITY: &str = "Berlin";
const SEEDED_WORK_POSTCODE: &str = "10115";
const SEEDED_WORK_COUNTRY: &str = "Germany";
const SEEDED_WORK_LABEL: &str = "Hauptstrasse 1\n10115 Berlin\nGermany";

const SEEDED_HOME_ADDR_KEY: &str = "addr-home-1";
const SEEDED_HOME_STREET: &str = "Heimweg 2";
const SEEDED_HOME_LOCALITY: &str = "Muenchen";
const SEEDED_HOME_POSTCODE: &str = "80331";
const SEEDED_HOME_COUNTRY: &str = "Germany";
const SEEDED_HOME_LABEL: &str = "Heimweg 2\n80331 Muenchen\nGermany";

/// The notes that the seeded card holds.
///
/// Only one leg needs this. When the user clears the Notes field, the mapping
/// gets a `notes` map with no visible entries. If a hidden note remains, the
/// patch removes only the cleared entry. If no note remains, the patch removes
/// the full property. The second case has no other leg.
#[derive(Clone, Copy)]
enum SeededNotes {
    /// Both notes: the one that the Notes field shows, and the one behind it.
    Both,
    /// Only the first note. The Notes field that the user clears is all of
    /// `notes`.
    TheShownOneAlone,
}

/// Seed the card that nine of the seeded legs start from, and return its id.
///
/// The card goes directly into the store, not through EDS, because no vCard can
/// state its shape. A card made through EDS has the given name as one
/// component, so the save has nothing to restore or (in the rename leg) remove.
fn seed_double_barrelled_card(server: &jmap_mock::MockServer) -> Id {
    seed_card(server, SeededNotes::Both)
}

/// Seed the same card, but with one note instead of two.
///
/// All other properties are the same as on the other card. The note that the
/// user can see is the only note, and the leg must still be able to show that
/// the save did not change the picture, the calendars, and the relations.
fn seed_single_note_card(server: &jmap_mock::MockServer) -> Id {
    seed_card(server, SeededNotes::TheShownOneAlone)
}

fn seed_card(server: &jmap_mock::MockServer, seeded_notes: SeededNotes) -> Id {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().expect("mock state lock");
    let account = state
        .account_mut(&account_id)
        .expect("the mock's default account");
    let book = account.seed_address_book("Personal", true);

    let mut components = vec![NameComponent::new("surname", EDITED_SURNAME)];
    for (value, phonetic) in EDITED_GIVEN_PARTS {
        let mut component = NameComponent::new("given", value);
        component
            .extra
            .insert("phonetic".to_owned(), serde_json::json!(phonetic));
        components.push(component);
    }

    let id = account.contact_cards.alloc_id();
    let mut card = ContactCard::simple(book, EDITED_FULL_NAME, EDITED_EMAIL_BEFORE);
    card.id = Some(id.clone());
    // The server sets a uid. The mock's `ContactCard/set` sets the same shape,
    // but seeding does not go through it.
    card.uid = Some(format!("urn:example:card:{}", id.as_str()));
    card.name = Some(Name {
        full: Some(EDITED_FULL_NAME.to_owned()),
        components: Some(components),
        ..Name::default()
    });
    // A picture under a key that only a server would select. Like the
    // pronunciations, the user does not edit it, so the save must return it
    // unchanged. But the user *can* see the picture, so EDS includes it in the
    // round trip.
    card.media = Some(
        [
            (
                SEEDED_PHOTO_KEY.to_owned(),
                Media {
                    kind: Some("photo".to_owned()),
                    uri: format!("data:{PHOTO_MEDIA_TYPE};base64,{PHOTO_BASE64}"),
                    media_type: Some(PHOTO_MEDIA_TYPE.to_owned()),
                    ..Media::default()
                },
            ),
            (
                SEEDED_LOGO_KEY.to_owned(),
                Media {
                    kind: Some("logo".to_owned()),
                    uri: SEEDED_LOGO_URI.to_owned(),
                    ..Media::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    // The two calendar resources, under server keys. They are two entries of
    // one map on lines with *different* names, so the reader's key counter must
    // give them different keys. No line can hold the `pref`; see
    // [`SEEDED_CALENDAR_PREF`].
    let mut calendar = Calendar {
        kind: Some("calendar".to_owned()),
        uri: SEEDED_CALENDAR_URI.to_owned(),
        ..Calendar::default()
    };
    calendar
        .extra
        .insert("pref".to_owned(), serde_json::json!(SEEDED_CALENDAR_PREF));
    card.calendars = Some(
        [
            (SEEDED_CALENDAR_KEY.to_owned(), calendar),
            (
                SEEDED_FREEBUSY_KEY.to_owned(),
                Calendar {
                    kind: Some("freeBusy".to_owned()),
                    uri: SEEDED_FREEBUSY_URI.to_owned(),
                    ..Calendar::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    // The relations: the marriage that the spouse line shows, and a sibling
    // that no field can show. With two entries, the test can see the difference
    // between a save that removed the marriage and a save that emptied the map.
    card.related_to = Some(
        [
            (SEEDED_SPOUSE.to_owned(), relation_of("spouse")),
            (SEEDED_SIBLING.to_owned(), relation_of("sibling")),
        ]
        .into_iter()
        .collect(),
    );
    // The notes, under server keys. This is the only mapped property where the
    // user can see one entry and not the other: Evolution shows only the first
    // `NOTE` line. No line can hold the `created`; see [`SEEDED_NOTE_CREATED`].
    // The caller selects the number of notes; see [`SeededNotes`].
    let mut note = Note {
        note: SEEDED_NOTE.to_owned(),
        ..Note::default()
    };
    note.extra
        .insert("created".to_owned(), serde_json::json!(SEEDED_NOTE_CREATED));
    let mut notes: BTreeMap<String, Note> = [(SEEDED_NOTE_KEY.to_owned(), note)].into();
    if matches!(seeded_notes, SeededNotes::Both) {
        notes.insert(
            SEEDED_SECOND_NOTE_KEY.to_owned(),
            Note {
                note: SEEDED_SECOND_NOTE.to_owned(),
                ..Note::default()
            },
        );
    }
    card.notes = Some(notes);
    // Three online services. The Jabber and Skype entries state only a URI (see
    // [`SEEDED_SERVICE_KEY`]), and the Matrix entry states only a `user`. The
    // Skype and Matrix entries also have contexts.
    card.online_services = Some(
        [
            (
                SEEDED_SERVICE_KEY.to_owned(),
                OnlineService {
                    service: Some(SEEDED_SERVICE.to_owned()),
                    uri: Some(SEEDED_SERVICE_URI.to_owned()),
                    ..OnlineService::default()
                },
            ),
            (
                SEEDED_SKYPE_SERVICE_KEY.to_owned(),
                OnlineService {
                    service: Some(SEEDED_SKYPE_SERVICE.to_owned()),
                    uri: Some(SEEDED_SKYPE_SERVICE_URI.to_owned()),
                    extra: [("contexts".to_owned(), serde_json::json!({"private": true}))].into(),
                    ..OnlineService::default()
                },
            ),
            (
                SEEDED_MATRIX_SERVICE_KEY.to_owned(),
                OnlineService {
                    service: Some(SEEDED_MATRIX_SERVICE.to_owned()),
                    user: Some(SEEDED_MATRIX_SERVICE_HANDLE.to_owned()),
                    extra: [("contexts".to_owned(), serde_json::json!({"work": true}))].into(),
                    ..OnlineService::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    card.anniversaries = Some(
        [
            (
                SEEDED_WEDDING_KEY.to_owned(),
                Anniversary {
                    kind: "wedding".to_owned(),
                    date: Some(serde_json::json!({
                        "@type": "PartialDate",
                        "year": SEEDED_WEDDING_YEAR,
                        "month": SEEDED_WEDDING_MONTH,
                        "day": SEEDED_WEDDING_DAY,
                    })),
                    ..Anniversary::default()
                },
            ),
            (
                SEEDED_YEAR_BIRTHDAY_KEY.to_owned(),
                Anniversary {
                    kind: "birth".to_owned(),
                    date: Some(serde_json::json!({
                        "@type": "PartialDate",
                        "year": SEEDED_BIRTH_YEAR,
                    })),
                    ..Anniversary::default()
                },
            ),
            (
                SEEDED_DEATHDAY_KEY.to_owned(),
                Anniversary {
                    kind: "death".to_owned(),
                    date: Some(serde_json::json!({
                        "@type": "PartialDate",
                        "year": SEEDED_DEATH_YEAR,
                        "month": SEEDED_DEATH_MONTH,
                        "day": SEEDED_DEATH_DAY,
                    })),
                    ..Anniversary::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    card.organizations = Some(
        [
            (
                SEEDED_ORG_KEY.to_owned(),
                Organization {
                    name: Some(SEEDED_ORG_NAME.to_owned()),
                    units: Some(vec![OrgUnit::new(SEEDED_ORG_UNIT)]),
                    ..Organization::default()
                },
            ),
            (
                SEEDED_SECOND_ORG_KEY.to_owned(),
                Organization {
                    name: Some(SEEDED_SECOND_ORG_NAME.to_owned()),
                    units: Some(vec![OrgUnit::new(SEEDED_SECOND_ORG_UNIT)]),
                    ..Organization::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    card.titles = Some(
        [
            (
                SEEDED_TITLE_KEY.to_owned(),
                Title {
                    name: SEEDED_TITLE_NAME.to_owned(),
                    kind: None,
                    ..Title::default()
                },
            ),
            (
                SEEDED_SECOND_TITLE_KEY.to_owned(),
                Title {
                    name: SEEDED_SECOND_TITLE_NAME.to_owned(),
                    kind: None,
                    ..Title::default()
                },
            ),
            (
                SEEDED_ROLE_KEY.to_owned(),
                Title {
                    name: SEEDED_ROLE_NAME.to_owned(),
                    kind: Some("role".to_owned()),
                    ..Title::default()
                },
            ),
            (
                SEEDED_SECOND_ROLE_KEY.to_owned(),
                Title {
                    name: SEEDED_SECOND_ROLE_NAME.to_owned(),
                    kind: Some("role".to_owned()),
                    ..Title::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    card.addresses = Some(
        [
            (
                SEEDED_WORK_ADDR_KEY.to_owned(),
                Address {
                    contexts: Some(serde_json::json!({"work": true})),
                    components: Some(vec![
                        AddressComponent::new("name", SEEDED_WORK_STREET),
                        AddressComponent::new("locality", SEEDED_WORK_LOCALITY),
                        AddressComponent::new("postcode", SEEDED_WORK_POSTCODE),
                        AddressComponent::new("country", SEEDED_WORK_COUNTRY),
                    ]),
                    full: Some(SEEDED_WORK_LABEL.to_owned()),
                    ..Address::default()
                },
            ),
            (
                SEEDED_HOME_ADDR_KEY.to_owned(),
                Address {
                    contexts: Some(serde_json::json!({"private": true})),
                    components: Some(vec![
                        AddressComponent::new("name", SEEDED_HOME_STREET),
                        AddressComponent::new("locality", SEEDED_HOME_LOCALITY),
                        AddressComponent::new("postcode", SEEDED_HOME_POSTCODE),
                        AddressComponent::new("country", SEEDED_HOME_COUNTRY),
                    ]),
                    full: Some(SEEDED_HOME_LABEL.to_owned()),
                    ..Address::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    account.contact_cards.seed_with_id(id.clone(), card);
    id
}

/// A `relatedTo` value with one relation type, as the Set of RFC 9553 §2.1.8:
/// the type is a key and its value is `true`.
fn relation_of(kind: &str) -> Relation {
    Relation {
        relation: Some([(kind.to_owned(), serde_json::json!(true))].into()),
        ..Relation::default()
    }
}

/// Check that the card on the server still has the seeded sibling, under the
/// same key and with the same type.
///
/// This is separate from the marriage check because the spouse legs change the
/// marriage but not this entry. No vCard line shows this entry, so the user
/// cannot see or edit it.
fn assert_the_seeded_sibling_survived(card: &ContactCard) {
    let related = card
        .related_to
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped everyone the card relates to: {card:?}"));
    let sibling = related
        .get(SEEDED_SIBLING)
        .unwrap_or_else(|| panic!("the save dropped the brother nobody touched: {card:?}"));
    assert_eq!(
        *sibling,
        relation_of("sibling"),
        "the save rewrote a relation nobody touched: {card:?}"
    );
}

/// Check that the card on the server still has both seeded relations.
///
/// For the legs that edit a different property, for the same reason as
/// [`assert_the_seeded_calendars_survived`]. A change to either entry, or a new
/// key for the marriage, is a change that the user did not ask for.
fn assert_the_seeded_relations_survived(card: &ContactCard) {
    assert_the_seeded_sibling_survived(card);
    let related = card.related_to.as_ref().expect("relations, just checked");
    assert_eq!(
        related.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_SIBLING, SEEDED_SPOUSE],
        "the save re-keyed a relation nobody touched: {card:?}"
    );
    assert_eq!(
        related[SEEDED_SPOUSE],
        relation_of("spouse"),
        "the save rewrote the marriage nobody touched: {card:?}"
    );
}

/// Check that the card on the server still has the seeded free/busy address:
/// the same URI, kind, and server key.
///
/// This is separate from the calendar check because the calendar leg changes
/// the calendar address but not this one.
fn assert_the_seeded_freebusy_survived(card: &ContactCard) {
    let calendars = card
        .calendars
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's calendars: {card:?}"));
    let freebusy = calendars.get(SEEDED_FREEBUSY_KEY).unwrap_or_else(|| {
        panic!("the save re-filed the free/busy address nobody touched: {card:?}")
    });
    assert_eq!(freebusy.kind.as_deref(), Some("freeBusy"), "{card:?}");
    assert_eq!(
        freebusy.uri, SEEDED_FREEBUSY_URI,
        "the save rewrote the free/busy address nobody touched: {card:?}"
    );
}

/// Check that the card on the server still has both seeded calendar resources,
/// unchanged.
///
/// For the legs that edit a different property, for the same reason as
/// [`assert_the_seeded_picture_survived`]. A new key for either entry, or a
/// lost `pref` (which no line can hold), is a change that the user did not ask
/// for.
fn assert_the_seeded_calendars_survived(card: &ContactCard) {
    assert_the_seeded_freebusy_survived(card);
    let calendars = card.calendars.as_ref().expect("calendars, just checked");
    assert_eq!(
        calendars.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_CALENDAR_KEY, SEEDED_FREEBUSY_KEY],
        "the save re-filed a calendaring resource nobody touched: {card:?}"
    );
    let calendar = calendars
        .get(SEEDED_CALENDAR_KEY)
        .expect("the seeded calendar, just checked");
    assert_eq!(calendar.kind.as_deref(), Some("calendar"), "{card:?}");
    assert_eq!(
        calendar.uri, SEEDED_CALENDAR_URI,
        "the save rewrote the calendar address nobody touched: {card:?}"
    );
    assert_eq!(
        calendar.pref.or_else(|| calendar
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(SEEDED_CALENDAR_PREF),
        "{card:?}"
    );
}

/// Check that the card on the server still has the seeded second note: the same
/// text under the same server key.
///
/// This is separate from the first-note check because the note legs change the
/// first note but not this one. The entry goes to a line, but the user cannot
/// see it: Evolution's Notes field shows only the first `NOTE` line. A save
/// that deleted it deletes a note that the user did not see.
fn assert_the_seeded_second_note_survived(card: &ContactCard) {
    let notes = card
        .notes
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's notes: {card:?}"));
    let second = notes
        .get(SEEDED_SECOND_NOTE_KEY)
        .unwrap_or_else(|| panic!("the save dropped the note nobody could see: {card:?}"));
    assert_eq!(
        second.note, SEEDED_SECOND_NOTE,
        "the save rewrote a note nobody touched: {card:?}"
    );
}

/// Check that the card on the server still has both seeded notes, unchanged.
///
/// For the legs that edit a different property, for the same reason as
/// [`assert_the_seeded_calendars_survived`]. A new key for either entry, or a
/// lost `created` (which no line can hold), is a change that the user did not
/// ask for.
fn assert_the_seeded_notes_survived(card: &ContactCard) {
    assert_the_seeded_second_note_survived(card);
    let notes = card.notes.as_ref().expect("notes, just checked");
    assert_eq!(
        notes.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_NOTE_KEY, SEEDED_SECOND_NOTE_KEY],
        "the save re-filed a note nobody touched: {card:?}"
    );
    let note = notes
        .get(SEEDED_NOTE_KEY)
        .expect("the seeded note, just checked");
    assert_eq!(
        note.note, SEEDED_NOTE,
        "the save rewrote the note nobody touched: {card:?}"
    );
    assert_eq!(
        note.created
            .as_ref()
            .map(|d| d.as_str())
            .or_else(|| note.extra.get("created").and_then(|v| v.as_str())),
        Some(SEEDED_NOTE_CREATED),
        "{card:?}"
    );
}

/// Check that the card on the server still has the seeded picture (the same
/// bytes, kind, and server key) and the seeded logo.
///
/// For the legs that do not edit the picture. A new key for the picture,
/// changed bytes, or a lost picture is a change that the user did not ask for.
fn assert_the_seeded_picture_survived(card: &ContactCard) {
    let media = card
        .media
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's picture: {card:?}"));
    assert_eq!(
        media.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_LOGO_KEY, SEEDED_PHOTO_KEY],
        "the save re-filed the media nobody touched: {card:?}"
    );
    let picture = &media[SEEDED_PHOTO_KEY];
    assert_eq!(picture.kind.as_deref(), Some("photo"), "{card:?}");
    assert_eq!(
        picture.media_type.as_deref(),
        Some(PHOTO_MEDIA_TYPE),
        "{card:?}"
    );
    assert_eq!(
        picture.uri,
        format!("data:{PHOTO_MEDIA_TYPE};base64,{PHOTO_BASE64}"),
        "the save rewrote the picture nobody touched: {card:?}"
    );
    let logo = &media[SEEDED_LOGO_KEY];
    assert_eq!(logo.kind.as_deref(), Some("logo"), "{card:?}");
    assert_eq!(logo.uri, SEEDED_LOGO_URI, "{card:?}");
}

/// Check that the card on the server still has the three seeded online
/// services. The Jabber entry must keep its key and service, and must still
/// state a `uri` and no `user`.
///
/// For the legs that edit a different property. The user can see the Jabber
/// entry and could retype it, so this checks that an edit to a different field
/// left the entry in the shape that the server gave it.
///
/// This check cannot find one failure (measured, not only by analysis): a save
/// that compares the *members* of an entry instead of the handle. That save
/// writes the same URI again, so all assertions here pass, but the server state
/// changes on each save. The fixture
/// `an_edit_that_left_a_uri_only_handle_alone_writes_nothing` in
/// `jmap-book-sync` finds it with a before-and-after comparison.
fn assert_the_seeded_service_survived(card: &ContactCard) {
    let services = card
        .online_services
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's online services: {card:?}"));
    assert_eq!(
        services.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            SEEDED_SERVICE_KEY,
            SEEDED_SKYPE_SERVICE_KEY,
            SEEDED_MATRIX_SERVICE_KEY,
        ],
        "the save re-filed the handles nobody touched: {card:?}"
    );
    let s1 = &services[SEEDED_SERVICE_KEY];
    assert_eq!(s1.service.as_deref(), Some(SEEDED_SERVICE), "{card:?}");
    assert_eq!(
        s1.uri.as_deref(),
        Some(SEEDED_SERVICE_URI),
        "the save rewrote the Jabber handle: {card:?}"
    );
    assert_eq!(
        s1.user, None,
        "the save answered a card stating a URI with one stating a handle: {card:?}"
    );

    let s2 = &services[SEEDED_SKYPE_SERVICE_KEY];
    assert_eq!(
        s2.service.as_deref(),
        Some(SEEDED_SKYPE_SERVICE),
        "{card:?}"
    );
    assert_eq!(
        s2.uri.as_deref(),
        Some(SEEDED_SKYPE_SERVICE_URI),
        "the save rewrote the Skype handle: {card:?}"
    );
    assert_eq!(s2.user, None);
    assert_eq!(
        s2.contexts.as_ref().or_else(|| s2.extra.get("contexts")),
        Some(&serde_json::json!({"private": true}))
    );

    let s3 = &services[SEEDED_MATRIX_SERVICE_KEY];
    assert_eq!(
        s3.service.as_deref(),
        Some(SEEDED_MATRIX_SERVICE),
        "{card:?}"
    );
    assert_eq!(
        s3.user.as_deref(),
        Some(SEEDED_MATRIX_SERVICE_HANDLE),
        "the save rewrote the Matrix handle: {card:?}"
    );
    assert_eq!(s3.uri, None);
    assert_eq!(
        s3.contexts.as_ref().or_else(|| s3.extra.get("contexts")),
        Some(&serde_json::json!({"work": true}))
    );
}

/// Check that the card on the server still has the three seeded anniversaries:
/// the wedding anniversary that the X-EVOLUTION-ANNIVERSARY line shows, the
/// year-only birthday that no line can show, and the death date that vCard 3.0
/// has no property for.
///
/// For the legs that edit a different property. A change to any of them, or a
/// lost year-only birthday or death date, is a change that the user did not ask
/// for.
fn assert_the_seeded_anniversaries_survived(card: &ContactCard) {
    let anniversaries = card
        .anniversaries
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's anniversaries: {card:?}"));
    assert_eq!(
        anniversaries.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            SEEDED_YEAR_BIRTHDAY_KEY,
            SEEDED_DEATHDAY_KEY,
            SEEDED_WEDDING_KEY
        ],
        "the save re-keyed an anniversary nobody touched: {card:?}"
    );
    let wedding = &anniversaries[SEEDED_WEDDING_KEY];
    assert_eq!(wedding.kind, "wedding", "{card:?}");
    assert_eq!(
        wedding.date,
        Some(serde_json::json!({
            "@type": "PartialDate",
            "year": SEEDED_WEDDING_YEAR,
            "month": SEEDED_WEDDING_MONTH,
            "day": SEEDED_WEDDING_DAY,
        })),
        "the save rewrote the wedding anniversary: {card:?}"
    );
    let birth = &anniversaries[SEEDED_YEAR_BIRTHDAY_KEY];
    assert_eq!(birth.kind, "birth", "{card:?}");
    assert_eq!(
        birth.date,
        Some(serde_json::json!({
            "@type": "PartialDate",
            "year": SEEDED_BIRTH_YEAR,
        })),
        "the save dropped or mangled the year-only birthday: {card:?}"
    );
    let death = &anniversaries[SEEDED_DEATHDAY_KEY];
    assert_eq!(death.kind, "death", "{card:?}");
    assert_eq!(
        death.date,
        Some(serde_json::json!({
            "@type": "PartialDate",
            "year": SEEDED_DEATH_YEAR,
            "month": SEEDED_DEATH_MONTH,
            "day": SEEDED_DEATH_DAY,
        })),
        "the save dropped or mangled the deathday: {card:?}"
    );
}

/// Check that the card on the server still has the two seeded organizations and
/// the four seeded titles and roles.
///
/// Evolution's contact editor shows only the first ORG line and the first TITLE
/// and ROLE lines, so the user cannot see the other entries. A save of other
/// fields must keep all of them on the server.
fn assert_the_seeded_organizations_and_titles_survived(card: &ContactCard) {
    let orgs = card
        .organizations
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's organizations: {card:?}"));
    assert_eq!(
        orgs.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_ORG_KEY, SEEDED_SECOND_ORG_KEY],
        "the save re-keyed or dropped an organization: {card:?}"
    );
    assert_eq!(orgs[SEEDED_ORG_KEY].name.as_deref(), Some(SEEDED_ORG_NAME));
    assert_eq!(
        orgs[SEEDED_ORG_KEY]
            .units
            .as_ref()
            .map(|u| u.iter().map(|o| o.name.as_str()).collect::<Vec<_>>()),
        Some(vec![SEEDED_ORG_UNIT])
    );
    assert_eq!(
        orgs[SEEDED_SECOND_ORG_KEY].name.as_deref(),
        Some(SEEDED_SECOND_ORG_NAME)
    );
    assert_eq!(
        orgs[SEEDED_SECOND_ORG_KEY]
            .units
            .as_ref()
            .map(|u| u.iter().map(|o| o.name.as_str()).collect::<Vec<_>>()),
        Some(vec![SEEDED_SECOND_ORG_UNIT])
    );

    let titles = card
        .titles
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's titles: {card:?}"));
    assert_eq!(
        titles.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            SEEDED_ROLE_KEY,
            SEEDED_SECOND_ROLE_KEY,
            SEEDED_TITLE_KEY,
            SEEDED_SECOND_TITLE_KEY
        ],
        "the save re-keyed or dropped a title/role: {card:?}"
    );
    assert_eq!(titles[SEEDED_TITLE_KEY].name, SEEDED_TITLE_NAME);
    assert_eq!(titles[SEEDED_TITLE_KEY].kind, None);
    assert_eq!(
        titles[SEEDED_SECOND_TITLE_KEY].name,
        SEEDED_SECOND_TITLE_NAME
    );
    assert_eq!(titles[SEEDED_SECOND_TITLE_KEY].kind, None);
    assert_eq!(titles[SEEDED_ROLE_KEY].name, SEEDED_ROLE_NAME);
    assert_eq!(titles[SEEDED_ROLE_KEY].kind.as_deref(), Some("role"));
    assert_eq!(titles[SEEDED_SECOND_ROLE_KEY].name, SEEDED_SECOND_ROLE_NAME);
    assert_eq!(titles[SEEDED_SECOND_ROLE_KEY].kind.as_deref(), Some("role"));
}

/// Check that the card on the server still has the two seeded addresses (work
/// and home), under the same keys and with the same full labels.
fn assert_the_seeded_addresses_survived(card: &ContactCard) {
    let addresses = card
        .addresses
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's addresses: {card:?}"));
    assert_eq!(
        addresses.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_HOME_ADDR_KEY, SEEDED_WORK_ADDR_KEY],
        "the save re-keyed or dropped an address: {card:?}"
    );
    assert_eq!(
        addresses[SEEDED_WORK_ADDR_KEY].full.as_deref(),
        Some(SEEDED_WORK_LABEL)
    );
    assert_eq!(
        addresses[SEEDED_HOME_ADDR_KEY].full.as_deref(),
        Some(SEEDED_HOME_LABEL)
    );
}

/// The port that the mock listens on, for the keyfile of the session.
fn mock_port(server: &jmap_mock::MockServer) -> u16 {
    server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number")
}

#[test]
fn an_edit_through_eds_keeps_the_name_parts_the_vcard_flattened() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/address-book-edit"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    // The client identifies the contact by its JMAP id, which the backend gives
    // EDS as the vCard `UID`.
    let output = session.run(
        &client,
        &[
            "jmap-functional",
            "edit",
            card_id.as_str(),
            EDITED_EMAIL_AFTER,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // How EDS read the `N` line from the backend: the two components joined in
    // the one field of their kind.
    assert_eq!(
        seen.get("read-full-name"),
        Some(&EDITED_FULL_NAME),
        "the contact EDS handed back is not the seeded one\n{report}"
    );
    assert_eq!(
        seen.get("read-given-name"),
        Some(&EDITED_GIVEN_FIELD),
        "EDS did not hand back the given name the emitter wrote\n{report}"
    );
    assert_eq!(
        seen.get("read-family-name"),
        Some(&EDITED_SURNAME),
        "EDS did not hand back the surname the emitter wrote\n{report}"
    );
    // How EDS read the `PHOTO` line from the backend. The write leg cannot
    // check this direction, because there EDS wrote the line. Here the emitter
    // wrote it: `X-JMAP-KEY` before the parameters of EDS, and the bytes of a
    // `data:` URI. If the user cannot see the picture, Evolution offers to
    // replace it with nothing.
    //
    // EDS returns a `file:` URI again, for the reason that the write leg gives.
    // The `.png` shows that the media type survived: EDS names the cache file
    // from the picture type.
    assert_eq!(
        seen.get("read-photo-type"),
        Some(&"uri"),
        "EDS did not read the picture off the line the emitter wrote\n{report}"
    );
    let seeded_photo = seen
        .get("read-photo-uri")
        .unwrap_or_else(|| panic!("the client reported no picture\n{report}"));
    assert!(
        seeded_photo.starts_with("file://") && seeded_photo.ends_with(".png"),
        "EDS did not cache the seeded picture as the kind of image it is: \
         {seeded_photo}\n{report}"
    );
    assert_eq!(
        seen.get("read-photo-file-base64"),
        Some(&PHOTO_BASE64),
        "EDS did not read back the bytes the emitter wrote\n{report}"
    );

    // After the save: the edit is in the cache, and EDS still has the original
    // name.
    assert_eq!(
        seen.get("read-back-email"),
        Some(&EDITED_EMAIL_AFTER),
        "the edit did not reach EDS's cache\n{report}"
    );
    assert_eq!(
        seen.get("read-back-given-name"),
        Some(&EDITED_GIVEN_FIELD),
        "the save changed the given name nobody edited\n{report}"
    );
    assert_eq!(
        seen.get("read-back-family-name"),
        Some(&EDITED_SURNAME),
        "the save changed the surname nobody edited\n{report}"
    );

    // The other end: the card on the server. The name components are the
    // purpose of this leg: both parts, in their original order, each with the
    // pronunciation that the `N` line cannot hold.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the edit never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    let components = card
        .name
        .as_ref()
        .and_then(|name| name.components.as_ref())
        .unwrap_or_else(|| panic!("the card on the server lost its name: {card:?}"));
    let stated: Vec<(&str, &str, Option<&str>)> = components
        .iter()
        .map(|component| {
            (
                component.kind.as_str(),
                component.value.as_str(),
                component
                    .extra
                    .get("phonetic")
                    .and_then(|phonetic| phonetic.as_str()),
            )
        })
        .collect();
    let mut expected: Vec<(&str, &str, Option<&str>)> = vec![("surname", EDITED_SURNAME, None)];
    for (value, phonetic) in EDITED_GIVEN_PARTS {
        expected.push(("given", value, Some(phonetic)));
    }
    assert_eq!(
        stated, expected,
        "an edit to the email address rewrote the name: {card:?}"
    );
    assert_eq!(
        card.name.as_ref().and_then(|name| name.full.as_deref()),
        Some(EDITED_FULL_NAME),
        "{card:?}"
    );
    // The edit on the server: patched in place, not added again.
    let emails = card
        .emails
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server lost its email: {card:?}"));
    assert_eq!(
        emails
            .values()
            .map(|email| email.address.as_str())
            .collect::<Vec<_>>(),
        vec![EDITED_EMAIL_AFTER],
        "{card:?}"
    );
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The given name that the user types, and the full name that Evolution's
/// contact editor changes to match it.
///
/// One word for two, and not a substring of either part: `Hans` does not show
/// which part of `Jean Paul` it replaced, so both parts must go.
const RETYPED_GIVEN_NAME: &str = "Hans";
const RETYPED_FULL_NAME: &str = "Hans Oldenburg";

#[test]
fn retyping_the_name_through_eds_replaces_the_parts_the_vcard_flattened() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/address-book-rename"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(
        &client,
        &[
            "jmap-functional",
            "rename",
            card_id.as_str(),
            RETYPED_FULL_NAME,
            RETYPED_GIVEN_NAME,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // EDS starts from the seeded card, with the two parts joined in one field.
    // The preceding leg checks this too. Check it again, because the save
    // selects the branch under test by comparing against this text.
    assert_eq!(
        seen.get("read-given-name"),
        Some(&EDITED_GIVEN_FIELD),
        "EDS did not hand back the given name the emitter wrote\n{report}"
    );

    // After the save, EDS has the name that the user typed, in the given-name
    // field and on the FN line.
    assert_eq!(
        seen.get("read-back-given-name"),
        Some(&RETYPED_GIVEN_NAME),
        "the name the user retyped did not survive the save\n{report}"
    );
    assert_eq!(
        seen.get("read-back-family-name"),
        Some(&EDITED_SURNAME),
        "the save changed the surname nobody edited\n{report}"
    );
    assert_eq!(
        seen.get("read-back-full-name"),
        Some(&RETYPED_FULL_NAME),
        "the full name the editor wrote did not survive the save\n{report}"
    );

    // The other end, and the necessary assertion: the server has the given name
    // as the *one* component that the user typed. The two seeded parts are
    // gone, and so are their pronunciations: a `phonetic` for `Jean` is not
    // correct for `Hans`.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the rename never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    let components = card
        .name
        .as_ref()
        .and_then(|name| name.components.as_ref())
        .unwrap_or_else(|| panic!("the card on the server lost its name: {card:?}"));
    let stated: Vec<(&str, &str, Option<&str>)> = components
        .iter()
        .map(|component| {
            (
                component.kind.as_str(),
                component.value.as_str(),
                component
                    .extra
                    .get("phonetic")
                    .and_then(|phonetic| phonetic.as_str()),
            )
        })
        .collect();
    assert_eq!(
        stated,
        vec![
            ("surname", EDITED_SURNAME, None),
            ("given", RETYPED_GIVEN_NAME, None),
        ],
        "the name the user retyped did not replace the parts it was built \
         from: {card:?}"
    );
    assert_eq!(
        card.name.as_ref().and_then(|name| name.full.as_deref()),
        Some(RETYPED_FULL_NAME),
        "{card:?}"
    );
    // The email address that nobody changed is still the seeded one.
    let emails = card
        .emails
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server lost its email: {card:?}"));
    assert_eq!(
        emails
            .values()
            .map(|email| email.address.as_str())
            .collect::<Vec<_>>(),
        vec![EDITED_EMAIL_BEFORE],
        "{card:?}"
    );
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The fourth leg: the user selects a new picture for the card, whose picture
/// has a server key.
///
/// The test examines where the new picture goes. When the photo field is *set*,
/// EDS writes the `PHOTO` line again and removes its parameters, which include
/// the `X-JMAP-KEY`. Thus the new picture has no key that identifies the entry
/// it replaces. Without pairing, the server holds two pictures: the old one
/// under its key and the new one under a key that the reader made by counting
/// lines. The save must pair the keyless picture with the old one and patch it.
///
/// This needs real daemons. A picture that the user does not change keeps its
/// key, because EDS does not *set* the line of a cached card. This was
/// measured: [`an_edit_through_eds_keeps_the_name_parts_the_vcard_flattened`]
/// passes with the pairing removed. A set picture loses its key. EDS selects
/// which of the two the save gets, and this leg makes EDS select a set.
#[test]
fn replacing_the_picture_through_eds_patches_the_entry_it_replaces() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/address-book-repicture"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(
        &client,
        &[
            "jmap-functional",
            "repicture",
            card_id.as_str(),
            REPLACEMENT_PHOTO_BASE64,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // The original picture, so that the leg shows what the user replaced.
    assert_eq!(
        seen.get("read-photo-file-base64"),
        Some(&PHOTO_BASE64),
        "the card EDS handed back is not the seeded one\n{report}"
    );
    // After the save, EDS has the new picture in the cache file of the card
    // that the backend made again. This one observation is the full round trip:
    // selected in a libebook consumer, sent to the server, and read back.
    assert_eq!(
        seen.get("read-back-photo-file-base64"),
        Some(&REPLACEMENT_PHOTO_BASE64),
        "the picture the user chose is not the one EDS ended up with\n{report}"
    );
    // This edit does not include the name. Assert it, as the `edit` leg does: a
    // save that changed it did something that the user did not ask for.
    assert_eq!(
        seen.get("read-back-given-name"),
        Some(&EDITED_GIVEN_FIELD),
        "the save changed the given name nobody edited\n{report}"
    );

    // The other end, and the necessary assertion: one picture, under the key
    // that the *server* selected, with the new bytes. A second entry, or the
    // old picture still here beside a new `m1`, shows that the pairing failed.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the new picture never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    let media = card
        .media
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's picture: {card:?}"));
    assert_eq!(
        media.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_LOGO_KEY, SEEDED_PHOTO_KEY],
        "the new picture was filed beside the old one instead of over it: {card:?}"
    );
    let picture = &media[SEEDED_PHOTO_KEY];
    assert_eq!(picture.kind.as_deref(), Some("photo"), "{card:?}");
    assert_eq!(
        picture.media_type.as_deref(),
        Some(PHOTO_MEDIA_TYPE),
        "{card:?}"
    );
    assert_eq!(
        picture.uri,
        format!("data:{PHOTO_MEDIA_TYPE};base64,{REPLACEMENT_PHOTO_BASE64}"),
        "the picture the user chose did not reach the server: {card:?}"
    );
    let logo = &media[SEEDED_LOGO_KEY];
    assert_eq!(logo.kind.as_deref(), Some("logo"), "{card:?}");
    assert_eq!(logo.uri, SEEDED_LOGO_URI, "{card:?}");
    // The properties that nobody changed: the given-name components that the
    // `N` line joined.
    let components = card
        .name
        .as_ref()
        .and_then(|name| name.components.as_ref())
        .unwrap_or_else(|| panic!("the card on the server lost its name: {card:?}"));
    assert_eq!(
        components
            .iter()
            .filter(|component| component.kind == "given")
            .map(|component| component.value.as_str())
            .collect::<Vec<_>>(),
        EDITED_GIVEN_PARTS.map(|(value, _)| value).to_vec(),
        "choosing a picture rewrote the name: {card:?}"
    );
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The calendar URI that the user types. Its host is different from the
/// free/busy address, so a save that patched the wrong entry cannot pass by
/// chance.
const RETYPED_CALENDAR_URI: &str = "https://calendars.example/jp/personal.ics";

/// The fifth leg: the user changes the calendar address, on a card with both
/// calendar resources under server keys.
///
/// The save must patch the edited entry and keep the other entry. The mapping's
/// own tests cannot show this. `E_CONTACT_CALENDAR_URI` and
/// `E_CONTACT_FREEBUSY_URL` are plain vCard attributes, not synthetic fields. A
/// throwaway C probe against libebook-contacts measured that a set changes the
/// *value* of the first line of that name and keeps its parameters, including
/// `X-JMAP-KEY`. This leg uses the path that Evolution uses, through the
/// daemons. If the key survives, the save can patch by key and does not need to
/// pair a keyless URI with the old one, as for a picture.
///
/// There are two entries because they go on lines with *different* names. The
/// free/busy address shows that a patch to `calendars/calendar-1` did not
/// replace the full map. Its unchanged URI shows that EDS wrote the second line
/// from the cached card.
#[test]
fn retyping_the_calendar_address_through_eds_patches_the_entry_it_replaces() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/address-book-recalendar"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(
        &client,
        &[
            "jmap-functional",
            "recalendar",
            card_id.as_str(),
            RETYPED_CALENDAR_URI,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // How EDS read the two lines from the emitter. The write leg cannot check
    // this direction, because there EDS wrote the lines. Each URI must be in
    // the field of its kind.
    assert_eq!(
        seen.get("read-calendar-uri"),
        Some(&SEEDED_CALENDAR_URI),
        "EDS did not read the calendar address off the line the emitter wrote\n{report}"
    );
    assert_eq!(
        seen.get("read-freebusy-uri"),
        Some(&SEEDED_FREEBUSY_URI),
        "EDS did not read the free/busy address off the line the emitter wrote\n{report}"
    );

    // After the save, EDS has the address that the user typed and the free/busy
    // address that the user did not change.
    assert_eq!(
        seen.get("read-back-calendar-uri"),
        Some(&RETYPED_CALENDAR_URI),
        "the calendar address the user typed did not survive the save\n{report}"
    );
    assert_eq!(
        seen.get("read-back-freebusy-uri"),
        Some(&SEEDED_FREEBUSY_URI),
        "the save changed the free/busy address nobody edited\n{report}"
    );

    // The other end, and the necessary assertion: still two entries, under the
    // two *server* keys. The edited entry has the new URI and still has the
    // `pref` that no line can hold. A third entry (the new URI under a `c1`
    // that the reader made by counting lines) shows that the key did not
    // survive EDS.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the new calendar address never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    let calendars = card
        .calendars
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's calendars: {card:?}"));
    assert_eq!(
        calendars.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_CALENDAR_KEY, SEEDED_FREEBUSY_KEY],
        "the calendar address the user typed was filed beside the old one \
         instead of over it: {card:?}"
    );
    let calendar = calendars
        .get(SEEDED_CALENDAR_KEY)
        .expect("the seeded calendar, just checked");
    assert_eq!(calendar.kind.as_deref(), Some("calendar"), "{card:?}");
    assert_eq!(
        calendar.uri, RETYPED_CALENDAR_URI,
        "the calendar address the user typed did not reach the server: {card:?}"
    );
    assert_eq!(
        calendar.pref.or_else(|| calendar
            .extra
            .get("pref")
            .and_then(|v| v.as_u64().map(|n| n as u32))),
        Some(SEEDED_CALENDAR_PREF),
        "the save replaced the entry instead of patching it: {card:?}"
    );
    assert_the_seeded_freebusy_survived(card);
    // The properties that nobody changed, asserted for the reason that the
    // other legs give.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The spouse name that the user types. It is a different person, not a
/// corrected spelling. The name *is* the key, so the save cannot see the
/// difference: it moves the marriage in both cases.
const RETYPED_SPOUSE: &str = "Marianne Oldenburg";

/// The sixth leg: the user retypes the spouse, on a card that relates to two
/// people on the server.
///
/// This is the one mapped property where the line shows the *key*. Only real
/// EDS shows whether `e_contact_set` on `E_CONTACT_SPOUSE` changes the one
/// `X-EVOLUTION-SPOUSE` line in place, or adds a new line next to the old one.
/// A second line goes to the server as a second marriage. The mapping cannot
/// see the difference from a card that really states two, because it has only
/// the lines.
///
/// There are two entries because the removal of a marriage is not the removal
/// of the map. No field shows the sibling's type. A save that removed the full
/// property, or replaced it with what the vCard can state, deletes a relation
/// that the user did not see.
#[test]
fn retyping_the_spouse_through_eds_moves_the_marriage_to_the_name_typed() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/address-book-respouse"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(
        &client,
        &[
            "jmap-functional",
            "respouse",
            card_id.as_str(),
            RETYPED_SPOUSE,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // How EDS read the line from the emitter. The write leg cannot check this
    // direction. On the server the name is the entry's *key*, so this checks
    // the change from key to value. If the spouse is missing here, the user
    // cannot see it.
    assert_eq!(
        seen.get("read-spouse"),
        Some(&SEEDED_SPOUSE),
        "EDS did not read the spouse off the line the emitter wrote\n{report}"
    );

    // After the save, EDS has the name that the user typed, one time. Two names
    // joined here show two spouse lines, which is the failure that this leg
    // looks for.
    assert_eq!(
        seen.get("read-back-spouse"),
        Some(&RETYPED_SPOUSE),
        "the spouse the user typed did not survive the save\n{report}"
    );

    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the new spouse never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    // The other end, and the necessary assertion: still two entries. The
    // sibling is unchanged, and the marriage has the new name as its key. Three
    // entries show that the old marriage remains. Two entries that include the
    // old spouse show that the save patched the wrong entry.
    let related = card
        .related_to
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped everyone the card relates to: {card:?}"));
    assert_eq!(
        related.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_SIBLING, RETYPED_SPOUSE],
        "the spouse the user typed was filed beside the old one instead of \
         over them: {card:?}"
    );
    assert_eq!(
        related[RETYPED_SPOUSE],
        relation_of("spouse"),
        "the name the user typed reached the server as something other than a \
         marriage: {card:?}"
    );
    assert_the_seeded_sibling_survived(card);
    // The properties that nobody changed, asserted for the reason that the
    // other legs give.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The seventh leg: the user *clears* the spouse field, on the same card.
///
/// This is the only leg where the edited card states no relations. The other
/// legs give the save a `relatedTo` that holds something. Before this leg, only
/// fixtures tested the path where the property is gone from the read-back
/// vCard: a removal with no replacement.
///
/// The fixtures also use an *inference* that only the daemons can confirm.
/// `as_evolution_retypes_the_spouse` in the `jmap-book-sync` tests assumes that
/// a cleared field leaves an empty `X-EVOLUTION-SPOUSE` line and does not
/// remove the attribute. The save removes the marriage in both cases, because
/// the reader refuses a line that names nobody. The client reports what the
/// card holds before the save. Against libebook-contacts 3.52 it reported the
/// empty line: `cleared-spouse-line=present`, `cleared-spouse-line-value=`
/// (empty).
///
/// The test prints that observation and does not assert it. The assertion below
/// is true for all versions: the *field* that Evolution shows must be empty. An
/// assertion on the attribute would make a valid change in libebook-contacts
/// look like a bug here, and both shapes cause the same removal. This was also
/// measured: the same EDS removes the attribute when the field is set to `NULL`
/// instead of the empty string, and this leg passes against that card too.
///
/// The sibling makes the leg useful. A cleared field removes one marriage. A
/// save that removed the full property (`relatedTo: null`) also deletes a
/// relation that no field shows and the user did not clear.
#[test]
fn clearing_the_spouse_through_eds_withdraws_the_marriage_and_keeps_the_brother() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/address-book-unspouse"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(&client, &["jmap-functional", "unspouse", card_id.as_str()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // The original marriage, so that the leg shows what the user cleared. If
    // EDS did not read the spouse, all assertions below pass for the wrong
    // reason.
    assert_eq!(
        seen.get("read-spouse"),
        Some(&SEEDED_SPOUSE),
        "EDS did not read the spouse off the line the emitter wrote\n{report}"
    );

    // The card that the save gets: its Spouse field is empty. The fixtures
    // could not measure this (see the doc comment). This finds an EDS that
    // keeps the old name on a line that the field does not show, which goes to
    // the server as a marriage that nobody removed.
    assert_eq!(
        seen.get("cleared-spouse"),
        Some(&""),
        "EDS kept a spouse on the card after the field was cleared\n{report}"
    );

    // After the save, the cache file of the card that the backend made again
    // still names nobody. A name here shows that the server kept the marriage.
    assert_eq!(
        seen.get("read-back-spouse"),
        Some(&""),
        "the cleared Spouse field came back filled in\n{report}"
    );

    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the withdrawal never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    // The other end, and the necessary assertion: only the sibling, with its
    // original type. An entry keyed by the old spouse shows that the marriage
    // was not removed. `relatedTo` must still exist: if it is gone, the sibling
    // is gone too.
    let related = card
        .related_to
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped everyone the card relates to: {card:?}"));
    assert_eq!(
        related.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_SIBLING],
        "clearing the Spouse field did not withdraw exactly the marriage: {card:?}"
    );
    assert_the_seeded_sibling_survived(card);
    // The properties that nobody changed, asserted for the reason that the
    // other legs give.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The note that the user types. It is not a changed spelling of a seeded note,
/// so a save that patched the wrong entry, or replaced the map, cannot pass by
/// chance.
const RETYPED_NOTE: &str = "met in Ghent, and paid up at last";

/// The eighth leg: the user retypes the note on a card with **two** notes on
/// the server.
///
/// Here the user sees part of a map and edits it. `E_CONTACT_NOTE` is the Notes
/// field of Evolution, and it is the *first* `NOTE` line. Each `notes` entry
/// writes a line, so a card with two notes shows one note and hides the other.
/// The test examines what happens to the hidden entry.
///
/// This leg examines two things that only real EDS can show:
///
/// - Does `e_contact_set` on a plain vCard attribute change the first line of
///   that name **in place** and keep its parameters (including `X-JMAP-KEY`)?
///   Or does it remove the key, as it does for a `PHOTO`? Without the key, the
///   new text goes to the server under an `n1` that the reader made by counting
///   lines, and the old note is deleted.
/// - Does the *second* line survive the set? If not, a user who edits the note
///   in Evolution deletes a note that they did not see. Only the daemons can
///   show this, because the save gets the card from EDS, not from this
///   repository.
#[test]
fn retyping_the_note_through_eds_patches_the_entry_it_replaces() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/address-book-renote"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(
        &client,
        &["jmap-functional", "renote", card_id.as_str(), RETYPED_NOTE],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // How EDS read the two lines from the emitter. The field shows the first
    // note. The line count shows what the field cannot: a card with one note
    // and a card with five notes look the same in the field.
    assert_eq!(
        seen.get("read-note"),
        Some(&SEEDED_NOTE),
        "EDS did not read the note off the first line the emitter wrote\n{report}"
    );
    assert_eq!(
        seen.get("read-note-lines"),
        Some(&"2"),
        "EDS did not keep both of the emitter's NOTE lines on the card\n{report}"
    );

    // What the set left on the card, which is the purpose of this leg: one line
    // changed and one line unchanged. A `1` shows that `e_contact_set` replaced
    // all lines of the name and deleted a hidden note. A `3` shows that it
    // added a line, which goes to the server as a note that nobody typed.
    assert_eq!(
        seen.get("retyped-note-lines"),
        Some(&"2"),
        "setting the Notes field did not leave the card's two NOTE lines \
         standing\n{report}"
    );
    assert_eq!(
        seen.get("read-back-note"),
        Some(&RETYPED_NOTE),
        "the note the user typed did not survive the save\n{report}"
    );

    // The other end, and the necessary assertion: still two entries under the
    // two *server* keys. The edited entry has the new text and still has the
    // `created` that no line can hold. A third entry (the new text under an
    // `n1` that the reader made by counting lines) shows that the key did not
    // survive EDS.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the new note never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    let notes = card
        .notes
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's notes: {card:?}"));
    assert_eq!(
        notes.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_NOTE_KEY, SEEDED_SECOND_NOTE_KEY],
        "the note the user typed was filed beside the old one instead of over \
         it: {card:?}"
    );
    let note = notes
        .get(SEEDED_NOTE_KEY)
        .expect("the seeded note, just checked");
    assert_eq!(
        note.note, RETYPED_NOTE,
        "the note the user typed did not reach the server: {card:?}"
    );
    assert_eq!(
        note.created
            .as_ref()
            .map(|d| d.as_str())
            .or_else(|| note.extra.get("created").and_then(|v| v.as_str())),
        Some(SEEDED_NOTE_CREATED),
        "the save replaced the entry instead of patching it: {card:?}"
    );
    assert_the_seeded_second_note_survived(card);
    // The properties that nobody changed, asserted for the reason that the
    // other legs give.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The separator that the client uses to join the values of the card's `NOTE`
/// lines. No seeded note holds it, and vCard gives it no structural meaning.
/// The notes hold the semicolon and the comma for the test, so neither can be
/// the separator.
const NOTE_LINE_SEPARATOR: &str = "|";

/// The ninth leg: the user **clears** the Notes field on a card with two notes
/// on the server.
///
/// This is the other half of
/// [`retyping_the_note_through_eds_patches_the_entry_it_replaces`], and no
/// other leg uses this branch of the save: the removal of one map entry through
/// a field that cannot show the map. The hidden note shows that the save
/// removed the visible entry and not the full property. The hidden note has its
/// own line, so a save that removed all of `notes` deletes a note that the user
/// did not see.
///
/// The client sets the empty string, not NULL, because Evolution's contact
/// editor gives `e_contact_set` the text of the field. What EDS then leaves on
/// the card is the measurement of this leg, and the client reports it without a
/// judgement. The spouse leg found that libebook-contacts 3.52 keeps the
/// *attribute* with no value. That result is about a different property. In
/// both shapes the note is removed, because the reader refuses an empty line.
/// Thus the assertions are true for both shapes: the field is empty, and the
/// only `NOTE` value on the card is the hidden note.
#[test]
fn clearing_the_note_through_eds_withdraws_it_and_keeps_the_one_behind_it() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/address-book-unnote"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(&client, &["jmap-functional", "unnote", card_id.as_str()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // The original card, so that the leg shows what the user cleared: the note
    // in the field, and the two lines that the field cannot count.
    assert_eq!(
        seen.get("read-note"),
        Some(&SEEDED_NOTE),
        "EDS did not read the note off the first line the emitter wrote\n{report}"
    );
    assert_eq!(
        seen.get("read-note-lines"),
        Some(&"2"),
        "EDS did not keep both of the emitter's NOTE lines on the card\n{report}"
    );

    // The card that the save gets. First the field, which is true for all
    // versions: if it still shows the old text, the clear did not occur.
    assert_eq!(
        seen.get("cleared-note"),
        Some(&""),
        "EDS kept a note in the field after it was cleared\n{report}"
    );

    // Then all `NOTE` values on that card. The field shows only the first line,
    // so it cannot show what else is on the card. Compare only values that are
    // not empty. Whether EDS removes the cleared line or keeps it with no value
    // is the business of libebook-contacts, and the mapping removes the note in
    // both cases. Against libebook-contacts 3.52, EDS keeps the line:
    // `cleared-note-lines` reads 2. The spouse leg found the same shape, and
    // the fixture `emptying_one_note_line_of_two_withdraws_that_note_alone`
    // states it.
    let cleared_lines = seen
        .get("cleared-note-line-values")
        .unwrap_or_else(|| panic!("the client did not report the card's NOTE lines\n{report}"));
    assert_eq!(
        cleared_lines
            .split(NOTE_LINE_SEPARATOR)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>(),
        vec![SEEDED_SECOND_NOTE],
        "clearing the Notes field did not leave exactly the note behind it \
         standing\n{report}"
    );

    // After the save, the cache of the card that the backend made again has one
    // line: the hidden note. Thus clearing the field *shows* the hidden note,
    // which is surprising but correct: the field is the first `NOTE` line, and
    // after the removal that is the second note. The old text here shows that
    // the removal did not get to the server. An empty field shows that the
    // second note was deleted too.
    assert_eq!(
        seen.get("read-back-note"),
        Some(&SEEDED_SECOND_NOTE),
        "the note behind the cleared one did not come back in its place\n{report}"
    );
    assert_eq!(
        seen.get("read-back-note-lines"),
        Some(&"1"),
        "the save left the card holding a number of notes nobody asked for\n{report}"
    );

    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the withdrawal never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    // The other end, and the necessary assertion: only the second note, under
    // the *server* key. A missing `notes` shows that the save removed the full
    // property. An entry under the first note's key shows that the save did not
    // remove the note.
    let notes = card
        .notes
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's notes: {card:?}"));
    assert_eq!(
        notes.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![SEEDED_SECOND_NOTE_KEY],
        "clearing the Notes field did not withdraw exactly the note it showed: {card:?}"
    );
    assert_the_seeded_second_note_survived(card);
    // The properties that nobody changed, asserted for the reason that the
    // other legs give.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The tenth leg: the user clears the Notes field on a card with **one** note
/// on the server.
///
/// This is the third shape of a cleared Notes field, and no other leg uses it.
/// The ninth leg clears a note with a hidden note behind it, so the save
/// removes the visible entry: `notes/note-1: null`. Here no note remains, so
/// the save must remove the property: `notes: null`, one patch member. The
/// mapping counts the hidden entries before it decides. Before this leg, only
/// fixtures tested this branch.
///
/// This is the only leg that asks two questions of a card with one note, and
/// only real EDS can answer them:
///
/// - Does the cleared field leave an empty `NOTE` line or no `NOTE` line? The
///   note is removed in both cases, because the reader refuses an empty line.
///   Thus the client reports the count and the test examines the *values*. The
///   shape decides which side of the mapping removes the note, and the earlier
///   measurement used a two-note card only.
/// - Is the field empty for the user after the save? On the ninth leg's card
///   the hidden note takes the place of the cleared note, so the field looks as
///   if it did not clear. Here nothing can take its place. An empty field shows
///   that the result of the ninth leg comes from the second note, not from a
///   save that refused the removal.
#[test]
fn clearing_the_only_note_through_eds_withdraws_the_whole_property() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_single_note_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/address-book-unnote-only"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(&client, &["jmap-functional", "unnote", card_id.as_str()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // The original card, so that the leg shows what the user cleared, and the
    // line count, which identifies this card. A `2` shows that the ninth leg's
    // card was seeded instead, and all assertions below then test the wrong
    // branch.
    assert_eq!(
        seen.get("read-note"),
        Some(&SEEDED_NOTE),
        "EDS did not read the note off the line the emitter wrote\n{report}"
    );
    assert_eq!(
        seen.get("read-note-lines"),
        Some(&"1"),
        "EDS did not hold exactly the one NOTE line the emitter wrote\n{report}"
    );

    // The card that the save gets. First the field, which is true for all
    // versions: if it still shows the old text, the clear did not occur.
    assert_eq!(
        seen.get("cleared-note"),
        Some(&""),
        "EDS kept a note in the field after it was cleared\n{report}"
    );

    // Then all `NOTE` values on the card. This time no hidden note remains, so
    // the card must hold no note text. Whether that is an empty line or no line
    // is the business of libebook-contacts, and the mapping removes the note in
    // both cases. Thus the test examines the values and reports the count.
    // Against libebook-contacts 3.52 the line stays: `cleared-note-lines` reads
    // 1. The fixture `emptying_the_only_note_line_withdraws_the_property`
    // states this shape.
    let cleared_lines = seen
        .get("cleared-note-line-values")
        .unwrap_or_else(|| panic!("the client did not report the card's NOTE lines\n{report}"));
    assert!(
        cleared_lines
            .split(NOTE_LINE_SEPARATOR)
            .all(|value| value.is_empty()),
        "clearing the only Notes field left a note on the card: \
         {cleared_lines:?}\n{report}"
    );

    // After the save, the cache of the card that the backend made again has no
    // note and no `NOTE` line. In the ninth leg the hidden note came into the
    // field. Here the field is really empty, because no note remains.
    assert_eq!(
        seen.get("read-back-note"),
        Some(&""),
        "the cleared Notes field came back filled in\n{report}"
    );
    assert_eq!(
        seen.get("read-back-note-lines"),
        Some(&"0"),
        "the save left a NOTE line on a card that has no notes\n{report}"
    );

    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the withdrawal never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    // The other end, and the necessary assertion: `notes` is gone from the
    // card. An empty map is the property with no content. An entry under the
    // note's key shows that the save did not remove the note.
    assert_eq!(
        card.notes, None,
        "clearing the only note did not withdraw the property: {card:?}"
    );
    // The properties that nobody changed, asserted for the reason that the
    // other legs give. This is more important here, because the patch of this
    // leg names a full property, not one entry.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The eleventh leg: the user retypes an instant-messaging handle that the
/// server stated as a **URI**, and the save must write it back as a URI.
///
/// This is the one mapped property where the server can state a value in a form
/// that the card cannot hold. RFC 9553 §2.3.2 lets an `onlineServices` entry
/// name the contact with a `user`, a `uri`, or both. Evolution's
/// instant-messaging fields hold only a handle. The user sees the entry only
/// because an `xmpp:` URI holds only the JID, so the reader can get the handle
/// from the URI. The save writes the edit back to the member that the handle
/// came from, and does not change the shape of the entry. Before this leg, only
/// fixtures tested this path, with a vCard that this repository wrote.
///
/// Three things that only real EDS can show:
///
/// - Does the handle get to the field? The reader selects the slot
///   (`TYPE=HOME`), and `E_CONTACT_IM_JABBER_HOME_1` is the field of that slot.
///   Whether libebook-contacts reads a handle from the parameter that the
///   emitter writes is a claim about EDS. The user cannot see a handle in the
///   wrong slot, or on a line with no `TYPE`.
/// - What does a set on that field leave on the card: one `X-JABBER` line or
///   two, with or without the `X-JMAP-KEY`? A second line tells the mapping
///   (which has only the lines) that the contact is at that service two times.
///   A lost key means that the save cannot patch by key and must pair a keyless
///   line with the entry. Against libebook-contacts 3.52 the set changes the
///   line in place and keeps its parameters: `retyped-im-handle-lines=1` and
///   `retyped-im-handle-key=handle-1`. The client reports both and the test
///   does not assert them, for the same reason as the spouse line: the save
///   gets to the same entry in both cases. The assertion below examines what
///   the *server* holds.
/// - From the nine legs that edit a different property: a save keeps the entry
///   in the shape that the server gave it. The line returns a `user` where the
///   server holds a `uri`, so an entry written again in the shape of the line
///   is incorrect. [`assert_the_seeded_service_survived`] checks this, and also
///   gives the failure that it cannot find.
///
/// Two mutations make only this leg fail in this file: a save that writes the
/// new handle to `user` instead of making the URI again, and the removal of
/// `xmpp` from the reader's scheme table, which leaves the field empty. The
/// `jmap-book-sync` fixtures also find both. This leg adds the *input*: the
/// fixtures state the card that a URI-only entry makes, and only the daemons
/// can show that EDS makes it.
#[test]
fn retyping_a_uri_only_handle_through_eds_writes_the_uri_back() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_double_barrelled_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/address-book-rehandle"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(
        &client,
        &[
            "jmap-functional",
            "rehandle",
            card_id.as_str(),
            RETYPED_HANDLE,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // The first of the three: the handle, which the server did not state as a
    // handle, read from the field that Evolution's contact editor shows. This
    // checks the full URI half of the mapping against real EDS: the reader knew
    // the scheme, got the handle from the URI, wrote the line with the `TYPE`
    // of this field, and EDS put the handle in that field.
    assert_eq!(
        seen.get("read-im-handle"),
        Some(&SEEDED_SERVICE_HANDLE),
        "EDS did not read the handle off the line the emitter drew from the \
         URI\n{report}"
    );
    assert_eq!(
        seen.get("read-im-handle-lines"),
        Some(&"1"),
        "EDS did not hold exactly the one X-JABBER line the emitter wrote\n{report}"
    );

    // After the save, the cache of the card that the backend made again has the
    // handle that the user typed, one time. Two handles joined here show that
    // the card states the contact at the service two times.
    assert_eq!(
        seen.get("read-back-im-handle"),
        Some(&RETYPED_HANDLE),
        "the handle the user typed did not survive the save\n{report}"
    );
    assert_eq!(
        seen.get("read-back-im-handle-lines"),
        Some(&"1"),
        "the save left the old handle on the card beside the new one\n{report}"
    );

    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the new handle never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the mock's default account")
        .contact_cards
        .get(&card_id)
        .expect("the seeded card is still there");

    // The other end, and the necessary assertion: the entry keeps the server's
    // key and service, and the new handle is in the member that it came from. A
    // `user` here shows that the save changed the shape of the entry. A second
    // entry shows that the save read the new handle as a different service.
    let services = card
        .online_services
        .as_ref()
        .unwrap_or_else(|| panic!("the save dropped the card's online services: {card:?}"));
    assert_eq!(
        services.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            SEEDED_SERVICE_KEY,
            SEEDED_SKYPE_SERVICE_KEY,
            SEEDED_MATRIX_SERVICE_KEY,
        ],
        "the handle the user typed was filed beside the entry it replaced: {card:?}"
    );
    let service = &services[SEEDED_SERVICE_KEY];
    assert_eq!(
        service.service.as_deref(),
        Some(SEEDED_SERVICE),
        "the save moved the handle to another service: {card:?}"
    );
    assert_eq!(
        service.uri.as_deref(),
        Some(RETYPED_SERVICE_URI),
        "the retyped handle did not go back onto the URI it was drawn from: {card:?}"
    );
    assert_eq!(
        service.user, None,
        "the save answered a card stating a URI with one stating a handle: {card:?}"
    );

    let s2 = &services[SEEDED_SKYPE_SERVICE_KEY];
    assert_eq!(
        s2.service.as_deref(),
        Some(SEEDED_SKYPE_SERVICE),
        "{card:?}"
    );
    assert_eq!(
        s2.uri.as_deref(),
        Some(SEEDED_SKYPE_SERVICE_URI),
        "{card:?}"
    );

    let s3 = &services[SEEDED_MATRIX_SERVICE_KEY];
    assert_eq!(
        s3.service.as_deref(),
        Some(SEEDED_MATRIX_SERVICE),
        "{card:?}"
    );
    assert_eq!(
        s3.user.as_deref(),
        Some(SEEDED_MATRIX_SERVICE_HANDLE),
        "{card:?}"
    );

    // The properties that nobody changed, asserted for the reason that the
    // other legs give.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The name and email address of the card that the `remove` leg deletes. Simple
/// values, because this leg does not test the mapping: it tests that
/// `remove_contact_sync` gets to the server.
const REMOVABLE_FULL_NAME: &str = "Alex Krycek";
const REMOVABLE_EMAIL: &str = "alex@example.com";

/// Seed one minimal card into the mock's store, directly and not through EDS,
/// as `seed_card` does. This leg tests what EDS does with a card that it did
/// not write.
fn seed_removable_card(server: &jmap_mock::MockServer) -> Id {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().expect("mock state lock");
    let account = state
        .account_mut(&account_id)
        .expect("the mock's default account");
    let book = account.seed_address_book("Personal", true);

    let id = account.contact_cards.alloc_id();
    let mut card = ContactCard::simple(book, REMOVABLE_FULL_NAME, REMOVABLE_EMAIL);
    card.id = Some(id.clone());
    card.uid = Some(format!("urn:example:card:{}", id.as_str()));
    account.contact_cards.seed_with_id(id.clone(), card);
    id
}

/// The twelfth leg: `remove_contact_sync`. It is the address-book equivalent of
/// the recurring-occurrence delete that `calendar.rs` and `cal-client.c` do on
/// the calendar side. The eleven other legs in this file save an edit and do
/// not get to this vfunc.
///
/// Both ends must agree. The EDS cache must not have the card: a backend that
/// returned success without sending the destroy still returns the card on the
/// next get. The server must not have the card: a backend that removed only its
/// cache entry leaves a card that the next refresh adds again.
#[test]
fn removing_a_contact_through_eds_reaches_the_server_and_the_cache() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let card_id = seed_removable_card(&server);
    let port = mock_port(&server);

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/address-book-remove"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let output = session.run(&client, &["jmap-functional", "remove", card_id.as_str()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check the connect first, for the reason that the first leg gives: a
    // read-only or disconnected book makes all later failure messages
    // misleading.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "EDS opened the book read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("removed"),
        Some(&"1"),
        "e_book_client_remove_contact_by_uid_sync did not report success\n{report}"
    );
    assert_eq!(
        seen.get("gone"),
        Some(&"1"),
        "EDS's own cache still hands back the removed contact\n{report}"
    );

    // The other end: the server. Check `method_calls()` only for a
    // `ContactCard/set` (a destroy is a `ContactCard/set`, as all writes in
    // this file are). The necessary claim is the *result*: the card is not in
    // the store.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "ContactCard/set"),
        "the removal never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let account = state
        .account(&account_id)
        .expect("the mock's default account");
    assert!(
        account.contact_cards.get(&card_id).is_none(),
        "the server still holds the card EDS reported gone: {:?}",
        account.contact_cards.get(&card_id)
    );
}
