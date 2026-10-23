// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! End to end: a meeting that a real `evolution-calendar-factory` organizes
//! on a real server gets to a guest on that server exactly one time. The
//! server sends it, not Evolution. The answer of the guest comes back into
//! the calendar of the organizer.
//!
//! `calendar-scheduling.rs` tests the backend against the mock.
//! `live_server_scheduling.rs` in `jmap-cal-sync` tests that the server
//! delivers what the sync layer asks for. This test joins the two. On the
//! organizer side, `functional-cal-scheduling-live-client`
//! (`tests/functional/cal-scheduling-live-client.c`) does what the comp
//! editor does. On the guest side, a direct JMAP connection reads the account
//! of the guest, because only that account shows what the guest got. The
//! client creates two meetings. The meeting that the user did not send must
//! not get to the guest.
//!
//! Evolution sends nothing for a backend that advertises `create-messages`
//! (`itip-utils.c`), and this program has no mail transport. Thus each
//! invitation in the inbox of the guest came from the server. The test counts
//! them to check that the server delivers exactly one time.
//!
//! ## How to run it
//!
//! Use two throwaway accounts on the same server, so that delivery stays on
//! that server (see `docs/manual-test-live-server.md`, steps 3 and 3a). The
//! write-test account is the organizer, and the recipient account is the
//! guest.
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-scheduling -- --ignored
//! ```
//!
//! When one of the accounts is not configured, the test skips and does not
//! fail.

use std::collections::BTreeMap;
use std::env;
use std::time::{Duration, Instant};

use jmap_client::{Client, Credentials};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter};
use jmap_proto::mail::EmailQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_MAIL};
use serde_json::json;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(user, password)`, or `None` to skip.
fn account(user_var: &str, password_var: &str) -> Option<(String, String)> {
    let user = env::var(user_var).ok()?;
    let password = env::var(password_var)
        .unwrap_or_else(|_| panic!("{user_var} is set but {password_var} is not"));
    Some((user, password))
}

fn connect(origin: &str, user: &str, password: &str) -> Client {
    // Always rebase, as the backend does behind the loopback proxy.
    Client::builder()
        .rebase_urls_to_origin(true)
        .connect(
            origin,
            Credentials::basic(user.to_owned(), password.to_owned()),
        )
        .unwrap_or_else(|error| panic!("could not fetch the session document for {user}: {error}"))
}

/// The calendar keyfile of the organizer, as in
/// `live-stalwart-calendar-attendees.rs`. It has no `[WebDAV Backend]`
/// group, as a new source has none.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart scheduling test\n\
         Enabled=true\n\
         \n\
         [Calendar]\n\
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

/// Runs the client one time in a new scratch tree named `name` and returns
/// its observations. If the client fails, the test fails with its full output.
fn run_client(
    name: &str,
    port: u16,
    user: &str,
    password: &str,
    args: &[&str],
) -> BTreeMap<String, String> {
    let client = required_path("JMAP_FUNCTIONAL_CAL_SCHEDULING_LIVE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");
    let mut session = Session::new(format!(
        "{}/live-stalwart-calendar-scheduling-{name}",
        env!("CARGO_TARGET_TMPDIR")
    ));
    session.write_source("jmap-functional", &keyfile(port, user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let output = session.run(&client, args);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the {name} run failed with {}\n--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}",
        output.status
    );
    observations(&stdout)
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// The copies of the meeting titled `title` in `account_id`.
fn copies_titled(client: &Client, account_id: &Id, title: &str) -> Vec<CalendarEvent> {
    let filter = CalendarEventQueryFilter {
        title: Some(title.to_owned()),
        ..CalendarEventQueryFilter::default()
    };
    let ids = client.event_query(account_id, filter).unwrap().ids;
    // A title filter is a substring match, so keep the exact titles only.
    client
        .event_get(account_id, &ids)
        .unwrap()
        .list
        .into_iter()
        .filter(|copy| copy.title.as_deref() == Some(title))
        .collect()
}

/// The `participationStatus` of `address` in the copy titled `title` in
/// `account_id`, if the copy and the participant exist.
fn guest_status(client: &Client, account_id: &Id, title: &str, address: &str) -> Option<String> {
    let copies = copies_titled(client, account_id, title);
    let participants = copies.first()?.participants.as_ref()?;
    participants.values().find_map(|participant| {
        let stated = participant.get("calendarAddress")?.as_str()?;
        if !stated.eq_ignore_ascii_case(address) {
            return None;
        }
        Some(
            participant
                .get("participationStatus")
                .and_then(|status| status.as_str())
                .unwrap_or("needs-action")
                .to_owned(),
        )
    })
}

/// The subjects of the messages of the guest that contain `title`.
fn guest_mail(guest: &Client, account_id: &Id, title: &str) -> Vec<String> {
    let filter = EmailQueryFilter {
        text: Some(title.to_owned()),
        ..EmailQueryFilter::default()
    };
    let ids = guest
        .email_query(account_id, filter, None, None, 0)
        .unwrap()
        .ids;
    guest
        .email_get(account_id, &ids, Some(&["subject"][..]))
        .unwrap()
        .into_iter()
        .filter_map(|email| email.subject)
        .collect()
}

/// Calls `probe` until `done` is true or `within` has passed. Returns the last
/// answer.
fn wait_for<T>(within: Duration, probe: impl Fn() -> T, done: impl Fn(&T) -> bool) -> T {
    let deadline = Instant::now() + within;
    loop {
        let answer = probe();
        if done(&answer) || Instant::now() >= deadline {
            return answer;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "needs a real JMAP server and two accounts; see docs/manual-test-live-server.md"]
fn a_meeting_saved_in_evolution_reaches_the_guest_once_and_the_answer_comes_back() {
    let Some((organizer_user, organizer_password)) = account(
        "JMAP_LIVE_SERVER_WRITE_USER",
        "JMAP_LIVE_SERVER_WRITE_PASSWORD",
    ) else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER not set; skipping");
        return;
    };
    let Some((guest_user, guest_password)) = account(
        "JMAP_LIVE_SERVER_RECIPIENT_USER",
        "JMAP_LIVE_SERVER_RECIPIENT_PASSWORD",
    ) else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER not set; skipping");
        return;
    };
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let guest = connect(&origin, &guest_user, &guest_password);
    let guest_calendars = guest.primary_account(CAPABILITY_CALENDARS).unwrap();
    let guest_mailbox = guest.primary_account(CAPABILITY_MAIL).unwrap();

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let stamp = unique_suffix() & 0xffff_ffff_ffff;
    let sent = format!("agent-fncal-sched-sent-{stamp:x}");
    let quiet = format!("agent-fncal-sched-quiet-{stamp:x}");

    let created = run_client(
        "create",
        port,
        &organizer_user,
        &organizer_password,
        &[
            "create",
            "jmap-functional",
            &organizer_user,
            &guest_user,
            &sent,
            &quiet,
        ],
    );
    for capability in [
        "create-messages",
        "save-schedules",
        "itip-suppress-on-remove-supported",
    ] {
        assert_eq!(
            created.get(capability).map(String::as_str),
            Some("1"),
            "{capability} not advertised, so Evolution would mail the guest too: {created:?}"
        );
    }

    // The server delivers the sent meeting one time into the calendar of the
    // guest, and the invitation one time into the inbox of the guest.
    let copies = wait_for(
        Duration::from_secs(30),
        || copies_titled(&guest, &guest_calendars, &sent),
        |copies| !copies.is_empty(),
    );
    assert_eq!(
        copies.len(),
        1,
        "the meeting lands in the guest's calendar exactly once: {copies:?}"
    );
    let mail = wait_for(
        Duration::from_secs(30),
        || guest_mail(&guest, &guest_mailbox, &sent),
        |mail| !mail.is_empty(),
    );
    eprintln!("the guest's mail about the sent meeting: {mail:?}");
    assert_eq!(
        mail.len(),
        1,
        "the invitation reaches the guest's inbox exactly once: {mail:?}"
    );
    assert!(
        copies_titled(&guest, &guest_calendars, &quiet).is_empty()
            && guest_mail(&guest, &guest_mailbox, &quiet).is_empty(),
        "the meeting the user chose not to send reached the guest"
    );

    // The guest accepts on the guest copy and tells the server to send the
    // reply to the organizer, as a JMAP client of the guest does.
    let copy = &copies[0];
    let guest_address = format!("mailto:{guest_user}");
    let participant = copy
        .participants
        .as_ref()
        .and_then(|participants| {
            participants.iter().find(|(_, participant)| {
                participant
                    .get("calendarAddress")
                    .and_then(|address| address.as_str())
                    .is_some_and(|address| address.eq_ignore_ascii_case(&guest_address))
            })
        })
        .map(|(key, _)| key.clone())
        .unwrap_or_else(|| panic!("the guest's copy does not list the guest: {copy:?}"));
    guest
        .event_update_with_scheduling(
            &guest_calendars,
            copy.id.as_ref().expect("the server named the guest's copy"),
            json!({ format!("participants/{participant}/participationStatus"): "accepted" }),
            true,
        )
        .expect("the guest's accept is stored and sent");

    // The server puts the reply into the copy of the organizer at a later
    // time. Wait for it, so that the settle run below reads it.
    let organizer = connect(&origin, &organizer_user, &organizer_password);
    let organizer_calendars = organizer.primary_account(CAPABILITY_CALENDARS).unwrap();
    let answered = wait_for(
        Duration::from_secs(30),
        || guest_status(&organizer, &organizer_calendars, &sent, &guest_address),
        |status| status.as_deref() == Some("accepted"),
    );
    assert_eq!(
        answered.as_deref(),
        Some("accepted"),
        "the server never filed the guest's accept into the organizer's copy"
    );

    // A new Evolution view of the calendar of the organizer shows the
    // answer. Then it removes the two meetings: the sent meeting with a
    // cancellation, the other meeting without.
    let settled = run_client(
        "settle",
        port,
        &organizer_user,
        &organizer_password,
        &["settle", "jmap-functional", &guest_user, &sent, &quiet],
    );
    assert_eq!(
        settled.get("guest-partstat").map(String::as_str),
        Some("ACCEPTED"),
        "the organizer's calendar does not show the guest's accept: {settled:?}"
    );
    assert_eq!(
        settled.get("removed-quiet").map(String::as_str),
        Some("1"),
        "{settled:?}"
    );
    assert_eq!(
        settled.get("removed-sent").map(String::as_str),
        Some("1"),
        "{settled:?}"
    );

    let cancelled = wait_for(
        Duration::from_secs(30),
        || copies_titled(&guest, &guest_calendars, &sent),
        |copies| {
            copies
                .iter()
                .all(|copy| copy.status.as_deref() == Some("cancelled"))
        },
    );
    assert!(
        cancelled
            .iter()
            .all(|copy| copy.status.as_deref() == Some("cancelled")),
        "the guest's copy is gone or marked cancelled: {cancelled:?}"
    );
    for copy in cancelled {
        if let Some(id) = copy.id {
            let _ = guest.event_destroy(&guest_calendars, &id);
        }
    }
}
