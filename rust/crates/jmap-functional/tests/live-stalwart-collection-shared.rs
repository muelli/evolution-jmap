// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendars and address books that another account shares with the user,
//! through a real `evolution-source-registry` against a real Stalwart.
//!
//! `jmap-collection-sync/tests/live_server_shared.rs` tests that
//! `Fanout::discover` finds a share, and stops when the grant is removed.
//! This test checks what the user sees. The populate of the collection
//! backend makes child `ESource`s of the account from those results. Each
//! child has the owner in its name and the `<accountId>/<collectionId>`
//! identity that the book and calendar factories read. When the share is
//! removed, a later populate removes the children from the registry.
//!
//! The test runs `functional-collection-shared-client`
//! (`tests/functional/collection-shared-client.c`) two times in one scratch
//! tree. The first run, with the grants, waits for the two shared children.
//! The owner then removes the grants. The second run starts from the
//! `.source` files of the first run and waits until the populate removes
//! them.
//!
//! ## How to run it
//!
//! Use the two-account environment of the shared-collection tests of the
//! `jmap-*-sync` crates. `JMAP_LIVE_SERVER_WRITE_*` is the owner, and
//! `JMAP_LIVE_SERVER_RECIPIENT_*` is the user of the Evolution account. Build
//! with `-DENABLE_FUNCTIONAL_TESTS=ON` and install the EDS runtime packages:
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-collection-shared -- --ignored
//! ```
//!
//! When one of the two pairs is not set, the test skips and does not fail.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{
    Keyfile, Session, SourceKind, observations, required_path, spawn_loopback_proxy,
};
use jmap_proto::calendars::Calendar;
use jmap_proto::contacts::AddressBook;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_PRINCIPALS};
use jmap_test_support::unique_suffix;
use serde_json::json;

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

/// The keyfile of `live-stalwart-collection.rs`, for the account of the
/// recipient: contacts and calendars enabled, mail disabled.
fn keyfile(port: u16, user: &str) -> String {
    Keyfile::new(
        SourceKind::Collection {
            contacts: true,
            calendars: true,
            mail: false,
        },
        port,
    )
    .display_name("JMAP functional live-Stalwart shared collection test")
    .user(user)
    .to_string()
}

/// The owner shares an address book (writable) and a calendar (read-only)
/// with the recipient. The Evolution account of the recipient gets one child
/// for each, named `<owner account name>: <collection>`. The children are
/// not removable, because no grant has `mayDelete`. The owner removes the
/// two grants, and the next populate removes the two children. At the end,
/// the test destroys the two collections.
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

    // The name of the owner account in the session of the recipient. The
    // display name of the child contains it.
    let recipient = connect(&origin, &recipient_user, &recipient_password);
    let owner_name = recipient
        .session()
        .accounts
        .get(&calendars_account)
        .expect("the grant puts the owner's account in the recipient's session")
        .name
        .clone();

    // Each name contains the kind, because JMAP ids are unique only per type.
    // Stalwart can give a new book and a new calendar the same id.
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
            Some(format!("{owner_name}: {name}").as_str()),
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
