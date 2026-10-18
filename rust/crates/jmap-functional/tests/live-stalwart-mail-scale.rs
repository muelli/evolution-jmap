// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mail twin of `live-stalwart-book-scale.rs` and
//! `live-stalwart-calendar-scale.rs`: how long the Camel provider's first
//! open of an already-large inbox takes against a real server, and how much
//! memory the process holding it peaks at.
//!
//! The book and calendar legs measure a factory daemon, because that is where
//! an EDS backend lives. A Camel provider has no daemon: it is dlopened into
//! the mail client's own process, so the process measured here is
//! `functional-mail-live-client` itself, the stand-in for Evolution. Its
//! `scale` phase reports three steps separately: opening the inbox, which is
//! where the first sync happens (every row fetched and written into Camel's
//! summary), a delta refresh like every later open makes, and a summary read
//! for every listed message, which is what the message list does to draw its
//! rows.
//!
//! Messages are seeded directly over JMAP with a raw [`Client`], in
//! `Email/set` batches, before Camel ever connects, so the open really is the
//! first one against an inbox that was already this size.
//!
//! ## Running it
//!
//! Same environment as `live-stalwart-mail.rs`, built through
//! `-DENABLE_FUNCTIONAL_TESTS=ON` so `JMAP_FUNCTIONAL_MAIL_LIVE_CLIENT`,
//! `_MAIL_MODULE` and `_MAIL_URLS` are set, with the EDS runtime packages
//! installed (`ci/install-deps-functional.sh`). `JMAP_SCALE_TEST_MESSAGES`
//! overrides the inbox size (default 10000). Use a throwaway account created
//! for this alone: the run leaves that many messages in its inbox.
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-mail-scale -- --ignored --nocapture
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL};
use serde_json::{Map, Value, json};

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never counts a previous run's messages as this
/// one's. Mirrors every other live-server test's own copy.
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

/// `(origin, user, password)`, or `None` to skip. Mirrors
/// `live-stalwart-mail.rs`'s own `live_server_params`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// Retries `attempt` on a 429 (`Http.rateLimitAuthenticated`) with a short
/// sleep, up to 20 times. Mirrors `live-stalwart-book-scale.rs`'s own copy,
/// duplicated for the reason it gives.
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
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart mail scale test\n\
         Enabled=true\n\
         \n\
         [Mail Account]\n\
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

/// One `Email/set` create entry: a plain-text message in `inbox_id`, with
/// `subject` unique to it, each a minute after the one before, so the summary
/// has distinct dates to sort on like a real inbox.
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

/// `seconds` since the epoch as an RFC 3339 UTC date, without pulling a date
/// crate into this crate's dev-dependencies for one format string.
fn chrono_free_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
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

    // The raw seeding client talks to the server directly, not through the
    // loopback proxy or Camel. See `live-stalwart-book-scale.rs` for why it
    // rebases the session's URLs.
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let raw_client = Client::builder()
        .rebase_urls_to_origin(rebase)
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
