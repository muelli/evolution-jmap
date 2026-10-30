// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify `jmap-mockd` Sieve mode flags.

use serde_json::{Value, json};

mod common;

use common::Mockd;

const ACCOUNT_ID: &str = "A1";
const CAP_CORE: &str = "urn:ietf:params:jmap:core";
const CAP_SIEVE: &str = "urn:ietf:params:jmap:sieve";
const CAP_VACATION: &str = "urn:ietf:params:jmap:vacationresponse";

fn api(mockd: &Mockd, body: Value) -> Value {
    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a response JSON body")
}

fn session(mockd: &Mockd) -> Value {
    let (status, _, body) = mockd.request("GET", "/.well-known/jmap", b"");
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a session JSON body")
}

#[test]
fn no_sieve_flag_omits_sieve_capability() {
    let mockd = Mockd::spawn(&["--no-sieve"]);
    let session = session(&mockd);

    assert!(session["capabilities"].get(CAP_SIEVE).is_none());
    assert!(
        session["accounts"][ACCOUNT_ID]["accountCapabilities"]
            .get(CAP_SIEVE)
            .is_none()
    );
}

#[test]
fn sieve_extensions_flag_changes_advertised_extensions() {
    let mockd = Mockd::spawn(&["--sieve-extensions", "fileinto,copy"]);
    let session = session(&mockd);

    assert_eq!(
        session["capabilities"][CAP_SIEVE]["sieveExtensions"],
        json!(["fileinto", "copy"])
    );
}

#[test]
fn sieve_vacation_script_flag_protects_the_vacation_script() {
    let mockd = Mockd::spawn(&["--sieve-vacation-script"]);

    let vacation_set = api(
        &mockd,
        json!({
            "using": [CAP_CORE, CAP_VACATION],
            "methodCalls": [[
                "VacationResponse/set",
                {
                    "accountId": ACCOUNT_ID,
                    "update": {
                        "singleton": {
                            "isEnabled": true,
                            "subject": "Out of office",
                            "textBody": "Back tomorrow",
                        }
                    }
                },
                "v1"
            ]],
        }),
    );
    assert!(vacation_set["methodResponses"][0][1]["updated"]["singleton"].is_null());

    let sieve_get = api(
        &mockd,
        json!({
            "using": [CAP_CORE, CAP_SIEVE],
            "methodCalls": [[
                "SieveScript/get",
                {
                    "accountId": ACCOUNT_ID,
                    "properties": ["id", "name", "isActive"],
                },
                "s1"
            ]],
        }),
    );

    let list = sieve_get["methodResponses"][0][1]["list"]
        .as_array()
        .expect("sieve list");
    let vacation_script = list
        .iter()
        .find(|entry| entry["name"] == "vacation")
        .expect("vacation script entry exists");
    assert_eq!(vacation_script["isActive"], json!(true));

    let script_id = vacation_script["id"]
        .as_str()
        .expect("vacation script id")
        .to_owned();

    let destroy = api(
        &mockd,
        json!({
            "using": [CAP_CORE, CAP_SIEVE],
            "methodCalls": [[
                "SieveScript/set",
                {
                    "accountId": ACCOUNT_ID,
                    "destroy": [script_id],
                },
                "s2"
            ]],
        }),
    );

    assert_eq!(
        destroy["methodResponses"][0][1]["notDestroyed"][script_id.as_str()]["type"],
        json!("forbidden")
    );
}
