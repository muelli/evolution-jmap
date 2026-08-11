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
//! Two legs, because they need two books. The first starts empty and writes a
//! contact into it. The second starts from a card the mock was seeded with
//! before EDS ever connected — a card from the *server*, holding a shape no
//! vCard can state, which is the only way to ask what real EDS does to it.

use jmap_functional::{Session, observations, required_path};
use jmap_proto::contacts::{ContactCard, Name, NameComponent};

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

    let output = session.run(&client, &["jmap-functional", "write", FULL_NAME]);
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

#[test]
fn an_edit_through_eds_keeps_the_name_parts_the_vcard_flattened() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // Seeded straight into the mock's store rather than written through EDS,
    // because the shape under test is one no vCard can state: a card created
    // through EDS would arrive with the given name as a single component,
    // leaving nothing for the save to put back.
    let card_id = {
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
        // What a server assigns; the mock's own `ContactCard/set` fills the
        // same shape in, and seeding bypasses it.
        card.uid = Some(format!("urn:example:card:{}", id.as_str()));
        card.name = Some(Name {
            full: Some(EDITED_FULL_NAME.to_owned()),
            components: Some(components),
            ..Name::default()
        });
        account.contact_cards.seed_with_id(id.clone(), card);
        id
    };

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

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
}
