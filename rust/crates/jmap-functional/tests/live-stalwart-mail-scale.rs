// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mail version of `live-stalwart-book-scale.rs` and
//! `live-stalwart-calendar-scale.rs`. It measures the first open of a large
//! inbox by the Camel provider against a real server: the time, and the peak
//! memory of the process.
//!
//! The book and calendar tests measure a factory daemon, because an EDS
//! backend runs there. A Camel provider has no daemon: it loads into the
//! process of the mail client. Thus this test measures
//! `functional-mail-live-client`, which is in place of Evolution. Its `scale`
//! phase reports three steps:
//!
//! - The open of the inbox, which does the first sync (it gets each row and
//!   writes it into the Camel summary).
//! - A delta refresh, as each later open does.
//! - A summary read for each listed message, as the message list does to
//!   show its rows.
//!
//! A raw [`Client`] creates the messages over JMAP in `Email/set` batches
//! before Camel connects. Thus the open is the first open of a large inbox.
//!
//! ## How to run it
//!
//! Use the same environment as `live-stalwart-mail.rs`. Build with
//! `-DENABLE_FUNCTIONAL_TESTS=ON`, so that `JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT`,
//! `_MAIL_MODULE` and `_MAIL_URLS` are set. Install the EDS runtime packages
//! (`ci/install-deps-functional.sh`). `JMAP_SCALE_TEST_MESSAGES` sets the
//! inbox size (default 10000). Use a throwaway account only for this test,
//! because the test leaves all the messages in its inbox.
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-mail-scale -- --ignored --nocapture
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_functional::{
    Keyfile, Session, SourceKind, observations, required_path, spawn_loopback_proxy,
};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL};
use serde_json::{Map, Value, json};

/// A value that is unique to this process. Thus a new run on the same
/// throwaway account does not count the messages of an earlier run.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn inbox_size() -> usize {
    env::var("JMAP_SCALE_TEST_MESSAGES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10_000)
}

/// `(origin, user, password)`, or `None` to skip. The same as
/// `live_server_params` in `live-stalwart-mail.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// Retries `attempt` on a 429 (`Http.rateLimitAuthenticated`) up to 19 times,
/// with a 5-second sleep before each retry. A copy of `retrying` in
/// `live-stalwart-book-scale.rs` (see its comment for the reason).
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

/// `live-stalwart-mail.rs`'s own keyfile.
fn keyfile(port: u16, user: &str) -> String {
    Keyfile::new(SourceKind::MailAccount, port)
        .display_name("JMAP functional live-Stalwart mail scale test")
        .user(user)
        .to_string()
}

/// One `Email/set` create entry: a plain-text message in `inbox_id` with a
/// unique `subject`. Each message is one minute after the one before, so the
/// summary has different dates to sort, as in a real inbox.
fn message(inbox_id: &str, subject: &str, index: usize) -> Value {
    let received = 1_767_225_600 + index as i64 * 60;
    let received_at = chrono_free_utc(received);
    json!({
        "mailboxIds": { inbox_id: true },
        "from": [{ "name": "Scale Sender", "email": "scale-sender@example.com" }],
        "to": [{ "name": "Scale Reader", "email": "scale-reader@example.com" }],
        "subject": subject,
        "receivedAt": received_at,
        "sentAt": received_at,
        "bodyStructure": { "type": "text/plain", "partId": "1" },
        "bodyValues": { "1": { "value": format!("Message {index} of the scale run.\r\n") } },
    })
}

/// `seconds` since the epoch as an RFC 3339 UTC date. This avoids a date
/// crate dependency for one format string.
fn chrono_free_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    // The civil-from-days algorithm of Howard Hinnant.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

#[test]
fn the_date_helper_formats_known_instants() {
    assert_eq!(chrono_free_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(chrono_free_utc(1_767_225_600), "2026-01-01T00:00:00Z");
    assert_eq!(chrono_free_utc(951_782_400 + 3_661), "2000-02-29T01:01:01Z");
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn camels_first_open_of_a_real_sized_inbox_is_measured() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-EDS mail scale leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");
    let n = inbox_size();
    let prefix = format!(
        "agent-fnmailscale-{:x}-",
        unique_suffix() & 0xffff_ffff_ffff
    );

    // The raw seeding client connects directly to the server, not through
    // the loopback proxy or Camel. See `live-stalwart-book-scale.rs` for why
    // it rebases the URLs of the session.
    let raw_client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch the session document for the raw seeding client");
    let account_id = raw_client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let inbox_id = raw_client
        .mailbox_get(&account_id)
        .expect("Mailbox/get failed for the raw seeding client")
        .list
        .into_iter()
        .find(|mailbox| mailbox.role.as_deref() == Some("inbox"))
        .expect("the write-test account needs an inbox")
        .id
        .expect("the server named the inbox");
    let batch = raw_client
        .session()
        .max_objects_in_set()
        .map_or(500, |limit| limit.min(500) as usize);

    let seed_start = Instant::now();
    for first in (0..n).step_by(batch) {
        let create: Map<String, Value> = (first..n.min(first + batch))
            .map(|i| {
                (
                    format!("m{i}"),
                    message(inbox_id.as_str(), &format!("{prefix}{i}"), i),
                )
            })
            .collect();
        let arguments = json!({ "accountId": account_id, "create": create });
        let answer = retrying(|| {
            raw_client.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/set", &arguments)
        })
        .unwrap_or_else(|error| panic!("Email/set create from #{first} failed: {error}"));
        let created = answer
            .get("created")
            .and_then(Value::as_object)
            .map_or(0, Map::len);
        assert_eq!(
            created,
            create.len(),
            "Email/set created {created} of {} messages from #{first}: {:?}",
            create.len(),
            answer.get("notCreated")
        );
    }
    eprintln!(
        "SCALE: seeded {n} messages directly over JMAP in {:?}",
        seed_start.elapsed()
    );

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-mail-scale"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_camel_provider(&module, &urls);

    let open_start = Instant::now();
    let (output, peak_rss_kb) = session.run_measuring_peak_rss(
        &client_binary,
        &["jmap-functional", &prefix, "scale"],
        "functional-mail-live-client",
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
    let number = |key: &str| -> u64 {
        seen.get(key)
            .unwrap_or_else(|| panic!("no '{key}' observation\n{report}"))
            .parse()
            .unwrap_or_else(|_| panic!("'{key}' was not a number\n{report}"))
    };
    assert_eq!(
        number("inbox-matching"),
        n as u64,
        "Camel's first listing should see exactly the {n} messages this run just seeded\n{report}"
    );

    eprintln!(
        "SCALE: Camel's first open of an inbox of {} messages ({n} seeded by this run): \
         open (first sync) {} ms, delta refresh {} ms, summary read {} ms, \
         whole client run {open_elapsed:?}",
        number("inbox-count"),
        number("open-ms"),
        number("refresh-ms"),
        number("summary-ms"),
    );
    match peak_rss_kb {
        Some(kb) => eprintln!("SCALE: functional-mail-live-client peak RSS {kb} kB"),
        None => eprintln!("SCALE: never saw the client process to measure"),
    }
}
