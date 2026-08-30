// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, address book: `evolution-addressbook-factory` loading
//! `libebookbackendjmap.so`, opening a book from a `.source` keyfile, and
//! serving a write through it to the mock JMAP server.
//!
//! Everything here is checked from the two ends and nothing in between: the
//! client program says what EDS gave a libebook consumer, the mock says what
//! the backend asked the server for. Neither end knows about the other, so
//! an assertion that holds on both is a claim about the whole path.
//!
//! Eleven legs, because they need eleven books. The first starts empty and
//! writes a contact into it. The other ten each start from a card the mock was
//! seeded with before EDS ever connected — a card from the *server*, holding a
//! shape no vCard can state, which is the only way to ask what real EDS does to
//! it — and take the branches a save can take with it: the user edits a field
//! beside the name, retypes the name itself, picks a new picture, retypes their
//! calendar address, retypes who they are married to, clears that field
//! altogether, retypes the note on a card carrying two of them, clears that
//! field too, clears it on a card whose only note it was, or retypes an
//! instant-messaging handle the server stated as a URI.

use std::collections::BTreeMap;

use jmap_functional::{Session, observations, required_path};
use jmap_proto::Id;
use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, Media, Name, NameComponent,
    Note, OnlineService, OrgUnit, Organization, Relation, Title,
};

/// The contact the client writes. One string, passed to the client on its
/// command line and looked for in the mock's store, so the two ends cannot
/// disagree about it by a typo.
const FULL_NAME: &str = "Dana Scully";
const EMAIL: &str = "dana@example.com";
/// The nickname, spelled as `book-client.c` spells it. The comma is the
/// point: RFC 2426 §3.1.3 states the nicknames as a comma-separated list, so
/// this is where "one nickname stays one nickname" is checked against the EDS
/// that writes the line — a split would reach the server as two entries.
const NICKNAME: &str = "Vee, the tall one";
/// The employer and department the client sets, spelled as `book-client.c`
/// spells them. Together they are one `ORG` line, which is what makes them
/// worth asserting here: whether EDS's two fields and JSContact's
/// `organizations` map meet in the middle is a claim about real EDS, not one
/// the mapping's own tests can make.
const ORG: &str = "Acme Ltd";
const ORG_UNIT: &str = "Research";
/// The job title and the role, likewise spelled as `book-client.c` spells
/// them. These are the two halves of JSContact's `titles` map, which vCard
/// splits across `TITLE` and `ROLE` — so this end of the leg is where the
/// `kind` that tells them apart is shown to survive real EDS.
const TITLE: &str = "Research Scientist";
const ROLE: &str = "Project Lead";
/// The postal address, spelled as `book-client.c` spells it. EDS keeps it in
/// the fields of one `ADR` line and JSContact in a list of named components,
/// so this is where the positional half of that mapping — which field means
/// which kind — is checked against the EDS that actually writes the line,
/// rather than against our own reading of the RFC.
const STREET: &str = "Hauptstrasse 1";
const LOCALITY: &str = "Berlin";
const POSTCODE: &str = "10115";
const COUNTRY: &str = "Germany";
/// The same address written out for an envelope, which EDS keeps on a `LABEL`
/// line and JSContact as the address's `full`. This is the leg's one claim
/// about a pairing rather than about a value: `E_CONTACT_ADDRESS_LABEL_WORK`
/// is a synthetic field, so the `X-JMAP-KEY` naming the address a label
/// belongs to does not survive EDS, and only the shared `TYPE` says the label
/// and the `ADR` are two views of one address. If that fell through, the
/// server would end up holding two addresses instead of one.
const ADDRESS_LABEL: &str = "Hauptstrasse 1\n10115 Berlin\nGermany";
/// The free-text note, spelled as `book-client.c` spells it. The semicolon
/// and the comma in it are the point: vCard gives both structural meaning,
/// and this is the one mapped property that holds prose, so what is checked
/// here is that the escaping our emitter applies is the escaping the EDS
/// reading the line back undoes.
const NOTE: &str = "met at FOSDEM; owes me a beer, apparently";
/// The home page, spelled as `book-client.c` spells it. EDS keeps it on the
/// first `URL` line and JSContact as an entry of the `links` map, and the comma
/// in the query string is why it is worth a leg of its own: a vCard value gives
/// the comma structural meaning and EDS escapes it, so this is where the URI is
/// shown to reach the server as the one the user typed — neither cut off at the
/// comma nor carrying the backslash EDS wrote.
const HOMEPAGE: &str = "https://dana.example/profile?tags=x-files,ufo";
/// The contact's own calendar and the free/busy data drawn from it, spelled as
/// `book-client.c` spells them. EDS keeps them on `CALURI` and `FBURL` and
/// JSContact keeps both in the one `calendars` map, told apart by a `kind` no
/// line carries — so what only real EDS can answer is whether the two fields
/// Evolution shows as Calendar and Free/Busy are the two lines the emitter
/// writes, and whether the reader puts each URI back under the kind it came
/// off. A URI that crossed under the wrong kind would be shown to the user as
/// the other resource, and the two fields sit next to each other.
const CALENDAR_URI: &str = "https://dana.example/cal/dana.ics";
const FREEBUSY_URI: &str = "https://dana.example/fb/dana.ifb";
/// Who the contact is married to, spelled as `book-client.c` spells it. EDS
/// keeps it on an `X-EVOLUTION-SPOUSE` line — vCard 3.0 has no `RELATED` — and
/// JSContact keeps it as the *key* of a `relatedTo` entry stating the type
/// `spouse`. So this is the one mapped property whose key crosses rather than
/// its value, and what only real EDS can answer is whether the field
/// Evolution shows as Spouse is the line the emitter writes, and whether the
/// name reaches the server as the entry's own key rather than as a value on
/// some entry keyed by something else.
const SPOUSE: &str = "Fox Mulder";
/// The instant-messaging handle the client sets, spelled as `book-client.c`
/// spells it. EDS keeps it on an `X-JABBER` line and JSContact as an
/// `onlineServices` entry, and two things about the crossing only real EDS can
/// answer: that the `TYPE` our emitter writes is the one EDS reads a handle back
/// out of — a line without one reaches no field at all — and that the comma
/// inside the handle survives, since a JSContact `user` is free text while vCard
/// gives the comma structural meaning.
const IM_HANDLE: &str = "dana,scully@jabber.example";
/// The service that handle is at, spelled as the mapping's own table spells it.
/// The line states the service by *being* `X-JABBER`, so this is what the reader
/// files a handle the user typed under.
const IM_SERVICE: &str = "Jabber";
/// The two categories the client files the contact under, spelled as
/// `book-client.c` spells them, and the character it joins them with when it
/// reports them back. EDS keeps them as a list on one `CATEGORIES` line and
/// JSContact as the `keywords` Set, so what this end checks is a cardinality
/// rather than a value: two tags here must be two members there, and the comma
/// inside the second is what would split one into two if the escaping fell
/// through anywhere along the path.
const CATEGORY_ONE: &str = "Friends";
const CATEGORY_TWO: &str = "beer, in Berlin";
const CATEGORY_SEPARATOR: &str = "|";
/// The birthday, spelled as `book-client.c` spells it — the text
/// `e_contact_date_to_string()` writes for the three numbers it sets, which
/// is also the text the `BDAY` line carries. EDS keeps a birthday in a
/// structured field and rebuilds the line from it, so this is where the one
/// mapped property whose *value* changes shape on the way across — three
/// numbers to a date and back — is checked against the EDS that writes it.
const BIRTHDAY: &str = "1964-03-27";
const BIRTH_YEAR: u64 = 1964;
const BIRTH_MONTH: u64 = 3;
const BIRTH_DAY: u64 = 27;
/// The picture of the contact, as the bytes of a real 69-byte PNG — a 1×1 red
/// pixel — spelled once, here, and handed to the client on its command line
/// rather than written out in both languages: the bytes are the assertion, so
/// the two ends must not be able to disagree about them by a typo.
///
/// A *real* image because that is what a user's photo is, and because the bytes
/// of one are the case the mapping had to be built around: they are not valid
/// UTF-8, so calcard hands them back as binary rather than as text, and the
/// slash in the base64 below is there to be carried through a `data:` URI
/// untouched. The value is long enough that EDS folds the `PHOTO` line across
/// two, which is the other half of what only a real EDS can be asked about.
const PHOTO_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";
/// What the picture is, spelled as `book-client.c` spells it. EDS writes the
/// subtype alone on the line (`TYPE=png`, measured against libebook-contacts
/// 3.52) and rebuilds `image/png` when it reads one back, so this is the one
/// place where "the parameter our emitter writes is the media type EDS means"
/// is checked from both ends instead of against a probe.
const PHOTO_MEDIA_TYPE: &str = "image/png";
/// The picture the user picks instead, in the leg that replaces one: a second
/// real PNG, the same 1×1 pixel painted a different colour. Different bytes from
/// [`PHOTO_BASE64`] and the same length, so a save that wrote the old picture
/// back cannot pass by accident and neither can one that truncated the new.
const REPLACEMENT_PHOTO_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYPgPAAEDAQAIicLsAAAAAElFTkSuQmCC";

/// The keyfile from `docs/examples/jmap-mock.source`, with the mock's
/// ephemeral port filled in. Kept as a literal here rather than read from
/// `docs/` so that a change to the documented recipe fails this test loudly
/// instead of quietly retargeting it; `jmap-backend-book`'s `recipe.rs` is
/// what holds the documented file to what it claims to mean.
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional test\n\
         Enabled=true\n\
         \n\
         [Address Book]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

#[test]
fn evolution_opens_the_book_and_a_write_reaches_the_server() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // No `[Resource] Identity=` in the keyfile above, so the backend asks
    // the server for the account's default address book. Seeding one flagged
    // default is what makes that question answerable.
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

    // Checked before the exit status, because a read-only book turns every
    // later failure into "Permission denied" — a message about the write
    // that is really about the connect. EDS takes this from what the backend
    // said during `connect_sync`; a backend that connects and never claims
    // the book is writable gives an address book Evolution greys out.
    //
    // Unless the client never got this far, in which case the failure is
    // earlier than anything here — the module missing from the factory's
    // directory, say — and the exit status is what says so.
    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the book, with {}\n{report}",
            output.status
        )
    });
    // The other half of "EDS is satisfied with this backend": the source's
    // connection status, which is what Evolution's UI shows as a connected
    // account and what every EDS client that waits for a backend waits on.
    // `readonly=0` says the backend claimed the book writable; this says
    // EDS's own view of the connection agrees, rather than the source still
    // sitting in `connecting` or having fallen back to `disconnected`.
    // Asserted first, for the reason `calendar.rs` spells out.
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

    // Read back through EDS: what the meta backend kept of the write.
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
    // The two calendaring fields, read back out of the same two EDS keeps them
    // in. Swapped here would mean the `kind` had picked the wrong line, which
    // is a failure no single-field assertion could see.
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
    // Read out of the same per-context slot the client wrote it to. Missing
    // here would mean the handle had come back on a line with no `TYPE` — in
    // the vCard, and in none of the fields Evolution shows.
    assert_eq!(
        seen.get("read-back-jabber"),
        Some(&IM_HANDLE),
        "the contact EDS handed back lost or cut off its Jabber handle\n{report}"
    );
    // Both tags, in the order EDS's list holds them, joined as the client
    // joined them. An extra item here would be a tag that had been split on
    // its comma; a missing one would be a tag the round trip dropped.
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
    // The picture, out of EDS's own cache — and *not* as the bytes that went
    // in: `EBookMetaBackend` puts every contact it caches through
    // `store_inline_photos`, which writes the picture into a file under the
    // book's cache directory and leaves the line pointing at it. So what a
    // libebook consumer reads back is a `file:` URI, whatever it wrote, and the
    // question is whether the bytes at the end of it are the ones it wrote.
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
    // The extension EDS chose, which is the only place a cached photo still
    // says what it is: it comes from the media type EDS read off the line, so a
    // picture whose `TYPE` had fallen through would be filed under some other
    // suffix and be one Evolution loads by guessing.
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
    // The client escapes this one observation's line breaks, since the report
    // is read a line at a time.
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

    // And the other end: what the server was actually asked to do. The read
    // path is deliberately not asserted here — `EBookMetaBackend` schedules
    // its refresh rather than running it, so whether `ContactCard/query` has
    // happened by now is a race. The write is synchronous.
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
    // The ORG line EDS wrote, as the server sees it: one `organizations`
    // entry whose name is the employer and whose first unit is the
    // department — the crossing this leg exists to check on real EDS.
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
    // The TITLE and the ROLE, as the server sees them: two entries of one
    // `titles` map, and the `kind` is what says which line each came off.
    let titles = card
        .titles
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no titles: {card:?}"));
    // RFC 9553 §2.2.4's default is spelled out here rather than borrowed
    // from the mapping, so this end states the wire shape it expects instead
    // of agreeing with the code that produced it.
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
    // The ADR line, as the server sees it: one `addresses` entry whose
    // components name what each field of the line meant. The kinds are
    // spelled out here rather than borrowed from the mapping, so this end
    // states the wire shape it expects instead of agreeing with the code
    // that produced it.
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
    // EDS wrote TYPE=WORK, because the client set the work address; that is
    // the `contexts` member on this side.
    assert_eq!(
        address.contexts,
        Some(serde_json::json!({"work": true})),
        "{card:?}"
    );
    // The NOTE line, as the server sees it: one `notes` entry holding the
    // text the user typed, punctuation and all. Asserted at this end too
    // because the client's read-back comes out of EDS's own cache, which
    // would agree with itself even if the line that went to the server had
    // been mangled on the way.
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
    // The NICKNAME line, as the server sees it: exactly one `nicknames` entry
    // holding the whole text, comma included. Two entries here would mean the
    // comma had been read as RFC 2426 §3.1.3's list separator somewhere along
    // the way, and the server would have been told the user has two nicknames.
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
    // The URL line, as the server sees it: exactly one `links` entry holding
    // the whole URI, of the kind a `URL` states — which is none at all. A URI
    // ending at the comma would mean the escaping had fallen through between
    // EDS's writer and our reader, and the server would hold a link pointing
    // somewhere the user never typed.
    let links = card
        .links
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no links: {card:?}"));
    assert_eq!(links.len(), 1, "{links:?}");
    let link = links.values().next().expect("one link");
    assert_eq!(link.uri, HOMEPAGE, "{card:?}");
    assert_eq!(link.kind, None, "{card:?}");
    // The CALURI and FBURL lines, as the server sees them: two `calendars`
    // entries, each stating what it is. The kinds are spelled out here rather
    // than borrowed from the mapping, so this end states the wire shape it
    // expects instead of agreeing with the code that produced it — and they are
    // asserted as a pair, since a mapping that put both URIs under one kind
    // would still have two entries and the right two URIs.
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
    // The X-EVOLUTION-SPOUSE line, as the server sees it: one `relatedTo`
    // entry, keyed by the name the user typed and stating that they are married
    // to them. Both halves matter and neither alone would do — a name that
    // arrived as a *value* under a key of the mapping's own invention would be
    // an entity the server has never heard of, and an entry keyed right but
    // stating no type is RFC 9555 §2.9.5's relation of no kind, which says
    // nothing about a marriage.
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
    // The X-JABBER line, as the server sees it: one `onlineServices` entry
    // stating the service and the handle. Asserted at this end too because the
    // client's read-back comes out of EDS's own cache, which would agree with
    // itself even about a handle that had reached the server cut off at the
    // comma — and because the `user` is where it has to land: a `uri` would be
    // this mapping claiming the handle is an RFC 3986 URI.
    let services = card
        .online_services
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no onlineServices: {card:?}"));
    let service = services.values().next().expect("one online service");
    assert_eq!(services.len(), 1, "{card:?}");
    assert_eq!(service.service.as_deref(), Some(IM_SERVICE), "{card:?}");
    assert_eq!(service.user.as_deref(), Some(IM_HANDLE), "{card:?}");
    assert_eq!(service.uri, None, "{card:?}");
    // The CATEGORIES line, as the server sees it: exactly two `keywords`
    // members, each set to `true`. Three would mean the comma inside the second
    // tag had been read as RFC 2426 §3.7.1's list separator somewhere along the
    // way, and the server would have been told the contact is filed under a tag
    // nobody typed. Asserted at this end too because the client's read-back
    // comes out of EDS's own cache, which would agree with itself even if the
    // line that went to the server had been split.
    let keywords = card
        .keywords
        .as_ref()
        .unwrap_or_else(|| panic!("the card on the server has no keywords: {card:?}"));
    assert_eq!(
        keywords.keys().map(String::as_str).collect::<Vec<_>>(),
        // Sorted, which is how the map holds them: byte order, so the capital
        // `F` comes before the lower-case `b` rather than the order the client
        // set them in.
        vec![CATEGORY_ONE, CATEGORY_TWO],
        "{card:?}"
    );
    assert!(
        keywords.values().all(|set| *set == serde_json::json!(true)),
        "{card:?}"
    );
    // The PHOTO line, as the server sees it: one `media` entry of kind `photo`
    // whose URI carries the bytes themselves. Asserted at this end too because
    // the client's read-back comes out of EDS's own cache, which would agree
    // with itself about a picture that had reached the server truncated at the
    // fold, or with the base64 EDS wrote re-encoded into something else.
    //
    // The URI is spelled out rather than borrowed from the mapping, so this end
    // states the wire shape it expects: RFC 2397's `data:` with the media type
    // in front of the payload, which is where a server that has never seen a
    // vCard reads what the bytes are.
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
    // The BDAY line, as the server sees it: one `anniversaries` entry of kind
    // `birth` whose date is the three numbers the client set. Spelled out
    // here rather than borrowed from the mapping, so this end states the wire
    // shape it expects instead of agreeing with the code that produced it.
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

/// The name the second leg is about: a double-barrelled given name, which RFC
/// 9553 §2.2.1 states as **two** `given` components. The vCard `N` value has
/// one field per kind, so the emitter joins them with a space and EDS is handed
/// `N:Oldenburg;Jean Paul;;;` — one string where the server holds two parts.
const EDITED_SURNAME: &str = "Oldenburg";
/// The two halves, each with the pronunciation the server keeps for it. A
/// `phonetic` has no `N` field at all, so it is the part of the card that is
/// invisible to the user and therefore cannot have been edited by one: it is
/// gone only if the save deleted it.
const EDITED_GIVEN_PARTS: [(&str, &str); 2] = [("Jean", "zhon"), ("Paul", "pol")];
/// What EDS is expected to hand back for the given name: the two parts joined,
/// byte for byte as the emitter wrote them. This is the load-bearing claim of
/// the leg — the backend puts the components back by recognising that the field
/// still reads as its parts joined, which is a *string* comparison against this
/// text. If EDS normalised the whitespace in it, a name nobody touched would
/// read as retyped and both halves would be replaced by the field's text.
const EDITED_GIVEN_FIELD: &str = "Jean Paul";
const EDITED_FULL_NAME: &str = "Jean Paul Oldenburg";
/// The one field the user edits, before and after. Deliberately not part of the
/// name: whatever the save then does to the name is something nobody asked for.
const EDITED_EMAIL_BEFORE: &str = "jp@example.com";
const EDITED_EMAIL_AFTER: &str = "jp@example.org";
/// The key the *server* filed the seeded card's picture under. Nothing like a
/// key the reader invents, which counts entries off the vCard (`m1`, `m2`, …),
/// so a picture still filed under this one after a save is a picture that was
/// paired with the line it came off rather than re-added under a new name.
///
/// This is the half of the picture's mapping that no test below the daemons can
/// make a claim about: EDS rewrites the `PHOTO` line — dropping the key with
/// it — whenever the photo field is *set*, so whether a save has to put the key
/// back at all depends on what real EDS does to a card whose picture the user
/// never touched.
const SEEDED_PHOTO_KEY: &str = "picture-1";
/// An unmodeled logo resource on the seeded card, filed under a server-chosen
/// key, that gets no vCard line and must survive edits untouched.
const SEEDED_LOGO_KEY: &str = "logo-1";
const SEEDED_LOGO_URI: &str = "https://oldenburg.example/logo.png";
/// The keys the *server* filed the seeded card's two calendaring resources
/// under, and where each points. Nothing like the keys the reader invents
/// (`c1`, `c2`, …), for the reason [`SEEDED_PHOTO_KEY`] is not.
const SEEDED_CALENDAR_KEY: &str = "calendar-1";
const SEEDED_FREEBUSY_KEY: &str = "freebusy-1";
const SEEDED_CALENDAR_URI: &str = "https://oldenburg.example/cal/jp.ics";
const SEEDED_FREEBUSY_URI: &str = "https://oldenburg.example/fb/jp.ifb";
/// How strongly the server says the calendar is preferred. A `CALURI` has no
/// parameter for it, so it rides in the entry's `extra` and is the member that
/// says whether a save *patched* the entry or replaced it: a replacement would
/// hold the URI and nothing else.
const SEEDED_CALENDAR_PREF: u32 = 1;
/// Who the server says the seeded card's owner is married to. A key rather than
/// a value — RFC 9553 §2.1.8 keys `relatedTo` by the related entity itself — and
/// free text rather than a `uid`, which is what RFC 9555 §2.9.5 allows and what
/// makes the name showable on a line at all.
const SEEDED_SPOUSE: &str = "Marie Oldenburg";
/// Somebody else the server relates the card to, of a type Evolution has no
/// field for. The entry is therefore invisible to the user: it reaches no line,
/// so it cannot have been edited by one, and a save that dropped it dropped
/// something nobody asked it to. It is the `relatedTo` counterpart of the
/// `phonetic` beside it, and the reason the map is seeded with two entries
/// rather than one.
const SEEDED_SIBLING: &str = "Klaus Oldenburg";
/// The keys the *server* filed the seeded card's two notes under, and what each
/// says. Two of them, and that is the shape under test: Evolution's Notes field
/// is the **first** `NOTE` line and nothing else, so a card carrying two notes
/// is one where the user edits a single entry of a map they can see only part
/// of — and what a save does to the part they cannot see is what no fixture can
/// answer, because only real EDS says what is left on the card it hands back.
const SEEDED_NOTE_KEY: &str = "note-1";
const SEEDED_SECOND_NOTE_KEY: &str = "note-2";
/// The semicolon and the comma are here for the reason [`NOTE`] carries them: a
/// note is the one mapped property a user types prose into, and both characters
/// are ones vCard gives structural meaning to, so a note arriving at the server
/// cut off at either — or carrying the backslash EDS wrote — shows up here.
const SEEDED_NOTE: &str = "met in Ghent; still owes me a beer, apparently";
const SEEDED_SECOND_NOTE: &str = "do not call before 10:00, ever";
/// The key the *server* filed the seeded card's instant-messaging handle under,
/// the service it is at, and — the point of this one — the shape the server
/// states it in.
///
/// RFC 9553 §2.3.2 lets an `onlineServices` entry name the contact with a `user`,
/// a `uri`, or both, and Evolution's instant-messaging fields hold a handle. So
/// this entry states **only** the URI: it is the one mapped property whose value
/// the card cannot carry as it stands, and it reaches the user at all only
/// because `xmpp:` spells the JID and nothing else, which lets the reader draw
/// [`SEEDED_SERVICE_HANDLE`] out of [`SEEDED_SERVICE_URI`].
///
/// Seeded on the card every seeded leg starts from, because the shape is
/// invisible from the vCard side in both directions: the line a leg that touches
/// nothing hands back states a `user`, so a save comparing the *members* rather
/// than the handle they both spell would rewrite this entry every time the
/// contact is saved for any reason at all.
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
/// The handle the user retypes into that field, and the URI the save has to
/// rebuild around it. A different host as well as a different local part, so a
/// URI half-rewritten shows up as neither.
const RETYPED_HANDLE: &str = "jp@xmpp.example";
const RETYPED_SERVICE_URI: &str = "xmpp:jp@xmpp.example";
/// When the server says the first note was written. RFC 9553 §2.8.3's `created`
/// has no `NOTE` component and no parameter to sit in, so it rides in the
/// entry's `extra` — which makes it the member that says whether a save
/// *patched* the entry or replaced it: a replacement would hold the text alone.
const SEEDED_NOTE_CREATED: &str = "2026-02-01T09:30:00Z";
/// The keys and dates the *server* filed the seeded card's anniversaries under.
///
/// Three entries of different shapes: a wedding anniversary that reaches the
/// `X-EVOLUTION-ANNIVERSARY` line, a year-only birthday no vCard line can state,
/// and a deathday vCard 3.0 has no field for.
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

/// The multiple organizations and titles/roles the server filed on the seeded card.
///
/// Evolution's contact editor displays only the first ORG line and first TITLE/ROLE
/// lines, so secondary entries are invisible to the user. Verified across every
/// seeded leg to ensure secondary entries survive client modifications intact.
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

/// Which of the seeded card's notes the server files on it.
///
/// The distinction exists for one leg only, and it is a distinction the save
/// itself draws: a Notes field the user empties reaches the mapping as a `notes`
/// map with nothing visible left in it, and what the patch then says depends on
/// whether the card holds anything the user was *not* shown. With a note behind
/// the cleared one the entry alone is withdrawn; with nothing behind it the whole
/// property goes. Two branches, and the one below the fold has no other leg.
#[derive(Clone, Copy)]
enum SeededNotes {
    /// Both notes: the one Evolution's Notes field shows, and the one behind it.
    Both,
    /// The first alone — the card on which the field the user empties *is* the
    /// whole of `notes`.
    TheShownOneAlone,
}

/// Put the card both name legs start from into the mock's store, and hand back
/// the id the server filed it under.
///
/// Seeded straight into the store rather than written through EDS, because the
/// shape under test is one no vCard can state: a card created through EDS would
/// arrive with the given name as a single component, leaving nothing for the
/// save to put back — or, in the leg that retypes the name, to discard.
fn seed_double_barrelled_card(server: &jmap_mock::MockServer) -> Id {
    seed_card(server, SeededNotes::Both)
}

/// The same card, save that the server filed one note on it rather than two.
///
/// Everything else is left exactly as the other legs have it: the point of the
/// card is that the note the user can see is the only one there is, and a leg
/// about that must still be able to say the save left the picture, the calendars
/// and the relations alone.
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
    // What a server assigns; the mock's own `ContactCard/set` fills the same
    // shape in, and seeding bypasses it.
    card.uid = Some(format!("urn:example:card:{}", id.as_str()));
    card.name = Some(Name {
        full: Some(EDITED_FULL_NAME.to_owned()),
        components: Some(components),
        ..Name::default()
    });
    // A picture the user has, under a key only a server would choose. Seeded on
    // the card both name legs start from because it is a property of the same
    // kind as the pronunciations beside it: the user edits neither, so the save
    // must hand it back exactly as it arrived — and unlike a `phonetic`, this
    // one is a property the user *can* see, so EDS puts it on the far side of
    // the round trip whether the mapping wants it there or not.
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
    // The two calendaring resources, likewise under keys only a server would
    // choose, and likewise on the card every leg starts from: two entries of one
    // map that cross on lines of *different* names, which is the shape the
    // reader's per-line key counter has to keep free of itself. The `pref` is
    // there because no line can carry it — see [`SEEDED_CALENDAR_PREF`].
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
    // Who the card relates to: the marriage the spouse line shows, and a brother
    // no field can show. Seeded on the card every leg starts from because the
    // pair is what tells a save that withdrew a marriage from one entity apart
    // from one that emptied the map.
    card.related_to = Some(
        [
            (SEEDED_SPOUSE.to_owned(), relation_of("spouse")),
            (SEEDED_SIBLING.to_owned(), relation_of("sibling")),
        ]
        .into_iter()
        .collect(),
    );
    // The notes, likewise under keys only a server would choose, and likewise on
    // the card every seeded leg starts from: the only mapped property of which
    // the user can see one entry and not the other, since the field Evolution
    // shows is the first line of a name every entry writes a line of. The
    // `created` is there because no line can carry it — see
    // [`SEEDED_NOTE_CREATED`]. How many of them the card holds is the caller's
    // choice; see [`SeededNotes`].
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
    // Where the contact is reachable, stated as a URI and nothing else — see
    // [`SEEDED_SERVICE_KEY`]. One entry rather than two, because unlike the
    // notes and the relations beside it there is nothing here for a second
    // entry to say that the first does not: the invisible-to-the-user case
    // belongs to a service EDS has no field for, which is a fixture's question
    // and not one that needs the daemons.
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

/// A `relatedTo` value stating one relation type, as RFC 9553 §1.4.3's Set:
/// the type is a key and its value is `true`.
fn relation_of(kind: &str) -> Relation {
    Relation {
        relation: Some([(kind.to_owned(), serde_json::json!(true))].into()),
        ..Relation::default()
    }
}

/// Hold the card the server now holds to the brother it was seeded with — the
/// entry no line shows, under the key and of the type it arrived with.
///
/// Split out from the marriage beside it because the leg that retypes the spouse
/// leaves this one alone, and that is what it has to prove: the save withdraws a
/// marriage from the entity the field stopped naming, not every relation the
/// server holds. The vCard never showed this entry, so a save that dropped it
/// deleted something the user could not have seen, let alone edited.
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

/// Hold the card the server now holds to both relations it was seeded with.
///
/// Shared by the legs that edit something else entirely, for the reason
/// [`assert_the_seeded_calendars_survived`] is: a user who retypes their name
/// has not remarried, so a save that touched either entry — or re-keyed the
/// marriage, which is the only way this property *can* be touched — did
/// something nobody asked for.
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

/// Hold the card the server now holds to the free/busy address it was seeded
/// with — the same URI, of the same kind, under the same server-chosen key.
///
/// Split out from the calendar beside it because the leg that retypes the
/// calendar address leaves this one alone, and that is exactly what it has to
/// prove: the field the user did not touch is not the field the save patched.
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

/// Hold the card the server now holds to both calendaring resources it was
/// seeded with, untouched.
///
/// Shared by the legs that edit something else entirely, for the reason
/// [`assert_the_seeded_picture_survived`] is: a user who retypes their name has
/// not moved their calendar, so a save that re-filed either entry under a key of
/// its own making — or dropped the `pref` no line can carry — did something
/// nobody asked for.
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

/// Hold the card the server now holds to the second note it was seeded with —
/// the same text, under the same server-chosen key.
///
/// Split out from the first note because the leg that retypes the note leaves
/// this one alone, and that is what it has to prove. It is the `notes`
/// counterpart of the brother beside it, one step weaker: the entry does reach
/// a line, so it is not invisible to the *mapping* — but it is invisible to the
/// **user**, since Evolution's Notes field is the first `NOTE` line and stops
/// there. A save that took it with the entry the user did edit would delete a
/// note nobody could have seen, let alone asked to lose.
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

/// Hold the card the server now holds to both notes it was seeded with,
/// untouched.
///
/// Shared by the legs that edit something else entirely, for the reason
/// [`assert_the_seeded_calendars_survived`] is: a user who retypes their name
/// has not rewritten their notes, so a save that re-filed either entry under a
/// key of its own making — or dropped the `created` no line can carry — did
/// something nobody asked for.
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

/// Hold the card the server now holds to the picture it was seeded with — the
/// same bytes, of the same kind, under the same server-chosen key.
///
/// Shared by both name legs because the claim is the same in each and is about
/// neither name: a user who edits one field, or retypes another, has not touched
/// the picture, so a save that re-filed it under a key of its own making — or
/// re-encoded the bytes, or dropped it — did something nobody asked for.
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

/// Hold the card the server now holds to the instant-messaging handle it was
/// seeded with — the same URI, at the same service, under the same server-chosen
/// key, and still stating no `user`.
///
/// Shared by the legs that edit something else entirely, and unlike every other
/// survival assertion beside it, this entry *is* visible to the user: it reaches
/// a line, and the user could have retyped it. So what it holds the save to is
/// that an edit somewhere else left the entry where the server put it — same
/// key, same service, and still stating a `uri` rather than the `user` the line
/// hands back.
///
/// What it does **not** catch, measured rather than reasoned about: a save that
/// compares an entry's *members* rather than the handle they both spell. That
/// one writes the URI back with the same text it already had, so every assertion
/// here still holds and only the card's state on the server moves — a patch, and
/// a bump, each time the contact is touched for any reason. Catching it takes a
/// before-and-after on that state, which is
/// `an_edit_that_left_a_uri_only_handle_alone_writes_nothing` in
/// `jmap-book-sync`'s fixtures.
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

/// Hold the card the server now holds to all three anniversaries it was seeded
/// with: the wedding anniversary the X-EVOLUTION-ANNIVERSARY line shows, the
/// year-only birthday no line can show, and the deathday vCard 3.0 has no
/// property for.
///
/// Shared by the legs that edit something else entirely: a user who retypes
/// their name or note has not changed their anniversaries, so a save that
/// touched any of them — or dropped the unmodeled year-only birthday and
/// deathday — did something nobody asked for.
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

/// Hold the card the server now holds to all multiple organizations and titles/roles
/// it was seeded with.
///
/// Evolution's contact editor displays only the first ORG line and first TITLE/ROLE lines,
/// so secondary entries are invisible to the user. A save that touched unrelated fields
/// must leave both organizations and all four titles/roles intact on the server.
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

/// Hold the card the server now holds to all multiple addresses and labels it was seeded with.
///
/// A save that touched unrelated fields must leave both addresses (work and home), their
/// structured components, and their full labels intact on the server.
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

/// The port the mock is listening on, for the keyfile the session is written
/// with.
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

    // The contact is named to the client by its JMAP id, which is what the
    // backend hands EDS as the vCard `UID`.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // What EDS made of the `N` line the backend wrote: the two components
    // joined into the single field their kind owns.
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
    // And what EDS made of the `PHOTO` line the backend wrote, which is the
    // direction the write leg cannot check: there the line came from EDS in the
    // first place, while this one the emitter wrote — `X-JMAP-KEY` in front of
    // the parameters EDS puts there itself, and the bytes of a `data:` URI
    // decoded back out. A picture the user is not shown is a picture Evolution
    // would offer to replace with nothing.
    //
    // A `file:` URI again, for the reason the write leg spells out, and the
    // `.png` is what says the media type the emitter stated survived: EDS names
    // the file it caches a picture in after what it decided the picture is.
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

    // And after the save: the edit took, and EDS's own view of the name is
    // still the text it started from rather than something the save rewrote.
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

    // The other end: the card the server now holds. The name components are
    // the whole point — both halves, in the order they went out in, each still
    // carrying the pronunciation the `N` line had no room for.
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
    // And the edit itself, at this end: patched in place, not re-added.
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

/// What the user retypes the given-name field to, and the full name Evolution's
/// contact editor keeps in step with it.
///
/// One word where the field held two, and one that is not a substring of either
/// half: nothing about `Hans` says which part of `Jean Paul` it replaced, which
/// is exactly why both parts have to go.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // The card EDS started from is the seeded one, with the two halves joined
    // into the single field their kind owns — the same starting point the
    // preceding leg checks, and worth checking again here because the branch
    // under test is chosen by comparing against exactly this text.
    assert_eq!(
        seen.get("read-given-name"),
        Some(&EDITED_GIVEN_FIELD),
        "EDS did not hand back the given name the emitter wrote\n{report}"
    );

    // And what EDS holds after the save: the name the user typed, on both the
    // field they typed it in and the FN line beside it.
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

    // The other end, and the load-bearing assertion: the card the server now
    // holds states the given name as the *one* component the user typed. The
    // two the card was seeded with are gone, and so are the pronunciations that
    // belonged to them — a `phonetic` for `Jean` is not one for `Hans`, and
    // keeping either would leave half an old first name beside the new one.
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
    // And what nobody touched: the email address is still the seeded one,
    // patched around rather than through.
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

/// The fourth leg, and the one edit that reaches the picture itself: the user
/// picks a new photo for the card the server filed under a key of its own.
///
/// The claim is where the new picture lands, not that it arrives. EDS rewrites
/// the `PHOTO` line out of the photo field whenever that field is *set*, and
/// drops the parameters it had — the `X-JMAP-KEY` among them — so the new
/// picture reaches the backend with nothing on it that says which entry it
/// replaces. If the save took that at face value the server would end up holding
/// two pictures: the old one under its own key and the new one under a name the
/// reader invented by counting lines. What has to happen instead is that the
/// keyless picture is paired with the one it replaced and patched over it.
///
/// Why this needs real daemons: an untouched picture keeps its key, because the
/// line EDS writes back into a cached card is not a `set` — measured here, by
/// [`an_edit_through_eds_keeps_the_name_parts_the_vcard_flattened`] passing with
/// the pairing removed. A *set* one does not. Which of the two a save is handed
/// is EDS's decision, and this is the leg that makes it.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // The picture the card started from, so that the leg says what was replaced
    // rather than only what replaced it.
    assert_eq!(
        seen.get("read-photo-file-base64"),
        Some(&PHOTO_BASE64),
        "the card EDS handed back is not the seeded one\n{report}"
    );
    // And what EDS holds afterwards: the new picture, back out of the cache
    // file the backend's re-rendered card was stored into. This is the whole
    // round trip in one observation — chosen in a libebook consumer, sent to the
    // server, read back off what the server now holds.
    assert_eq!(
        seen.get("read-back-photo-file-base64"),
        Some(&REPLACEMENT_PHOTO_BASE64),
        "the picture the user chose is not the one EDS ended up with\n{report}"
    );
    // The name is not part of this edit, and is asserted for the same reason the
    // `edit` leg asserts it: a save that rewrote it did something nobody asked.
    assert_eq!(
        seen.get("read-back-given-name"),
        Some(&EDITED_GIVEN_FIELD),
        "the save changed the given name nobody edited\n{report}"
    );

    // The other end, and the load-bearing assertion: one picture, under the key
    // the *server* chose, holding the bytes the user picked. A second entry here
    // — or this one still holding the old picture beside a new `m1` — is the
    // pairing having fallen through.
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
    // And what nobody touched, at this end too: the name components the `N`
    // line flattened, and the email address.
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

/// What the user retypes the Calendar field to. A URI on a different host from
/// the free/busy address beside it, so a save that patched the wrong entry could
/// not pass by the two happening to agree.
const RETYPED_CALENDAR_URI: &str = "https://calendars.example/jp/personal.ics";

/// The fifth leg: the user moves their calendar, on a card carrying both
/// calendaring resources the server filed under keys of its own.
///
/// The claim is that the save patches the entry the user edited and leaves the
/// one beside it alone — which needs real daemons for a reason the mapping's own
/// tests cannot reach. `E_CONTACT_CALENDAR_URI` and `E_CONTACT_FREEBUSY_URL` are
/// plain vCard attributes rather than synthetic fields, so a set on one is
/// measured to rewrite the *value* of the first line of that name in place and
/// leave its parameters — the `X-JMAP-KEY` among them — where they were. That
/// measurement was a throwaway C probe against libebook-contacts; here it is the
/// path Evolution takes, through the daemons, and the key surviving is what says
/// the save may patch by key rather than having to pair a keyless URI with the
/// one it replaced the way a picture does.
///
/// Two entries rather than one because they cross on lines of *different* names:
/// the free/busy address is what says a patch aimed at `calendars/calendar-1`
/// did not land on the whole map, and its untouched URI is what says EDS wrote
/// the second line back out of the cached card rather than out of a field
/// nobody set.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // What EDS made of the two lines the emitter wrote — the direction the write
    // leg cannot ask about, where the lines came from EDS in the first place.
    // Each URI in the field its kind chose, which is the reader's half of the
    // mapping checked against real EDS rather than against a probe.
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

    // And what EDS holds after the save: the address the user typed, and the one
    // they did not.
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

    // The other end, and the load-bearing assertion: two entries still, under
    // the two keys the *server* chose, and the one the user edited holding the
    // new URI with the `pref` no line could carry still on it. A third entry
    // here — the new URI filed under a `c1` the reader invented by counting
    // lines — is the key having failed to survive EDS.
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
    // And what nobody touched at all: the picture and the name components the
    // `N` line flattened, asserted for the reason the other legs assert them.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// What the user retypes the Spouse field to. A different person from the one
/// the server holds — not a respelling of that name — because that is the only
/// reading the field supports: the name *is* the key, so nothing distinguishes
/// "this is somebody else" from "this is the same person, spelled right", and
/// the save takes both as the marriage moving.
const RETYPED_SPOUSE: &str = "Marianne Oldenburg";

/// The sixth leg: the user retypes who they are married to, on a card the server
/// relates to two people.
///
/// The one mapped property whose *key* is what the line shows, so this is where
/// real EDS is asked the question no test below the daemons can: whether
/// `e_contact_set` on `E_CONTACT_SPOUSE` rewrites the one
/// `X-EVOLUTION-SPOUSE` line in place or leaves the old one standing beside the
/// new. A second line would reach the server as a second marriage — the card
/// would say the user is married to both — and the mapping could not tell that
/// from a card that really did state two, because it has nothing but the lines
/// to go on.
///
/// Two entries rather than one because withdrawing a marriage is not emptying
/// the map: the brother is of a type no field shows, so a save that took the
/// whole property back — or replaced it wholesale with what the vCard could
/// state — would delete a relation the user never saw and could not have
/// touched.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // What EDS made of the line the emitter wrote — the direction the write leg
    // cannot ask about, where the line came from EDS in the first place. The
    // name is the entry's *key* over on the server, so this is where the
    // crossing from key to value is checked against real EDS: a spouse missing
    // here is one the user cannot see, whatever the card holds.
    assert_eq!(
        seen.get("read-spouse"),
        Some(&SEEDED_SPOUSE),
        "EDS did not read the spouse off the line the emitter wrote\n{report}"
    );

    // And what EDS holds after the save: the name the user typed, once. Two
    // names joined here would be EDS holding two spouse lines, which is the
    // failure this leg exists for.
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

    // The other end, and the load-bearing assertion: two entries still, the
    // brother's untouched and the marriage now keyed by the name the user typed.
    // Three would be the old marriage still standing — the user married to
    // somebody they stopped naming — and two of which one is the old spouse
    // would be the save having patched the wrong entry.
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
    // And what nobody touched at all, asserted for the reason the other legs
    // assert them.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The seventh leg: the user *clears* the Spouse field, on the same card the
/// server relates to two people.
///
/// The other half of the marriage, and the one branch of the save that no leg
/// above reaches: an edited card stating no relations at all. Every other leg
/// hands the save a `relatedTo` holding something, so the path where the whole
/// property has gone from the read-back vCard — a withdrawal with nothing to put
/// in its place — has only ever been driven against fixtures.
///
/// Which is also where the fixtures rest on an *inference* the daemons are
/// needed to settle. `as_evolution_retypes_the_spouse` in `jmap-book-sync`'s
/// tests assumes clearing the field leaves an empty `X-EVOLUTION-SPOUSE` line
/// rather than dropping the attribute, by analogy with another field; the save
/// is written to withdraw the marriage either way — the reader refuses a line
/// naming nobody — but which one real EDS does was never measured. So the client
/// reports what it found on the card it is about to hand over, and what it
/// reported against libebook-contacts 3.52 is the empty line the fixture assumed:
/// `cleared-spouse-line=present`, `cleared-spouse-line-value=` (empty).
///
/// That observation is printed rather than asserted, and the assertion below is
/// the version-robust half of it: whatever EDS did to the line, the *field*
/// Evolution shows must read empty. Pinning the attribute's presence instead
/// would make a legitimate change in a later libebook-contacts look like a bug in
/// this repository, when both shapes reach the same withdrawal — which is not a
/// guess either: the same EDS drops the attribute outright when the field is set
/// to `NULL` rather than to the empty string, and this leg passes unchanged
/// against that card too.
///
/// The brother is again what makes the leg say something: emptying the field
/// withdraws one marriage, and a save that answered it by taking the property
/// back — the shape `relatedTo: null` — would delete a relation of a type no
/// field shows, which the user never saw and cannot have cleared.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // The marriage the card started from, so the leg says what was cleared
    // rather than only that nothing is there afterwards — a spouse EDS never
    // read would make every assertion below pass for the wrong reason.
    assert_eq!(
        seen.get("read-spouse"),
        Some(&SEEDED_SPOUSE),
        "EDS did not read the spouse off the line the emitter wrote\n{report}"
    );

    // What the save is handed: a card whose Spouse field is empty. This is the
    // measurement the fixtures could not make — see the comment above — and the
    // failure it rules out is EDS holding the old name on a line the field no
    // longer shows, which would reach the server as a marriage nobody withdrew.
    assert_eq!(
        seen.get("cleared-spouse"),
        Some(&""),
        "EDS kept a spouse on the card after the field was cleared\n{report}"
    );

    // And what EDS holds after the save, back out of the cache file the
    // backend's re-rendered card was stored into: still nobody. A name here is
    // the server having been told to keep the marriage.
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

    // The other end, and the load-bearing assertion: the brother alone, of the
    // type he arrived with. No entry keyed by the old spouse — that would be the
    // marriage never withdrawn — and a `relatedTo` that is present at all, since
    // the property going with the marriage is the brother going too.
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
    // And what nobody touched at all, asserted for the reason the other legs
    // assert them.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// What the user retypes the Notes field to. Deliberately not a respelling of
/// either seeded note: a save that patched the wrong entry, or that replaced the
/// map with what the vCard could state, could not pass by two texts happening to
/// agree.
const RETYPED_NOTE: &str = "met in Ghent, and paid up at last";

/// The eighth leg: the user retypes the note on a card the server filed **two**
/// notes on.
///
/// The property where the user sees part of a map and edits it anyway.
/// `E_CONTACT_NOTE` is Evolution's Notes field, and it is the *first* `NOTE`
/// line — every `notes` entry writes a line, so a card with two notes shows the
/// user one and hides the other behind it. Retyping the field is therefore an
/// edit to one entry of a map, made through a field that cannot express the map,
/// and what happens to the entry behind it is the question.
///
/// Two things only real EDS can answer, and the leg exists for both:
///
/// - whether `e_contact_set` on a plain vCard attribute rewrites the first line
///   of that name **in place**, keeping its parameters — the `X-JMAP-KEY` among
///   them — or drops the key the way it drops a `PHOTO`'s. Keyless, the retyped
///   text would reach the server under an `n1` the reader invented by counting
///   lines, and the note it replaced would be deleted;
/// - whether the *second* line survives the set at all. If it did not, a user
///   editing their note in Evolution would silently delete a note they were
///   never shown — and nothing below the daemons could see it, because the card
///   the save is handed is EDS's rendering rather than this repository's.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // What EDS made of the two lines the emitter wrote. The field shows the
    // first note, which is the reader's half of the mapping checked against real
    // EDS — and the line count beside it is the half the field cannot show,
    // since a card holding one note and a card holding five read alike here.
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

    // And what the set left behind, which is the observation this leg was
    // written for: one line rewritten, the other still standing. A `1` here is
    // `e_contact_set` having replaced every line of the name — the user's edit
    // deleting a note they were never shown — and a `3` is it having appended
    // beside the old one, which reaches the server as a note nobody typed.
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

    // The other end, and the load-bearing assertion: two entries still, under
    // the two keys the *server* chose, and the one the user edited holding the
    // new text with the `created` no line could carry still on it. A third entry
    // here — the new text filed under an `n1` the reader invented by counting
    // lines — is the key having failed to survive EDS.
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
    // And what nobody touched at all, asserted for the reason the other legs
    // assert them.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// What the client joins the card's `NOTE` line values with when it reports
/// them. A character neither seeded note holds, and not one vCard gives
/// structural meaning to — the semicolon and the comma are the very things the
/// notes carry to be checked, so neither of them can also be the delimiter.
const NOTE_LINE_SEPARATOR: &str = "|";

/// The ninth leg: the user **empties** the Notes field on a card the server
/// filed two notes on.
///
/// The other half of [`retyping_the_note_through_eds_patches_the_entry_it_replaces`],
/// and the branch of the save no other leg reaches: an entry withdrawn from a
/// map through a field that cannot express the map. The note behind it is what
/// says the withdrawal was of the entry the user could see rather than of the
/// property — it reaches a line of its own, so a save that answered an emptied
/// field by taking the whole of `notes` back would delete a note nobody was ever
/// shown, let alone asked to lose.
///
/// The empty string rather than NULL, because that is what Evolution's contact
/// editor writes: it hands `e_contact_set` the text of the field, and the text of
/// a field the user emptied is `""`. What EDS then leaves on the card is the
/// measurement this leg exists for, and it is reported rather than judged — the
/// spouse leg found libebook-contacts 3.52 leaves the *attribute* standing with
/// no value on it, and whether a `NOTE` behaves the same way is that leg's answer
/// about another property, not this one's. Either shape withdraws the note, since
/// a line saying nothing is refused on the way in; what is asserted is therefore
/// what is true of both — the field reads empty, and the only `NOTE` value left
/// on the card is the note the user never saw.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // The card the user started from, so the leg says what was cleared rather
    // than only that nothing is there afterwards: the note the field showed, and
    // the two lines behind it that the field cannot count.
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

    // What the save is handed. The field first, which is the version-robust
    // half: a card still showing the old text here is the user's clearing never
    // having happened at all.
    assert_eq!(
        seen.get("cleared-note"),
        Some(&""),
        "EDS kept a note in the field after it was cleared\n{report}"
    );

    // Then every `NOTE` value left on that card, which is what the field cannot
    // say — it reads the first line whatever else is there, so it shows the
    // note behind the cleared one and the card underneath alike. Only the
    // values that say something are compared, because whether the emptied line
    // is struck off or left standing with nothing on it is libebook-contacts'
    // business and the mapping withdraws the note either way. Measured against
    // libebook-contacts 3.52 it is the latter: `cleared-note-lines` reads 2, so
    // the card handed to the save states a note that says nothing — which is
    // the same shape the spouse leg found, and the shape
    // `emptying_one_note_line_of_two_withdraws_that_note_alone` states as its
    // fixture.
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

    // And what EDS holds after the save, out of the cache the backend's
    // re-rendered card was stored into: one line, and it is the note the user
    // was never shown. Clearing the field therefore *reveals* the note behind
    // it rather than emptying the field — surprising, and correct: the field is
    // the first `NOTE` line, and after the withdrawal that is the second note.
    // Asserted because the alternative readings are both failures — the old
    // text here is the withdrawal never reaching the server, and an empty field
    // is the second note gone with the first.
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

    // The other end, and the load-bearing assertion: the second note alone,
    // under the key the *server* chose. A `notes` that is absent entirely is the
    // save having answered an emptied field by taking the property back, and an
    // entry still keyed by the first note is the withdrawal never made.
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
    // And what nobody touched at all, asserted for the reason the other legs
    // assert them.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The tenth leg: the user empties the Notes field on a card the server filed
/// **one** note on.
///
/// The third and last shape a cleared Notes field can have, and the one no other
/// leg reaches. The ninth leg clears a note with another behind it, so the save
/// withdraws the entry the user could see and writes `notes/note-1: null`; here
/// there is nothing behind it, so what the save must say is that the property
/// itself is gone — `notes: null`, one patch member rather than one per surviving
/// key. The mapping draws that line explicitly (it counts what the card hides
/// before deciding), and until now the branch below the fold was exercised
/// against fixtures alone.
///
/// Two things only real EDS can answer, and this leg is the only place either is
/// asked of a card holding a single note:
///
/// - whether emptying the field leaves the card as a `NOTE` line saying nothing
///   or with no `NOTE` line at all. Both withdraw the note, since a line saying
///   nothing is refused on the way in, so the count is reported and the *values*
///   are what is judged — but which shape EDS produces decides which side of the
///   mapping does the withdrawing, and it was measured on a two-note card only;
/// - whether the field is then empty for the user. On the ninth leg's card the
///   note behind the cleared one takes its place, which reads as a field that
///   would not clear; here nothing can, so an empty field afterwards is the
///   observation that says the ninth leg's surprise was the second note and not
///   a save that refused the withdrawal.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // The card the user started from, so the leg says what was cleared rather
    // than only that nothing is there afterwards — and the count beside it,
    // which is what makes this card the one under test: a `2` here is the
    // seeding having put the ninth leg's card in front of the daemons instead,
    // and every assertion below would then be about the wrong branch.
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

    // What the save is handed. The field first, which is the version-robust
    // half: a card still showing the old text here is the user's clearing never
    // having happened at all.
    assert_eq!(
        seen.get("cleared-note"),
        Some(&""),
        "EDS kept a note in the field after it was cleared\n{report}"
    );

    // Then every `NOTE` value left on the card, and this time there is no note
    // behind the cleared one for the emptied line to be confused with: nothing
    // the card says is a note any more. Whether that is a line standing with no
    // value on it or no line at all is libebook-contacts' business — the
    // mapping withdraws the note either way — so the values are what is judged
    // and the count is reported. Measured against libebook-contacts 3.52 the
    // line stands: `cleared-note-lines` reads 1, which is the shape
    // `emptying_the_only_note_line_withdraws_the_property` states as its
    // fixture.
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

    // And what EDS holds after the save, out of the cache the backend's
    // re-rendered card was stored into: no note, and no line to hold one.
    // Unlike the ninth leg — where the note behind the cleared one surfaced in
    // the field — the field really is empty here, because there is nothing left
    // to reveal.
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

    // The other end, and the load-bearing assertion: `notes` gone from the card
    // altogether. An empty map would be the property still there saying nothing,
    // and an entry still keyed by the note is the withdrawal never made.
    assert_eq!(
        card.notes, None,
        "clearing the only note did not withdraw the property: {card:?}"
    );
    // And what nobody touched at all, asserted for the reason the other legs
    // assert them — here doubly, since the patch this leg is about names a whole
    // property rather than one entry of one.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_service_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The eleventh leg: the user retypes an instant-messaging handle the server
/// stated as a **URI**, and the save has to write it back in the shape it came
/// in.
///
/// The one mapped property whose value the server can state in a form the card
/// cannot carry. RFC 9553 §2.3.2 lets an `onlineServices` entry name the contact
/// with a `user`, a `uri`, or both; Evolution's instant-messaging fields hold a
/// handle and nothing else. So the entry reaches the user only because `xmpp:`
/// spells the JID and nothing after it, which lets the reader draw the handle out
/// of the URI — and the save writes the edit back onto the member it was drawn
/// from rather than answering a card shaped one way with a card shaped another.
/// Until now that whole crossing was driven against fixtures, where the vCard is
/// one this repository wrote.
///
/// Three things only real EDS can answer:
///
/// - whether the drawn handle reaches the field at all. The reader picks the
///   slot — `TYPE=HOME` — and `E_CONTACT_IM_JABBER_HOME_1` is the field that slot
///   feeds, so whether the parameter our emitter writes is the one libebook-contacts
///   reads a handle back out of is a claim about EDS. A handle in the wrong slot,
///   or on a line with no `TYPE` at all, is one the user never sees;
/// - what a set on that field leaves on the card: one `X-JABBER` line or two, and
///   with the `X-JMAP-KEY` still on it or without. A second line would tell the
///   mapping — which has only the lines — that the contact is at that service
///   twice, and a lost key would mean the save could not patch the entry the URI
///   came from and had to pair a keyless line with it instead. Measured against
///   libebook-contacts 3.52 the set rewrites the line in place and keeps its
///   parameters: `retyped-im-handle-lines=1` and `retyped-im-handle-key=handle-1`.
///   Both are reported rather than asserted, for the reason the spouse line is —
///   the save reaches the same entry either way, and the assertion below is on
///   what the *server* ends up holding;
/// - and, from the nine legs beside this one, that a save which touches something
///   else entirely leaves the entry in the shape the server chose — the line
///   hands back a `user` where the server holds a `uri`, so an entry rewritten
///   into the shape the line states is a card answered with a card it never was.
///   That is [`assert_the_seeded_service_survived`], which also says what it
///   cannot see.
///
/// Two mutations redden this leg and nothing else in the file: making the save
/// write the retyped handle onto a `user` instead of rebuilding the URI, and
/// dropping `xmpp` from the reader's table of schemes, which leaves the handle
/// undrawn and the field empty. Both are caught by `jmap-book-sync`'s fixtures
/// too, so what this leg adds is the *input* — the fixtures state the card a
/// URI-only entry produces, and only the daemons can say EDS produces it.
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // The first of the three: the handle the server never stated as a handle,
    // read out of the field Evolution's contact editor shows. This is the whole
    // of the URI half of the mapping asked of real EDS at once — the scheme was
    // recognised, the handle was drawn out of it, the line was written with the
    // `TYPE` this field is read off, and EDS filed it there.
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

    // And what EDS holds after the save, out of the cache the backend's
    // re-rendered card was stored into: the handle the user typed, once. Two
    // joined here would be the card stating the contact is at the service twice.
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

    // The other end, and the load-bearing assertion: the entry the server filed
    // is still the entry the server filed — same key, same service — and the
    // handle the user typed went back onto the member it was drawn from. A
    // `user` here instead would be the save telling the server the contact is
    // named a way this card never said it was; a second entry would be the
    // retype read as a handle at a service the contact had not been at.
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

    // And what nobody touched at all, asserted for the reason the other legs
    // assert them.
    assert_the_seeded_picture_survived(card);
    assert_the_seeded_calendars_survived(card);
    assert_the_seeded_relations_survived(card);
    assert_the_seeded_notes_survived(card);
    assert_the_seeded_anniversaries_survived(card);
    assert_the_seeded_organizations_and_titles_survived(card);
    assert_the_seeded_addresses_survived(card);
}

/// The name and address of the card the `remove` leg deletes. Plain, since
/// nothing about this leg is a mapping question — it is whether
/// `remove_contact_sync` really reaches the server, and neither end reads
/// this text back for anything beyond confirming the right card was seeded.
const REMOVABLE_FULL_NAME: &str = "Alex Krycek";
const REMOVABLE_EMAIL: &str = "alex@example.com";

/// Put a single, minimal card into the mock's store, the way `seed_card`
/// does for the name-editing legs — straight into the store rather than
/// through EDS, since the point of this leg is what EDS does with a card it
/// did not just write.
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

/// `remove_contact_sync` — the address-book mirror of the recurring-occurrence
/// delete `calendar.rs`/`cal-client.c` already drive on the calendar side, and
/// the one vfunc none of the other ten legs in this file reach: they all save
/// an edit, never destroy the card outright.
///
/// Checked from both ends, as every other leg is: EDS's own cache has to
/// agree the card is gone (a backend that answered success without truly
/// forwarding the destroy would still hand back the card on the next get),
/// and the server the backend talks to has to have lost it too (a backend
/// that only dropped its local cache entry would leave a ghost the next
/// `ContactCard/get`-driven refresh would resurrect).
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

    // The connect, checked before anything else for the reason the first leg
    // spells out: a read-only or unconnected book turns every later failure
    // into a message about the wrong thing.
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

    // The other end: the server the backend actually talks to. The write path
    // is deliberately not asserted against `method_calls()` here beyond this —
    // a destroy is a `ContactCard/set` call like every other write this file
    // checks — because the load-bearing claim is the *outcome*, not the call
    // shape: the card is gone from the store, not merely that some request
    // was sent.
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
