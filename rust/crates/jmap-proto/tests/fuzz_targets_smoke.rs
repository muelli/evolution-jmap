// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable smoke tests for session, response, changes, and queryChanges fuzz targets.

use jmap_proto::methods::{ChangesResponse, QueryChangesResponse};
use jmap_proto::response::Response;
use jmap_proto::session::Session;

#[test]
fn hostile_bytes_do_not_panic_session_or_response_or_changes() {
    let hostile_inputs: &[&[u8]] = &[
        b"",
        b"{",
        b"[",
        b"}",
        b"]",
        b"null",
        b"true",
        b"12345",
        b"\"unterminated string",
        b"{\"methodResponses\": null}",
        b"{\"methodResponses\": [\"invalid\"]}",
        b"{\"sessionState\": 12345}",
        b"\xFF\xFE\x00\x00",
        b"{\"accountId\": null, \"oldState\": 123}",
        b"{\"accountId\": \"acc\", \"oldQueryState\": false}",
    ];

    for input in hostile_inputs {
        if let Ok(session) = serde_json::from_slice::<Session>(input) {
            let _ = serde_json::to_vec(&session);
        }
        if let Ok(resp) = serde_json::from_slice::<Response>(input) {
            let _ = serde_json::to_vec(&resp);
        }
        if let Ok(changes) = serde_json::from_slice::<ChangesResponse>(input) {
            let _ = serde_json::to_vec(&changes);
        }
        if let Ok(qc) = serde_json::from_slice::<QueryChangesResponse>(input) {
            let _ = serde_json::to_vec(&qc);
        }
    }
}

#[test]
fn changes_and_query_changes_seeds_roundtrip() {
    let changes_json = br#"{
        "accountId": "acc1",
        "oldState": "state1",
        "newState": "state2",
        "hasMoreChanges": false,
        "created": ["id1", "id2"],
        "updated": ["id3"],
        "destroyed": ["id4"]
    }"#;
    let chg: ChangesResponse =
        serde_json::from_slice(changes_json).expect("ChangesResponse parses");
    assert_eq!(chg.account_id.as_str(), "acc1");
    let serialized = serde_json::to_vec(&chg).expect("ChangesResponse serializes");
    assert!(serde_json::from_slice::<ChangesResponse>(&serialized).is_ok());

    let query_changes_json = br#"{
        "accountId": "acc1",
        "oldQueryState": "qs1",
        "newQueryState": "qs2",
        "total": 5,
        "removed": ["id1"],
        "added": [{"id": "id2", "index": 0}]
    }"#;
    let qc: QueryChangesResponse =
        serde_json::from_slice(query_changes_json).expect("QueryChangesResponse parses");
    assert_eq!(qc.account_id.as_str(), "acc1");
    let serialized_qc = serde_json::to_vec(&qc).expect("QueryChangesResponse serializes");
    assert!(serde_json::from_slice::<QueryChangesResponse>(&serialized_qc).is_ok());
}
