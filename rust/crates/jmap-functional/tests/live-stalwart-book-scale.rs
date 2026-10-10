// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Item 94(c): the real-EDS leg shared by `jmap-book-sync` and `jmap-cal-sync`'s
//! own scale checkpoints (item 94a/94b, both fully delivered through the sync
//! layer directly). Every number recorded there came from `BookSync`/`CalSync`
//! calling the real server with no EDS in between; this file asks the question
//! those numbers cannot: how long `evolution-addressbook-factory`'s own first
//! open of an already-large book takes, and how much memory it peaks at,
//! through the real meta-backend machinery (cache population, vCard
//! rendering for every card) rather than just the wire calls.
//!
//! Contacts are seeded directly over JMAP with a raw [`Client`] (mirroring
//! `live-stalwart-book-changes.rs`'s own "a real second client" pattern) so
//! the account is already large *before* EDS ever connects to it — unlike
//! `live-stalwart-book.rs`'s single-contact `write` phase, this never asks
//! EDS to do the importing. Confirmed clean at both 1,000 and the item's own
//! 5,000-card checkpoint (`docs/BOOK-SYNC-SCALE.md`): no scale-only bug, cost
//! growing sub-linearly with book size. `jmap-cal-sync`'s calendar-factory
//! counterpart is still open.
//!
//! ## Running it
//!
//! Same environment as every other `live-stalwart-*` test in this crate —
//! see `docs/manual-test-live-server.md` — built through
//! `-DENABLE_FUNCTIONAL_TESTS=ON` so `JMAP_FUNCTIONAL_BOOK_CLIENT`/
//! `_MODULE` are set. `JMAP_SCALE_TEST_CARDS` overrides the batch size
//! (default 1000); the write-test account should be a throwaway created for
//! this alone, since this leaves that many cards in its address book.
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-book-scale -- --ignored --nocapture
//! ```
//!
//! Also needs `-DENABLE_FUNCTIONAL_TESTS=ON` and the EDS *runtime* packages
//! (`docs/functional-tests.md`), which this runner's own dev VM does not
//! have installed natively -- run inside the `ubuntu:24.04` podman recipe
//! `docs/eds-version-matrix.md`'s "The `functional` job itself, reproduced
//! from scratch" section documents, same as every other `-live-stalwart`
//! leg's first confirmation.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::contacts::ContactCard;
use jmap_proto::session::CAPABILITY_CONTACTS;

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never mistakes a previous run's card for this
/// one's. Mirrors every other live-server test's own copy.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn batch_size() -> usize {
    env::var("JMAP_SCALE_TEST_CARDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(5000)
}

/// `(origin, user, password)`, or `None` to skip -- mirrors
/// `live-stalwart-book.rs`'s own `live_server_params`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// Retries `attempt` on a 429 (`Http.rateLimitAuthenticated`) with a short
/// sleep, up to 20 times. Mirrors `jmap-book-sync/tests/live_server_scale.rs`'s
/// own `retrying` exactly, duplicated rather than shared: this crate's
/// dev-dependencies stop at `jmap_client`/`jmap_proto`, and pulling in
/// `jmap-book-sync` just for one helper would cross a turf boundary this
/// file has no other reason to cross.
fn retrying<T>(mut attempt: impl FnMut() -> Result<T, ClientError>) -> Result<T, ClientError> {
    for remaining in (0..20).rev() {
        match attempt() {
            Err(ClientError::Http { status: 429, .. }) if remaining > 0 => {
                sleep(Duration::from_secs(5));
            }
            other => return other,
        }
    }
    unreachable!("the loop above always returns on its last iteration (remaining == 0)")
}

/// `address-book.rs`'s own keyfile, with a real `User=` added -- the same
/// shape `live-stalwart-book.rs` writes.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart scale test\n\
         Enabled=true\n\
         \n\
         [Address Book]\n\
         BackendName=jmap\n\
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

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn the_addressbook_factorys_first_open_of_a_real_sized_book_is_measured() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-EDS book scale leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");
    let n = batch_size();
    let run = unique_suffix();

    // A raw client, talking to the real server directly -- not through the
    // loopback proxy or EDS -- the same role `live-stalwart-book-changes.rs`'s
    // second client plays, here used to build the large book EDS has never
    // seen rather than to inject one change into it. `rebase_urls_to_origin`
    // mirrors every `jmap-*-sync` live-server test's own `connect_for_write`:
    // the session document can name a configured hostname this runner cannot
    // reach, independently of whether EDS is involved at all.
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let raw_client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch the session document for the raw seeding client");
    let account_id = raw_client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let address_book_id = raw_client
        .address_books(&account_id)
        .expect("AddressBook/get failed for the raw seeding client")
        .into_iter()
        .find(|book| book.is_default == Some(true))
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the default address book");

    let import_start = Instant::now();
    for i in 0..n {
        let name = format!("agent-fnbookscale-{run:x}-{i}");
        let email = format!("agent-fnbookscale-{run:x}-{i}@example.com");
        let card = ContactCard::simple(address_book_id.clone(), &name, &email);
        retrying(|| raw_client.contact_create(&account_id, &card))
            .unwrap_or_else(|error| panic!("ContactCard/set create #{i} failed: {error}"));
    }
    let import_elapsed = import_start.elapsed();
    eprintln!("SCALE: seeded {n} cards directly over JMAP in {import_elapsed:?}");

    // Only now does EDS enter the picture -- a fresh `Session`, so
    // `evolution-addressbook-factory`'s open of this account really is its
    // first, against a book that was already this size before it connected.
    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-book-scale"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_address_book_backend(&module);

    let open_start = Instant::now();
    let (output, peak_rss_kb) = session.run_measuring_peak_rss(
        &client_binary,
        &["jmap-functional", "list"],
        "evolution-addressbook-factory",
    );
    let open_elapsed = open_start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    let seen = observations(&stdout);
    let listed: usize = seen
        .get("contacts")
        .unwrap_or_else(|| panic!("no 'contacts' observation\n{report}"))
        .parse()
        .unwrap_or_else(|_| panic!("'contacts' was not a number\n{report}"));
    assert_eq!(
        listed, n,
        "the factory's first listing should see exactly the {n} cards this run just seeded, \
         on a freshly seeded throwaway account\n{report}"
    );

    eprintln!(
        "SCALE: evolution-addressbook-factory's first open + cold listing of {listed} cards \
         took {open_elapsed:?}"
    );
    match peak_rss_kb {
        Some(kb) => eprintln!("SCALE: evolution-addressbook-factory peak RSS {kb} kB"),
        None => eprintln!(
            "SCALE: never saw an evolution-addressbook-factory process to measure -- \
             did the open even reach the factory?"
        ),
    }
}
