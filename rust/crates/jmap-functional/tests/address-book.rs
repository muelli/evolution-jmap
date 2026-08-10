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
}
