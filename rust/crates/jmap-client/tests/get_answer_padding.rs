// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A `/get` answer can contain objects that the request did not name.
//! RFC 8620 §5.1 returns only the requested ids, so the client must drop the
//! others for each chunk. Otherwise a server with a small `maxObjectsInGet`
//! can add objects to each answer and make the client keep all of them.

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use serde_json::{Value, json};

/// The number of unrequested objects that the server adds to each answer.
const PADDING: usize = 3;

fn session() -> Value {
    let caps = [
        "urn:ietf:params:jmap:core",
        "urn:ietf:params:jmap:mail",
        "urn:ietf:params:jmap:submission",
        "urn:ietf:params:jmap:contacts",
        "urn:ietf:params:jmap:calendars",
        "urn:ietf:params:jmap:principals",
    ];
    let mut capabilities = serde_json::Map::new();
    let mut account_capabilities = serde_json::Map::new();
    for cap in caps {
        capabilities.insert(cap.to_owned(), json!({}));
        account_capabilities.insert(cap.to_owned(), json!({}));
    }
    capabilities.insert(
        "urn:ietf:params:jmap:core".to_owned(),
        json!({
            "maxSizeUpload": 50000000,
            "maxConcurrentUpload": 4,
            "maxSizeRequest": 10000000,
            "maxConcurrentRequests": 4,
            "maxCallsInRequest": 16,
            "maxObjectsInGet": 2,
            "maxObjectsInSet": 500,
            "collationAlgorithms": ["i;ascii-casemap"]
        }),
    );
    json!({
        "capabilities": capabilities,
        "accounts": {
            "acc1": {
                "name": "user@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": account_capabilities
            }
        },
        "primaryAccounts": {},
        "username": "user@example.com",
        "apiUrl": "https://mail.example.internal/jmap/",
        "downloadUrl": "https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "https://mail.example.internal/jmap/upload/{accountId}/",
        "eventSourceUrl": "https://mail.example.internal/jmap/eventsource/",
        "state": "0"
    })
}

/// One object of the type that `method` returns, with the id `id`.
fn object(method: &str, id: &str) -> Value {
    match method {
        "Email/get" => json!({"id": id}),
        "Thread/get" => json!({"id": id, "emailIds": []}),
        "EmailSubmission/get" => json!({"id": id, "identityId": "i1", "emailId": "m1"}),
        "SearchSnippet/get" => json!({"emailId": id}),
        "ContactCard/get" => json!({"id": id, "@type": "Card", "version": "1.0"}),
        "CalendarEvent/get" => json!({"id": id, "@type": "Event"}),
        "Principal/get" => json!({"id": id, "type": "individual", "name": id, "capabilities": {}}),
        other => panic!("unexpected method {other}"),
    }
}

/// A server that answers each `/get` with the requested objects, then
/// [`PADDING`] objects and [`PADDING`] `notFound` ids that nobody requested.
struct PaddingServer;

impl Transport for PaddingServer {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let body = match request.method {
            HttpMethod::Get => session(),
            HttpMethod::Post => {
                let sent: Value = serde_json::from_slice(request.body.unwrap()).unwrap();
                let responses: Vec<Value> = sent["methodCalls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|call| {
                        let method = call[0].as_str().unwrap();
                        let arguments = &call[1];
                        let ids = arguments
                            .get("ids")
                            .or_else(|| arguments.get("emailIds"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let first = ids.first().and_then(Value::as_str).unwrap_or("none");
                        let mut list: Vec<Value> = ids
                            .iter()
                            .map(|id| object(method, id.as_str().unwrap()))
                            .collect();
                        let mut not_found = Vec::new();
                        for n in 0..PADDING {
                            list.push(object(method, &format!("pad-{first}-{n}")));
                            not_found.push(json!(format!("gone-{first}-{n}")));
                        }
                        json!([
                            method,
                            {
                                "accountId": "acc1",
                                "state": "s1",
                                "list": list,
                                "notFound": not_found
                            },
                            call[2]
                        ])
                    })
                    .collect();
                json!({"methodResponses": responses, "sessionState": "0"})
            }
        };
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: body.to_string().into_bytes(),
            final_url: request.url.to_owned(),
        })
    }
}

fn client() -> Client {
    Client::builder()
        .transport(PaddingServer)
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds")
}

fn account() -> Id {
    Id::new("acc1")
}

/// Three ids, so that `maxObjectsInGet` 2 makes two chunks.
fn requested() -> Vec<Id> {
    vec![Id::new("a"), Id::new("b"), Id::new("c")]
}

fn ids_of<'a>(ids: impl Iterator<Item = Option<&'a Id>>) -> Vec<String> {
    ids.map(|id| id.expect("every object has an id").to_string())
        .collect()
}

const EXPECTED: [&str; 3] = ["a", "b", "c"];

#[test]
fn email_get_keeps_only_the_requested_emails() {
    let emails = client().email_get(&account(), &requested(), None).unwrap();
    assert_eq!(ids_of(emails.iter().map(|e| e.id.as_ref())), EXPECTED);
}

#[test]
fn thread_get_keeps_only_the_requested_threads() {
    let threads = client().thread_get(&account(), requested()).unwrap();
    assert_eq!(ids_of(threads.iter().map(|t| t.id.as_ref())), EXPECTED);
}

#[test]
fn email_submission_get_keeps_only_the_requested_submissions() {
    let submissions = client()
        .email_submission_get(&account(), requested())
        .unwrap();
    assert_eq!(ids_of(submissions.iter().map(|s| s.id.as_ref())), EXPECTED);
}

#[test]
fn search_snippet_get_keeps_only_the_requested_snippets() {
    let snippets = client()
        .search_snippet_get(
            &account(),
            requested(),
            None::<jmap_proto::mail::EmailQueryFilter>,
        )
        .unwrap();
    assert_eq!(ids_of(snippets.iter().map(|s| Some(&s.email_id))), EXPECTED);
}

#[test]
fn contact_get_keeps_only_the_requested_cards_and_not_found_ids() {
    let response = client().contact_get(&account(), &requested()).unwrap();
    assert_eq!(
        ids_of(response.list.iter().map(|c| c.id.as_ref())),
        EXPECTED
    );
    assert!(response.not_found.is_empty(), "{:?}", response.not_found);
}

#[test]
fn event_get_keeps_only_the_requested_events_and_not_found_ids() {
    let response = client().event_get(&account(), &requested()).unwrap();
    assert_eq!(
        ids_of(response.list.iter().map(|e| e.id.as_ref())),
        EXPECTED
    );
    assert!(response.not_found.is_empty(), "{:?}", response.not_found);
}

#[test]
fn principal_get_keeps_only_the_requested_principals() {
    let principals = client().principal_get(&account(), &requested()).unwrap();
    assert_eq!(ids_of(principals.iter().map(|p| p.id.as_ref())), EXPECTED);
}

/// A `jmap-mockd` that adds every unrequested email of the account to each
/// `Email/get` answer, seeded with the emails `E1` to `E4`.
fn padding_mock(objects_in_get: u64) -> MockServer {
    let server = MockServer::builder()
        .email_get_includes_unrequested()
        .objects_in_get(objects_in_get)
        .start();
    let state = server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&server.account_id()).unwrap();
    let inbox = account.seed_mailbox("Inbox", Some(jmap_proto::mail::role::INBOX));
    for n in 1..=4 {
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            &format!("Subject {n}"),
            "Body",
            &format!("2026-08-0{n}T10:00:00Z"),
        ));
    }
    drop(state);
    server
}

#[test]
fn email_get_against_the_padding_mock_returns_only_the_one_requested_email() {
    let server = padding_mock(500);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let emails = client
        .email_get(&server.account_id(), &[Id::new("E1")], None)
        .unwrap();
    assert_eq!(ids_of(emails.iter().map(|e| e.id.as_ref())), ["E1"]);
}

#[test]
fn email_get_against_the_padding_mock_drops_padding_from_every_chunk() {
    let server = padding_mock(2);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let requested = [Id::new("E1"), Id::new("E3"), Id::new("E4")];
    let calls_before = server.method_calls().len();
    let emails = client
        .email_get(&server.account_id(), &requested, Some(&["subject"]))
        .unwrap();
    assert_eq!(
        ids_of(emails.iter().map(|e| e.id.as_ref())),
        ["E1", "E3", "E4"]
    );
    let subjects: Vec<_> = emails.iter().map(|e| e.subject.as_deref()).collect();
    assert_eq!(
        subjects,
        [Some("Subject 1"), Some("Subject 3"), Some("Subject 4")]
    );
    let gets = server.method_calls()[calls_before..]
        .iter()
        .filter(|name| name.as_str() == "Email/get")
        .count();
    assert_eq!(gets, 2, "maxObjectsInGet 2 splits three ids into two calls");
}
