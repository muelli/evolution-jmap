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

use jmap_functional::{Session, observations, required_path};

/// The contact the client writes. One string, passed to the client on its
/// command line and looked for in the mock's store, so the two ends cannot
/// disagree about it by a typo.
const FULL_NAME: &str = "Dana Scully";
const EMAIL: &str = "dana@example.com";
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

    let output = session.run(&client, &["jmap-functional", FULL_NAME]);
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
        seen.get("read-back-birthday"),
        Some(&BIRTHDAY),
        "the contact EDS handed back lost or moved its birthday\n{report}"
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
