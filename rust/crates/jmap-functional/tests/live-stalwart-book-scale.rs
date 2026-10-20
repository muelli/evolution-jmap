// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The real-EDS scale test for address books. The scale tests of
//! `jmap-book-sync` and `jmap-cal-sync` measure `BookSync`/`CalSync` against
//! the real server without EDS. This test measures the first open of a large
//! book by `evolution-addressbook-factory`: the time and the peak memory. It
//! includes the meta-backend work (cache population, vCard rendering for each
//! card), not only the wire calls.
//!
//! A raw [`Client`] creates the contacts over JMAP (as the second client of
//! `live-stalwart-book-changes.rs` does). Thus the account is large before
//! EDS connects, and EDS does not import the contacts. Results at 1,000 and
//! 5,000 cards: no scale-only bug, and the cost increases sub-linearly with
//! the book size. The calendar version is `live-stalwart-calendar-scale.rs`.
//!
//! ## How to run it
//!
//! Use the same environment as the other `live-stalwart-*` tests in this
//! crate (see `docs/manual-test-live-server.md`). Build with
//! `-DENABLE_FUNCTIONAL_TESTS=ON`, so that `JMAP_FUNCTIONAL_BOOK_CLIENT`/
//! `_MODULE` are set. `JMAP_SCALE_TEST_CARDS` sets the batch size (default
//! 5000). Use a throwaway write-test account only for this test, because the
//! test leaves all the cards in its address book.
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-book-scale -- --ignored --nocapture
//! ```
//!
//! The test also needs the EDS runtime packages (`docs/functional-tests.md`).
//! If the host does not have them, run the test in the `ubuntu:24.04` podman
//! recipe in the section "The `functional` job itself, reproduced from
//! scratch" of `docs/eds-version-matrix.md`.
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::contacts::ContactCard;
use jmap_proto::session::CAPABILITY_CONTACTS;

/// A value that is unique to this process. Thus a new run on the same
/// throwaway account does not read a card from an earlier run.
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

/// `(origin, user, password)`, or `None` to skip. The same as
/// `live_server_params` in `live-stalwart-book.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// Retries `attempt` on a 429 (`Http.rateLimitAuthenticated`) up to 19 times,
/// with a 5-second sleep before each retry. This is a copy of `retrying` in
/// `jmap-book-sync/tests/live_server_scale.rs`, for `ClientError`. This crate
/// does not depend on `jmap-book-sync`, and one helper is not a sufficient
/// reason to add that dependency.
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

/// The keyfile of `address-book.rs`, with a real `User=`, as in
/// `live-stalwart-book.rs`.
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

    // A raw client that connects directly to the real server, not through
    // the loopback proxy or EDS. It makes the large book before EDS sees it.
    // `rebase_urls_to_origin` is necessary because the session document can
    // name a hostname that this runner cannot reach.
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

    // EDS starts only now, in a new `Session`. Thus this is the first open of
    // the account by `evolution-addressbook-factory`, on a book that is
    // already large.
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
