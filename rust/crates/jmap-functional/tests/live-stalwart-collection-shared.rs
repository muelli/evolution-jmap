// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendars and address books another account shares with the user, through
//! a real `evolution-source-registry` against a real Stalwart.
//!
//! `jmap-collection-sync/tests/live_server_shared.rs` proves that
//! `Fanout::discover` finds a share and lets it go once the grant is
//! withdrawn. This leg asks what a user sees: that the collection backend's
//! populate turns those answers into child `ESource`s of the account (named
//! with the owner, carrying the `<accountId>/<collectionId>` identity the book
//! and calendar factories read), and that a later populate removes them from
//! the registry once the share is gone.
//!
//! Two runs of `functional-collection-shared-client`
//! (`tests/functional/collection-shared-client.c`) in one scratch tree: the
//! first, with the grants in place, waits for both shared children; the
//! second, after the owner withdrew them, starts from the `.source` files the
//! first run left on disk and waits for the populate to remove them.
//!
//! ## Running it
//!
//! The two-account recipe of the `jmap-*-sync` crates' shared-collection
//! tests (`JMAP_LIVE_SERVER_WRITE_*` is the owner, `JMAP_LIVE_SERVER_RECIPIENT_*`
//! the user whose Evolution account this is), built through
//! `-DENABLE_FUNCTIONAL_TESTS=ON`, with the EDS runtime packages installed:
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-collection-shared -- --ignored
//! ```
//!
//! Skipped, not failed, when either pair is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::calendars::Calendar;
use jmap_proto::contacts::AddressBook;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_PRINCIPALS};
use serde_json::json;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)` for the account named by `prefix`, or `None`
/// to skip.
fn live_server_params(prefix: &str) -> Option<(String, String, String)> {
    let user = env::var(format!("{prefix}_USER")).ok()?;
    let password = env::var(format!("{prefix}_PASSWORD"))
        .unwrap_or_else(|_| panic!("{prefix}_USER is set but {prefix}_PASSWORD is not"));
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .unwrap_or_else(|_| panic!("set JMAP_LIVE_SERVER_URL alongside {prefix}_USER"));
    Some((origin, user, password))
}

fn connect(origin: &str, user: &str, password: &str) -> Client {
    Client::builder()
        .rebase_urls_to_origin(true)
        .connect(origin, Credentials::basic(user, password))
        .unwrap_or_else(|error| panic!("could not fetch the session document for {user}: {error}"))
}

/// `live-stalwart-collection.rs`'s keyfile: the recipient's account, contacts
/// and calendars on, mail off.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart shared collection test\n\
         Enabled=true\n\
         \n\
         [Collection]\n\
         BackendName=jmap\n\
         ContactsEnabled=true\n\
         CalendarEnabled=true\n\
         MailEnabled=false\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         User={user}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

/// The owner shares an address book (writable) and a calendar (read-only)
/// with the recipient. The recipient's Evolution account gains one child for
/// each, named `<collection> (<owner account name>)` and not offered for
/// deletion (neither grant has `mayDelete`). The owner withdraws both grants,
/// and the next populate removes both children. Destroys both collections at
/// the end.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn shared_collections_appear_in_the_source_registry_and_go_when_unshared() {
    let Some((origin, owner_user, owner_password)) = live_server_params("JMAP_LIVE_SERVER_WRITE")
    else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping");
        return;
    };
    let Some((_, recipient_user, recipient_password)) =
        live_server_params("JMAP_LIVE_SERVER_RECIPIENT")
    else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping");
        return;
    };
    let client = required_path("JMAP_FUNCTIONAL_COLLECTION_SHARED_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    let owner = connect(&origin, &owner_user, &owner_password);
    if !owner
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!("server does not advertise {CAPABILITY_PRINCIPALS}; skipping");
        return;
    }
    let contacts_account = owner
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the owner account needs the contacts capability");
    let calendars_account = owner
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the owner account needs the calendars capability");
    let recipient_principal = owner
        .principal_query(
            &contacts_account,
            PrincipalQueryFilter::email(&recipient_user),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner cannot resolve the recipient's principal id by email");

    let suffix = unique_suffix();
    let book_name = format!("agent-functional-shared-book-{suffix}");
    let book_id = owner
        .address_book_create(&contacts_account, &AddressBook::new(book_name.clone()))
        .expect("AddressBook/set create failed")
        .id
        .expect("the server named the new book");
    let calendar_name = format!("agent-functional-shared-cal-{suffix}");
    let calendar_id = owner
        .calendar_create(&calendars_account, &Calendar::new(calendar_name.clone()))
        .expect("Calendar/set create failed")
        .id
        .expect("the server named the new calendar");

    owner
        .address_book_update(
            &contacts_account,
            &book_id,
            json!({"shareWith": {recipient_principal.as_str(): {
                "mayRead": true, "mayWrite": true
            }}}),
        )
        .expect("AddressBook/set shareWith failed");
    owner
        .calendar_update(
            &calendars_account,
            &calendar_id,
            json!({"shareWith": {recipient_principal.as_str(): {
                "mayReadItems": true, "mayReadFreeBusy": true
            }}}),
        )
        .expect("Calendar/set shareWith failed");

    // The name the recipient's session gives the owner's account, which is
    // what the child's display name carries.
    let recipient = connect(&origin, &recipient_user, &recipient_password);
    let owner_name = recipient
        .session()
        .accounts
        .get(&calendars_account)
        .expect("the grant puts the owner's account in the recipient's session")
        .name
        .clone();

    // The kind is part of each name: JMAP ids are unique per type only, and
    // Stalwart does give a fresh book and a fresh calendar the same id.
    let book_child = format!("address-book:{contacts_account}/{book_id}");
    let calendar_child = format!("calendar:{calendars_account}/{calendar_id}");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    const ACCOUNT_UID: &str = "jmap-functional-live-collection-shared";
    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-collection-shared"
    ));
    session.write_source(ACCOUNT_UID, &keyfile(port, &recipient_user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &recipient_password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_collection_backend(&module);

    let run = |mode: &str| {
        let output = session.run(&client, &[ACCOUNT_UID, mode, &book_child, &calendar_child]);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
        assert!(
            output.status.success(),
            "the {mode} run failed with {}\n{report}",
            output.status
        );
        (stdout, report)
    };

    let (stdout, report) = run("present");
    let seen = observations(&stdout);
    assert_eq!(seen.get("account-found"), Some(&"1"), "{report}");
    assert_eq!(
        seen.get("present"),
        Some(&"2"),
        "the populate did not add both shared children\n{report}"
    );
    for (index, kind, name) in [
        (0, "address-book", &book_name),
        (1, "calendar", &calendar_name),
    ] {
        let key = |field: &str| format!("shared-{index}-{field}");
        assert_eq!(seen.get(key("kind").as_str()), Some(&kind), "{report}");
        assert_eq!(
            seen.get(key("display-name").as_str()).copied(),
            Some(format!("{name} ({owner_name})").as_str()),
            "the {kind} child does not name its owner\n{report}"
        );
        assert_eq!(
            seen.get(key("backend-name").as_str()),
            Some(&"jmap"),
            "{report}"
        );
        assert_eq!(seen.get(key("enabled").as_str()), Some(&"1"), "{report}");
        assert_eq!(
            seen.get(key("remote-deletable").as_str()),
            Some(&"0"),
            "the {kind} was shared without mayDelete\n{report}"
        );
    }

    owner
        .address_book_update(&contacts_account, &book_id, json!({"shareWith": null}))
        .expect("AddressBook/set withdrawing the grant failed");
    owner
        .calendar_update(&calendars_account, &calendar_id, json!({"shareWith": null}))
        .expect("Calendar/set withdrawing the grant failed");

    let (stdout, report) = run("absent");
    let seen = observations(&stdout);
    assert_eq!(
        seen.get("initially-present"),
        Some(&"2"),
        "the second run did not start from the first run's children\n{report}"
    );
    assert_eq!(
        seen.get("present"),
        Some(&"0"),
        "the populate did not remove the withdrawn shares\n{report}"
    );

    owner
        .address_book_destroy(&contacts_account, &book_id)
        .expect("AddressBook/set destroy failed (cleanup)");
    owner
        .calendar_destroy(&calendars_account, &calendar_id)
        .expect("Calendar/set destroy failed (cleanup)");
}
