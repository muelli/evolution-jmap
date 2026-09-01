// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `VacationResponse` (RFC 8621 §8): capability detection, the singleton
//! `get`/`set`, and the two refusals RFC 8621 requires (no create, no
//! destroy). No Evolution UI or EDS surface consumes this yet; the wiring
//! waits ready, the same way scheduled send did.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::error::set;
use jmap_proto::mail::VacationResponse;
use jmap_proto::methods::SetRequest;
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_VACATION_RESPONSE};
use serde_json::json;

/// A server advertises `urn:ietf:params:jmap:vacationresponse` both at
/// session level and on the account, and the account resolves through the
/// same generic mechanism every other capability does.
#[test]
fn vacation_response_capability_is_advertised_and_resolves_to_the_account() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    assert!(client.session().vacation_response_capability().is_some());
    assert_eq!(
        client
            .session()
            .resolve_primary_account(CAPABILITY_VACATION_RESPONSE),
        Some(&account_id)
    );
}

/// The object always exists (RFC 8621 §8.1): a server that has never seen a
/// `VacationResponse/set` still answers `get` with a disabled singleton,
/// never `notFound`.
#[test]
fn vacation_response_get_returns_the_disabled_singleton_by_default() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let vacation = client.vacation_response_get(&account_id).unwrap();
    assert_eq!(vacation.id.unwrap().as_str(), "singleton");
    assert!(!vacation.is_enabled);
    assert!(vacation.subject.is_none());
}

/// A `VacationResponse/set` update round-trips every RFC 8621 §8.1 property:
/// `isEnabled`, the `fromDate`/`toDate` window, `subject`, `textBody` and
/// `htmlBody`.
#[test]
fn vacation_response_update_round_trips_rfc8621_fields() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    client
        .vacation_response_update(
            &account_id,
            json!({
                "isEnabled": true,
                "fromDate": "2026-08-01T00:00:00Z",
                "toDate": "2026-08-15T00:00:00Z",
                "subject": "Out of office",
                "textBody": "I am on annual leave.",
                "htmlBody": "<p>I am on annual leave.</p>",
            }),
        )
        .unwrap();

    let vacation = client.vacation_response_get(&account_id).unwrap();
    assert!(vacation.is_enabled);
    assert_eq!(vacation.from_date.unwrap().as_str(), "2026-08-01T00:00:00Z");
    assert_eq!(vacation.to_date.unwrap().as_str(), "2026-08-15T00:00:00Z");
    assert_eq!(vacation.subject.as_deref(), Some("Out of office"));
    assert_eq!(vacation.text_body.as_deref(), Some("I am on annual leave."));
    assert_eq!(
        vacation.html_body.as_deref(),
        Some("<p>I am on annual leave.</p>")
    );
}

/// RFC 8621 §8: "This is a singleton type... A client MUST NOT attempt to
/// create... or destroy" it. A create is refused with a `singleton` `SetError`
/// rather than being honoured or crashing the mock.
#[test]
fn vacation_response_cannot_be_created() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = SetRequest::<VacationResponse>::new(account_id.clone())
        .create("new", VacationResponse::new(true));
    let arguments = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_VACATION_RESPONSE],
            "VacationResponse/set",
            &request,
        )
        .unwrap();
    let response: jmap_proto::methods::SetResponse<VacationResponse> =
        serde_json::from_value(arguments).unwrap();

    let not_created = response.not_created.expect("create must be refused");
    assert_eq!(not_created["new"].error_type, set::SINGLETON);
    assert!(response.created.is_none());
}

/// The destroy half of the same rule: `singleton`, not a silent no-op and not
/// an actual removal.
#[test]
fn vacation_response_cannot_be_destroyed() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let singleton_id: jmap_proto::Id = "singleton".into();
    let request =
        SetRequest::<VacationResponse>::new(account_id.clone()).destroy(singleton_id.clone());
    let arguments = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_VACATION_RESPONSE],
            "VacationResponse/set",
            &request,
        )
        .unwrap();
    let response: jmap_proto::methods::SetResponse<VacationResponse> =
        serde_json::from_value(arguments).unwrap();

    let not_destroyed = response.not_destroyed.expect("destroy must be refused");
    assert_eq!(not_destroyed[&singleton_id].error_type, set::SINGLETON);
    assert!(response.destroyed.is_none());

    // Still there and unaffected on the next get.
    let vacation = client.vacation_response_get(&account_id).unwrap();
    assert_eq!(vacation.id.unwrap().as_str(), "singleton");
}
