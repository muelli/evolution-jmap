// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live probe of `Principal/getAvailability` (draft-ietf-jmap-calendars §2.2)
// against a real server. Built to answer the roadmap's Track E "OPERATOR
// CONFIRMATION now RUNNER-CLAIMABLE" item: does the real deployment
// implement the method at all, and if so, does it spell the response
// fields the way `jmap-proto::principals` assumes?
//
// Deliberately uses `Client::single_call` to get the raw `serde_json::Value`
// back rather than the typed `Client::get_availability` wrapper, so a
// field-name mismatch shows up as a JSON diff in the printed output instead
// of an opaque deserialization error.
//
// Usage:
//   cargo run -p evolution-jmap-client --example principals-availability-probe -- \
//       <origin> <user> <password>
// e.g.
//   ... -- http://stalwart-1....internal:8080 agent1@agent-avail-probe.test '...'

use jmap_client::{Client, Credentials};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CORE, CAPABILITY_PRINCIPALS};
use serde_json::json;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(origin), Some(user), Some(password)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: principals-availability-probe <origin> <user> <password>");
        std::process::exit(2);
    };

    let client =
        Client::connect(&origin, Credentials::basic(user.clone(), password)).expect("connect");
    let session = client.session();
    println!(
        "session capabilities: {}",
        session
            .capabilities
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );

    if !session.capabilities.contains_key(CAPABILITY_PRINCIPALS) {
        println!("NOT SUPPORTED: server does not advertise {CAPABILITY_PRINCIPALS}");
        return;
    }
    if !session.capabilities.contains_key(CAPABILITY_CALENDARS) {
        println!("NOTE: server does not advertise {CAPABILITY_CALENDARS} at all");
    }

    println!("apiUrl = {}", session.api_url);

    let account_id = client
        .primary_account(CAPABILITY_PRINCIPALS)
        .or_else(|_| client.primary_account(CAPABILITY_CORE))
        .expect("an account for the principals or core capability");
    println!("account_id = {account_id}");

    // Resolve our own principal via Principal/query-by-email, mirroring
    // jmap_cal_sync::freebusy's attendee resolution.
    let ids = client
        .principal_query(&account_id, PrincipalQueryFilter::email(&user))
        .expect("Principal/query");
    println!("Principal/query({user}) -> {ids:?}");
    let Some(principal_id) = ids.into_iter().next() else {
        println!("NOT FOUND: no principal resolves for {user}");
        return;
    };

    // Raw Principal/get, printed verbatim, to see real field spelling
    // (capabilities bag especially) before trusting the typed wrapper.
    let get_args = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS],
            "Principal/get",
            &json!({ "accountId": account_id, "ids": [principal_id] }),
        )
        .expect("Principal/get");
    println!(
        "Principal/get raw response:\n{}",
        serde_json::to_string_pretty(&get_args).unwrap()
    );

    // Create a real busy event first, so getAvailability has something
    // non-empty to answer with — an empty `list` proves nothing about field
    // spelling.
    let calendar_id = client
        .calendars(&account_id)
        .expect("Calendar/get")
        .into_iter()
        .next()
        .and_then(|calendar| calendar.id)
        .expect("account needs a default calendar");
    let event = CalendarEvent::simple(
        calendar_id,
        "agent-avail-probe busy slot",
        "2026-01-01T10:00:00",
        "PT1H",
    );
    let created = client
        .event_create(&account_id, &event)
        .expect("event_create");
    println!("created event id = {:?}", created.id);

    // Raw Principal/getAvailability, printed verbatim.
    let availability_args = client.single_call(
        &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS, CAPABILITY_CALENDARS],
        "Principal/getAvailability",
        &json!({
            "accountId": account_id,
            "id": principal_id,
            "utcStart": "2026-01-01T00:00:00Z",
            "utcEnd": "2026-01-02T00:00:00Z",
            "showDetails": true,
        }),
    );
    match availability_args {
        Ok(value) => println!(
            "Principal/getAvailability raw response:\n{}",
            serde_json::to_string_pretty(&value).unwrap()
        ),
        Err(error) => println!("Principal/getAvailability FAILED: {error}"),
    }

    // Same call through the typed wrapper, to confirm it deserializes the
    // real response cleanly (or show exactly how it doesn't).
    match client.get_availability(
        &account_id,
        &principal_id,
        "2026-01-01T00:00:00Z",
        "2026-01-02T00:00:00Z",
        true,
    ) {
        Ok(periods) => println!("typed get_availability() -> {periods:?}"),
        Err(error) => println!("typed get_availability() FAILED: {error}"),
    }

    if let Some(id) = created.id {
        client
            .event_destroy(&account_id, &id)
            .expect("event_destroy (cleanup)");
        println!("cleaned up probe event {id}");
    }
}
