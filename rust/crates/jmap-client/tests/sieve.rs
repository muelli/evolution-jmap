// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests JMAP Sieve support per RFC 9661.
//!
//! Covers capability detection, `SieveScript/get`, `set`, `query`, and `validate`.

use jmap_client::sieve::{Activation, SieveSetErrorKind, set_error_kind};
use jmap_client::{Client, Credentials, Error, rebase_urls_from_env};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::blob::{BlobUploadRequest, UploadBlob};
use jmap_proto::error::{SetError, method, set};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_SIEVE};
use jmap_proto::sieve::{SieveScript, SieveScriptQueryFilter, sieve_set_error};
use serde_json::json;

/// Verifies that the server advertises `urn:ietf:params:jmap:sieve` with mandatory `maxSizeScript`.
#[test]
fn sieve_capability_is_advertised_and_resolves_to_the_account() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let capability = client.session().sieve_capability();
    assert!(capability.is_some());
    assert!(capability.unwrap().max_size_script > 0);
    assert_eq!(
        client.session().resolve_primary_account(CAPABILITY_SIEVE),
        Some(&account_id)
    );
}

/// Verifies that an account initially has no Sieve scripts.
#[test]
fn sieve_script_get_returns_no_scripts_on_a_fresh_account() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let scripts = client.sieve_scripts(&account_id).unwrap();
    assert!(scripts.is_empty());
}

/// Verifies that `SieveScript/get` with an unknown identifier returns `notFound`.
#[test]
fn sieve_script_get_reports_unknown_ids_as_not_found() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let arguments = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_SIEVE],
            "SieveScript/get",
            &jmap_proto::methods::GetRequest::ids(account_id.clone(), ["nonexistent"]),
        )
        .unwrap();
    let response: jmap_proto::methods::GetResponse<jmap_proto::sieve::SieveScript> =
        serde_json::from_value(arguments).unwrap();
    assert!(response.list.is_empty());
    assert_eq!(
        response.not_found,
        vec![jmap_proto::Id::from("nonexistent")]
    );
}

/// Verifies that creating a Sieve script leaves it inactive per RFC 9661 section 2.4.
#[test]
fn sieve_script_create_stores_an_inactive_script() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let script = SieveScript::new("vacation", "B1");
    let created = client.sieve_script_create(&account_id, &script).unwrap();

    assert!(created.id.is_some());
    assert_eq!(created.name, "vacation");
    assert!(!created.is_active);

    let scripts = client.sieve_scripts(&account_id).unwrap();
    assert_eq!(scripts.len(), 1);
    assert_eq!(scripts[0].id, created.id);
}

/// Verifies that `sieve_script_create` omits server-set `isActive` from the wire create object
/// per RFC 9661 section 2.1, and `sieve_script_deactivate` omits `onSuccessActivateScript`
/// when `onSuccessDeactivateScript` is sent per RFC 9661 section 2.4.
#[test]
fn sieve_script_create_and_deactivate_emit_rfc_9661_wire_shapes() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let origin = format!("http://{addr}");

    let handle = std::thread::spawn(move || {
        let mut recorded_bodies = Vec::new();
        for step in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = rest.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            drop(reader);
            if content_length > 0 {
                recorded_bodies.push(serde_json::from_slice::<serde_json::Value>(&body).unwrap());
            }

            let response_json = match step {
                0 => json!({
                    "capabilities": {
                        "urn:ietf:params:jmap:core": {
                            "maxSizeUpload": 50000000,
                            "maxConcurrentUpload": 4,
                            "maxSizeRequest": 10000000,
                            "maxConcurrentRequests": 4,
                            "maxCallsInRequest": 16,
                            "maxObjectsInGet": 500,
                            "maxObjectsInSet": 500,
                            "collationAlgorithms": ["i;unicode-casemap"]
                        },
                        "urn:ietf:params:jmap:sieve": {}
                    },
                    "accounts": {
                        "A1": {
                            "name": "writer",
                            "isPersonal": true,
                            "isReadOnly": false,
                            "accountCapabilities": {
                                "urn:ietf:params:jmap:sieve": {
                                    "maxSizeScript": 65536
                                }
                            }
                        }
                    },
                    "primaryAccounts": {
                        "urn:ietf:params:jmap:sieve": "A1"
                    },
                    "username": "writer",
                    "apiUrl": format!("http://{addr}/jmap/"),
                    "downloadUrl": format!("http://{addr}/download/{{accountId}}/{{blobId}}/{{name}}?type={{type}}"),
                    "uploadUrl": format!("http://{addr}/upload/{{accountId}}/"),
                    "eventSourceUrl": format!("http://{addr}/eventsource/"),
                    "state": "s0"
                }),
                1 => json!({
                    "methodResponses": [[
                        "SieveScript/set",
                        {
                            "accountId": "A1",
                            "oldState": "s0",
                            "newState": "s1",
                            "created": {
                                "new": {
                                    "id": "script-cyrus-1",
                                    "isActive": false,
                                    "blobId": "B-server"
                                }
                            }
                        },
                        "c0"
                    ]],
                    "sessionState": "s0"
                }),
                _ => json!({
                    "methodResponses": [[
                        "SieveScript/set",
                        {
                            "accountId": "A1",
                            "oldState": "s1",
                            "newState": "s2"
                        },
                        "c1"
                    ]],
                    "sessionState": "s0"
                }),
            };
            let payload = serde_json::to_vec(&response_json).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            )
            .unwrap();
            stream.write_all(&payload).unwrap();
        }
        recorded_bodies
    });

    let client = Client::connect(&origin, Credentials::none()).unwrap();
    let account_id = Id::from("A1");
    let script = SieveScript::new("cyrus-filter", "B-client");
    let created = client.sieve_script_create(&account_id, &script).unwrap();
    assert_eq!(created.id, Some(Id::from("script-cyrus-1")));
    assert_eq!(
        created.name, "cyrus-filter",
        "sieve_script_create must merge caller-supplied name when server omits it in created"
    );
    assert_eq!(created.blob_id, Id::from("B-server"));
    assert!(!created.is_active);

    client.sieve_script_deactivate(&account_id).unwrap();

    let bodies = handle.join().unwrap();
    let create_obj = &bodies[0]["methodCalls"][0][1]["create"]["new"];
    assert_eq!(create_obj["name"], "cyrus-filter");
    assert_eq!(create_obj["blobId"], "B-client");
    assert!(
        create_obj.get("isActive").is_none(),
        "sieve_script_create must omit server-set isActive on create (RFC 9661 section 2.1), got {create_obj}"
    );

    let deactivate_args = &bodies[1]["methodCalls"][0][1];
    assert_eq!(deactivate_args["onSuccessDeactivateScript"], true);
    assert!(
        deactivate_args.get("onSuccessActivateScript").is_none(),
        "sieve_script_deactivate must not send both onSuccessActivateScript and onSuccessDeactivateScript (RFC 9661 section 2.4), got {deactivate_args}"
    );
}

/// Verifies that duplicate script names fail with `alreadyExists`.
#[test]
fn sieve_script_create_rejects_a_duplicate_name() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();

    match client.sieve_script_create(&account_id, &SieveScript::new("vacation", "B2")) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, set::ALREADY_EXISTS),
        other => panic!("expected Set error, got {other:?}"),
    }
}

/// Verifies that a patch update renames a script.
#[test]
fn sieve_script_update_renames() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();
    let id = created.id.unwrap();

    client
        .sieve_script_update(&account_id, &id, json!({"name": "out-of-office"}))
        .unwrap();

    let scripts = client.sieve_scripts(&account_id).unwrap();
    assert_eq!(scripts[0].name, "out-of-office");
}

/// Verifies that patching `isActive` directly fails with `invalidProperties`.
#[test]
fn sieve_script_update_rejects_a_direct_is_active_patch() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();
    let id = created.id.unwrap();

    match client.sieve_script_update(&account_id, &id, json!({"isActive": true})) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, set::INVALID_PROPERTIES),
        other => panic!("expected Set error, got {other:?}"),
    }
}

/// Verifies that destroying an active script fails with `sieveIsActive`.
#[test]
fn sieve_script_destroy_refuses_the_active_script_until_deactivated() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();
    let id = created.id.unwrap();
    client.sieve_script_activate(&account_id, &id).unwrap();

    match client.sieve_script_destroy(&account_id, &id) {
        Err(Error::Set(set_error)) => {
            assert_eq!(set_error.error_type, sieve_set_error::SIEVE_IS_ACTIVE)
        }
        other => panic!("expected Set error, got {other:?}"),
    }

    client.sieve_script_deactivate(&account_id).unwrap();
    client.sieve_script_destroy(&account_id, &id).unwrap();
    assert!(client.sieve_scripts(&account_id).unwrap().is_empty());
}

/// Verifies that activating a script deactivates the previously active script.
#[test]
fn sieve_script_activate_switches_the_previous_one_off() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let a = client
        .sieve_script_create(&account_id, &SieveScript::new("a", "B1"))
        .unwrap()
        .id
        .unwrap();
    let b = client
        .sieve_script_create(&account_id, &SieveScript::new("b", "B2"))
        .unwrap()
        .id
        .unwrap();

    client.sieve_script_activate(&account_id, &a).unwrap();
    client.sieve_script_activate(&account_id, &b).unwrap();

    let scripts = client.sieve_scripts(&account_id).unwrap();
    let active: Vec<_> = scripts
        .iter()
        .filter(|script| script.is_active)
        .map(|script| script.id.clone().unwrap())
        .collect();
    assert_eq!(active, vec![b]);
}

/// Verifies that unfiltered `SieveScript/query` returns all script identifiers.
#[test]
fn sieve_script_query_with_no_filter_returns_every_script() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let a = client
        .sieve_script_create(&account_id, &SieveScript::new("a", "B1"))
        .unwrap()
        .id
        .unwrap();
    let b = client
        .sieve_script_create(&account_id, &SieveScript::new("b", "B2"))
        .unwrap()
        .id
        .unwrap();

    let mut ids = client
        .sieve_script_query(&account_id, SieveScriptQueryFilter::new())
        .unwrap();
    ids.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(ids, expected);
}

/// Verifies that filtering by `name` matches scripts by substring.
#[test]
fn sieve_script_query_filters_by_name() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();
    let out_of_office = client
        .sieve_script_create(&account_id, &SieveScript::new("out-of-office", "B2"))
        .unwrap()
        .id
        .unwrap();

    let ids = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_name("out-of"),
        )
        .unwrap();
    assert_eq!(ids, vec![out_of_office]);
}

/// Verifies that filtering by `isActive` matches the active script.
#[test]
fn sieve_script_query_filters_by_is_active() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let a = client
        .sieve_script_create(&account_id, &SieveScript::new("a", "B1"))
        .unwrap()
        .id
        .unwrap();
    client
        .sieve_script_create(&account_id, &SieveScript::new("b", "B2"))
        .unwrap();
    client.sieve_script_activate(&account_id, &a).unwrap();

    let ids = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_is_active(true),
        )
        .unwrap();
    assert_eq!(ids, vec![a]);
}

/// Verifies `SieveScript/validate` on raw script content via temporary blob upload.
#[test]
fn sieve_script_validate_content_is_always_valid() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let response = client
        .sieve_script_validate_content(&account_id, b"not actually sieve at all".to_vec())
        .unwrap();
    assert!(response.is_valid());
    assert!(response.error.is_none());
}

/// Verifies that validating by `blobId` resolves the stored blob.
#[test]
fn sieve_script_validate_by_blob_id_resolves_a_real_blob() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let uploaded = client
        .blob_upload(&BlobUploadRequest::new(account_id.clone()).create_blob(
            "b0",
            UploadBlob::from_text("require [\"fileinto\"];", "text/plain"),
        ))
        .unwrap();
    let blob_id = uploaded.created.unwrap().get("b0").unwrap().id.clone();

    let response = client.sieve_script_validate(&account_id, blob_id).unwrap();
    assert!(response.is_valid());
}

/// Verifies that an unknown `blobId` returns `invalidArguments`.
#[test]
fn sieve_script_validate_rejects_an_unknown_blob_id() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    match client.sieve_script_validate(&account_id, Id::from("nonexistent")) {
        Err(Error::Method(method_error)) => {
            assert_eq!(method_error.error_type, method::INVALID_ARGUMENTS)
        }
        other => panic!("expected Method error, got {other:?}"),
    }
}

/// Verifies `Client::sieve_capability` for a valid account, an unknown account,
/// and merges top-level `implementation` with per-account capability fields.
#[test]
fn client_sieve_capability_returns_account_capability_and_merges_implementation() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let cap = client
        .sieve_capability(&account_id)
        .expect("mock account advertises urn:ietf:params:jmap:sieve");
    assert!(cap.max_size_script > 0);
    assert_eq!(cap.max_number_scripts, Some(32));

    assert!(
        client
            .sieve_capability(&Id::from("unknown-account"))
            .is_none(),
        "unknown account id must return None"
    );
}

/// Verifies `Client::sieve_script_put` (create and update with `Activation::Keep`,
/// `Activation::Activate`, and `Activation::Deactivate`) and `Client::sieve_script_text`
/// against `jmap-mock`.
#[test]
fn sieve_script_put_and_text_round_trip_create_update_and_activation_against_mock() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let v1_text = "require [\"fileinto\"];\r\nfileinto \"INBOX\";\r\n";
    let created = client
        .sieve_script_put(
            &account_id,
            None,
            "evolution-jmap",
            v1_text,
            Activation::Keep,
        )
        .unwrap();
    let script_id = created.id.clone().expect("created script has id");
    assert_eq!(created.name, "evolution-jmap");
    assert!(!created.is_active);
    assert_eq!(
        client.sieve_script_text(&account_id, &created).unwrap(),
        v1_text
    );

    // Update text and activate in one call.
    let v2_text = "require [\"fileinto\", \"imap4flags\"];\r\naddflag \"\\\\Seen\";\r\nfileinto \"INBOX\";\r\n";
    let updated = client
        .sieve_script_put(
            &account_id,
            Some(&script_id),
            "evolution-jmap-renamed",
            v2_text,
            Activation::Activate,
        )
        .unwrap();
    assert_eq!(updated.id.as_ref(), Some(&script_id));
    assert_eq!(updated.name, "evolution-jmap-renamed");
    assert!(updated.is_active);
    assert_eq!(
        client.sieve_script_text(&account_id, &updated).unwrap(),
        v2_text
    );

    let listed = client.sieve_scripts(&account_id).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "evolution-jmap-renamed");
    assert!(listed[0].is_active);
    assert_eq!(
        client.sieve_script_text(&account_id, &listed[0]).unwrap(),
        v2_text
    );

    // Update text without changing activation state (Activation::Keep).
    let v2b_text = "require [\"fileinto\"];\r\nfileinto \"Archive\";\r\n";
    let kept = client
        .sieve_script_put(
            &account_id,
            Some(&script_id),
            "evolution-jmap-renamed",
            v2b_text,
            Activation::Keep,
        )
        .unwrap();
    assert_eq!(kept.id.as_ref(), Some(&script_id));
    assert_eq!(
        client.sieve_script_text(&account_id, &kept).unwrap(),
        v2b_text
    );
    assert!(
        client.sieve_scripts(&account_id).unwrap()[0].is_active,
        "Activation::Keep must leave the active script active"
    );

    // Update and deactivate in one call.
    let v3_text = "keep;\r\n";
    let deactivated = client
        .sieve_script_put(
            &account_id,
            Some(&script_id),
            "evolution-jmap",
            v3_text,
            Activation::Deactivate,
        )
        .unwrap();
    assert!(!deactivated.is_active);
    assert_eq!(deactivated.name, "evolution-jmap");
    assert_eq!(
        client.sieve_script_text(&account_id, &deactivated).unwrap(),
        v3_text
    );

    let listed_after = client.sieve_scripts(&account_id).unwrap();
    assert!(!listed_after[0].is_active);

    // Create a second script directly active.
    let second = client
        .sieve_script_put(
            &account_id,
            None,
            "second-active",
            "discard;\r\n",
            Activation::Activate,
        )
        .unwrap();
    assert!(second.is_active);
    let active_ids = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_is_active(true),
        )
        .unwrap();
    assert_eq!(active_ids, vec![second.id.unwrap()]);
}

/// Verifies that `Client::sieve_script_text` rejects non-UTF-8 blob bytes as an error
/// rather than silently replacing them.
#[test]
fn sieve_script_text_rejects_non_utf8_blob_bytes() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let uploaded = client
        .upload_blob(
            &account_id,
            "application/sieve",
            vec![0xff, 0xfe, b'k', b'e', b'e', b'p', b';'],
        )
        .unwrap();
    let created = client
        .sieve_script_create(
            &account_id,
            &SieveScript::new("invalid-utf8", uploaded.blob_id),
        )
        .unwrap();

    let err = client
        .sieve_script_text(&account_id, &created)
        .expect_err("non-UTF-8 script bytes must return an error");
    assert!(
        matches!(err, Error::Protocol(ref msg) if msg.contains("UTF-8")),
        "expected Error::Protocol mentioning UTF-8, got {err:?}"
    );

    let empty_blob_err = client
        .sieve_script_text(&account_id, &SieveScript::default())
        .expect_err("empty blobId must return an error");
    assert!(
        matches!(empty_blob_err, Error::Protocol(ref msg) if msg.contains("blobId")),
        "expected Error::Protocol mentioning blobId, got {empty_blob_err:?}"
    );
}

/// Verifies `jmap_client::sieve::set_error_kind` across RFC 9661 and server-variant error types,
/// and checks that `SetError` description stays reachable through `Error::Set`.
#[test]
fn sieve_set_error_kind_maps_standard_and_server_variant_codes_with_description() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let s1 = client
        .sieve_script_put(
            &account_id,
            None,
            "dup-test",
            "keep;\r\n",
            Activation::Activate,
        )
        .unwrap();
    let s1_id = s1.id.unwrap();

    // Duplicate name -> AlreadyExists
    match client.sieve_script_put(&account_id, None, "dup-test", "keep;\r\n", Activation::Keep) {
        Err(Error::Set(err)) => {
            assert_eq!(set_error_kind(&err), Some(SieveSetErrorKind::AlreadyExists));
        }
        other => panic!("expected Error::Set(alreadyExists), got {other:?}"),
    }

    // Destroy active script -> SieveIsActive
    match client.sieve_script_destroy(&account_id, &s1_id) {
        Err(Error::Set(err)) => {
            assert_eq!(set_error_kind(&err), Some(SieveSetErrorKind::SieveIsActive));
        }
        other => panic!("expected Error::Set(sieveIsActive), got {other:?}"),
    }

    // Check all five kinds and server-specific aliases (`invalidScript`, `scriptIsActive`),
    // ensuring `description` remains accessible on `SetError`.
    let invalid_sieve = SetError::new("invalidSieve").with_description("line 1: syntax error");
    assert_eq!(
        set_error_kind(&invalid_sieve),
        Some(SieveSetErrorKind::InvalidSieve)
    );
    assert_eq!(
        invalid_sieve.description.as_deref(),
        Some("line 1: syntax error")
    );

    let invalid_script = SetError::new("invalidScript")
        .with_description("Expected token \"command\" at line 1, column 1.");
    assert_eq!(
        set_error_kind(&invalid_script),
        Some(SieveSetErrorKind::InvalidSieve)
    );

    let sieve_active = SetError::new("sieveIsActive").with_description("active script");
    assert_eq!(
        set_error_kind(&sieve_active),
        Some(SieveSetErrorKind::SieveIsActive)
    );

    let script_active = SetError::new("scriptIsActive")
        .with_description("Deactivate Sieve script before deletion.");
    assert_eq!(
        set_error_kind(&script_active),
        Some(SieveSetErrorKind::SieveIsActive)
    );

    let too_large = SetError::new("tooLarge")
        .with_description("Sieve script is too large at line 0, column 0.");
    assert_eq!(
        set_error_kind(&too_large),
        Some(SieveSetErrorKind::TooLarge)
    );

    let over_quota = SetError::new("overQuota")
        .with_description("There are too many sieve scripts, please delete some.");
    assert_eq!(
        set_error_kind(&over_quota),
        Some(SieveSetErrorKind::OverQuota)
    );

    let already_exists = SetError::new("alreadyExists")
        .with_description("A sieve script with name 'x' already exists.");
    assert_eq!(
        set_error_kind(&already_exists),
        Some(SieveSetErrorKind::AlreadyExists)
    );

    let other = SetError::new("forbidden").with_description("reserved");
    assert_eq!(set_error_kind(&other), None);
}

fn connect_live_write() -> Option<Client> {
    let url = std::env::var("JMAP_LIVE_SERVER_URL").ok()?;
    let user = std::env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let pass = std::env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD").ok()?;
    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase_urls_from_env())
            .connect(&url, Credentials::basic(user, pass))
            .expect("failed to connect to live JMAP server"),
    )
}

fn exercise_sieve_helpers_on_live_server(expected_server: &str) {
    let Some(client) = connect_live_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping live Sieve test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_SIEVE)
        .expect("live account must advertise urn:ietf:params:jmap:sieve");
    let cap = client
        .sieve_capability(&account_id)
        .expect("Client::sieve_capability must return Some for the Sieve account");
    let implementation = cap.implementation.as_deref().unwrap_or("");
    if !implementation.contains(expected_server) {
        eprintln!(
            "connected server implementation is {implementation:?}, not {expected_server:?}; skipping"
        );
        return;
    }
    assert!(cap.max_size_script > 0);
    assert!(
        cap.sieve_extensions.iter().any(|ext| ext == "fileinto"),
        "expected fileinto in sieveExtensions, got {:?}",
        cap.sieve_extensions
    );

    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name_1 = format!("agent-sieve-put-{suffix}-1");
    let name_2 = format!("agent-sieve-put-{suffix}-2");
    let text_1 = "require [\"fileinto\"];\r\nfileinto \"INBOX\";\r\n";
    let text_2 = "require [\"fileinto\", \"imap4flags\"];\r\naddflag \"\\\\Seen\";\r\nfileinto \"INBOX\";\r\n";

    // Create inactive via sieve_script_put + Activation::Keep, then read back text.
    let created = client
        .sieve_script_put(&account_id, None, &name_1, text_1, Activation::Keep)
        .expect("sieve_script_put create failed");
    let script_id = created.id.clone().expect("server assigned script id");
    assert_eq!(created.name, name_1);
    assert!(!created.is_active);
    assert_eq!(
        client
            .sieve_script_text(&account_id, &created)
            .expect("sieve_script_text after create failed"),
        text_1
    );

    // Duplicate name must map to SieveSetErrorKind::AlreadyExists.
    match client.sieve_script_put(&account_id, None, &name_1, text_1, Activation::Keep) {
        Err(Error::Set(err)) => {
            assert_eq!(set_error_kind(&err), Some(SieveSetErrorKind::AlreadyExists));
        }
        other => panic!("expected AlreadyExists SetError on duplicate name, got {other:?}"),
    }

    // Update text, rename, and activate in one call via Activation::Activate.
    let updated = client
        .sieve_script_put(
            &account_id,
            Some(&script_id),
            &name_2,
            text_2,
            Activation::Activate,
        )
        .expect("sieve_script_put update + Activate failed");
    assert_eq!(updated.id.as_ref(), Some(&script_id));
    assert_eq!(updated.name, name_2);
    assert!(updated.is_active);
    assert_eq!(
        client
            .sieve_script_text(&account_id, &updated)
            .expect("sieve_script_text after update failed"),
        text_2
    );

    // Destroying active script must map to SieveSetErrorKind::SieveIsActive.
    match client.sieve_script_destroy(&account_id, &script_id) {
        Err(Error::Set(err)) => {
            assert_eq!(set_error_kind(&err), Some(SieveSetErrorKind::SieveIsActive));
        }
        other => {
            panic!("expected SieveIsActive SetError when destroying active script, got {other:?}")
        }
    }

    // Invalid Sieve syntax must map to SieveSetErrorKind::InvalidSieve with description.
    match client.sieve_script_put(
        &account_id,
        Some(&script_id),
        &name_2,
        "bogus_cmd;\r\n",
        Activation::Keep,
    ) {
        Err(Error::Set(err)) => {
            assert_eq!(set_error_kind(&err), Some(SieveSetErrorKind::InvalidSieve));
            assert!(
                err.description.as_deref().is_some_and(|d| !d.is_empty()),
                "InvalidSieve error must preserve server description, got {err:?}"
            );
        }
        other => panic!("expected InvalidSieve SetError on invalid syntax, got {other:?}"),
    }

    // Deactivate via sieve_script_put + Activation::Deactivate, then destroy.
    let deactivated = client
        .sieve_script_put(
            &account_id,
            Some(&script_id),
            &name_2,
            text_1,
            Activation::Deactivate,
        )
        .expect("sieve_script_put update + Deactivate failed");
    assert!(!deactivated.is_active);

    client
        .sieve_script_destroy(&account_id, &script_id)
        .expect("cleanup sieve_script_destroy failed");
}

/// Live test exercising `sieve_capability`, `sieve_script_put`, `sieve_script_text`,
/// and `set_error_kind` against Stalwart (`stalwart-runner`).
#[test]
#[ignore = "needs a live Stalwart JMAP server; see docs/manual-test-live-server.md"]
fn sieve_helpers_round_trip_against_live_stalwart() {
    exercise_sieve_helpers_on_live_server("Stalwart");
}

/// Live test exercising `sieve_capability`, `sieve_script_put`, `sieve_script_text`,
/// and `set_error_kind` against Cyrus IMAP (`ci/cyrus-oracle.sh`).
#[test]
#[ignore = "needs a live Cyrus JMAP server; see docs/manual-test-live-server.md"]
fn sieve_helpers_round_trip_against_live_cyrus() {
    exercise_sieve_helpers_on_live_server("Cyrus");
}
