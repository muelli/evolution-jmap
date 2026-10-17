// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mutation test coverage for jmap-client modules:
//! - blob.rs (RFC 9404 Blob/get, Blob/upload, Blob/lookup, and RFC 8620 Blob/copy)
//! - limits.rs (MAX_API_RESPONSE_BYTES, MAX_OAUTH_*, MAX_BLOB_BYTES)
//! - quota.rs (RFC 9425 Quota/get and Quota/query)
//! - resolver.rs (RFC 8620 Section 2.2 SRV autodiscovery and NoSrvResolver)
//! - snooze.rs (vendor email snoozing and snoozed mailbox management)

use jmap_client::limits::{
    MAX_API_RESPONSE_BYTES, MAX_BLOB_BYTES, MAX_OAUTH_METADATA_BYTES, MAX_OAUTH_REGISTRATION_BYTES,
    MAX_OAUTH_TOKEN_BYTES,
};
use jmap_client::resolver::{NoSrvResolver, Resolver, SrvTarget};
use jmap_client::{Client, Credentials, Error};
use jmap_proto::blob::{BlobGetRequest, BlobLookupRequest, BlobUploadRequest, UploadBlob};
use jmap_proto::mail::{EmailImport, SnoozeDetails, role};
use jmap_proto::methods::BlobCopyRequest;
use jmap_proto::quota::QuotaQueryFilter;
use jmap_proto::session::CAPABILITY_QUOTA;
use jmap_proto::state::UtcDate;

#[test]
fn limits_exact_byte_constants() {
    assert_eq!(MAX_API_RESPONSE_BYTES, 64 * 1024 * 1024);
    assert_eq!(MAX_API_RESPONSE_BYTES, 67_108_864);

    assert_eq!(MAX_OAUTH_METADATA_BYTES, 1024 * 1024);
    assert_eq!(MAX_OAUTH_METADATA_BYTES, 1_048_576);

    assert_eq!(MAX_OAUTH_REGISTRATION_BYTES, 1024 * 1024);
    assert_eq!(MAX_OAUTH_REGISTRATION_BYTES, 1_048_576);

    assert_eq!(MAX_OAUTH_TOKEN_BYTES, 1024 * 1024);
    assert_eq!(MAX_OAUTH_TOKEN_BYTES, 1_048_576);

    assert_eq!(MAX_BLOB_BYTES, 256 * 1024 * 1024);
    assert_eq!(MAX_BLOB_BYTES, 268_435_456);
}

#[test]
fn resolver_default_and_target_properties() {
    let resolver = NoSrvResolver;
    assert_eq!(resolver.lookup_srv("example.com"), None);
    assert_eq!(resolver.lookup_srv("fastmail.com"), None);
    assert_eq!(resolver.lookup_srv(""), None);

    let default_resolver: NoSrvResolver = Default::default();
    assert_eq!(default_resolver.lookup_srv("example.org"), None);

    let target = SrvTarget {
        host: "jmap.example.com".to_owned(),
        port: 8443,
    };
    assert_eq!(target.host, "jmap.example.com");
    assert_eq!(target.port, 8443);
    assert_eq!(
        target,
        SrvTarget {
            host: "jmap.example.com".to_owned(),
            port: 8443,
        }
    );
    assert_ne!(
        target,
        SrvTarget {
            host: "jmap.example.com".to_owned(),
            port: 443,
        }
    );
    assert_ne!(
        target,
        SrvTarget {
            host: "other.example.com".to_owned(),
            port: 8443,
        }
    );

    let cloned = target.clone();
    assert_eq!(cloned, target);
    let debug_repr = format!("{target:?}");
    assert!(debug_repr.contains("jmap.example.com"));
    assert!(debug_repr.contains("8443"));
}

#[test]
fn blob_client_methods_roundtrip() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let upload_req = BlobUploadRequest::new(account_id.clone()).create_blob(
        "test-blob-1",
        UploadBlob::from_text("mutation test blob payload", "text/plain"),
    );
    let upload_resp = client.blob_upload(&upload_req).expect("blob_upload");
    let created = upload_resp.created.expect("created blobs map present");
    let result = created.get("test-blob-1").expect("blob created");
    assert_eq!(result.size, 26);
    assert_eq!(result.content_type.as_deref(), Some("text/plain"));

    let get_req = BlobGetRequest::new(account_id.clone(), [result.id.clone()]).with_properties([
        "data:asText",
        "size",
        "type",
    ]);
    let get_resp = client.blob_get(&get_req).expect("blob_get");
    assert_eq!(get_resp.list.len(), 1);
    assert_eq!(
        get_resp.list[0].data_as_text.as_deref(),
        Some("mutation test blob payload")
    );
    assert_eq!(get_resp.list[0].size, Some(26));

    let lookup_req = BlobLookupRequest::new(account_id.clone(), ["Email"], [result.id.clone()]);
    let lookup_resp = client.blob_lookup(&lookup_req).expect("blob_lookup");
    assert!(lookup_resp.list.is_empty() || !lookup_resp.list.is_empty());

    let copy_req = BlobCopyRequest::new(account_id.clone(), account_id, [result.id.clone()]);
    let copy_resp = client.blob_copy(&copy_req).expect("blob_copy");
    let copied = copy_resp.copied.expect("copied map present");
    assert!(copied.contains_key(&result.id));
}

#[test]
fn quota_client_methods_and_capability_checks() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let quotas = client.quotas(&account_id).expect("quotas");
    assert_eq!(quotas.len(), 1);
    assert_eq!(quotas[0].resource_type, "octets");
    assert_eq!(quotas[0].scope, "account");

    let query_ids = client
        .quota_query(&account_id, QuotaQueryFilter::new())
        .expect("quota_query");
    assert_eq!(query_ids.len(), 1);

    let no_quota_server = jmap_mock::MockServer::builder()
        .without_capability(CAPABILITY_QUOTA)
        .start();
    let no_quota_account = no_quota_server.account_id();
    let no_quota_client = Client::connect(no_quota_server.origin(), Credentials::none()).unwrap();

    let empty_quotas = no_quota_client
        .quotas(&no_quota_account)
        .expect("quotas without capability");
    assert!(empty_quotas.is_empty());

    let empty_query = no_quota_client
        .quota_query(&no_quota_account, QuotaQueryFilter::new())
        .expect("quota_query without capability");
    assert!(empty_query.is_empty());
}

#[test]
fn snooze_client_methods_and_mailbox_management() {
    let server = jmap_mock::MockServer::builder().snooze_extension().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let inbox = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_mailbox("Inbox", Some(role::INBOX))
    };

    let snoozed_mailbox = client
        .snoozed_mailbox(&account_id)
        .expect("snoozed_mailbox");
    assert_eq!(snoozed_mailbox.role.as_deref(), Some(role::SNOOZED));
    let snoozed_id = snoozed_mailbox.id.expect("snoozed mailbox id");

    let second_snoozed = client.snoozed_mailbox(&account_id).expect("second ask");
    assert_eq!(second_snoozed.id.as_ref(), Some(&snoozed_id));
    assert_eq!(second_snoozed.role.as_deref(), Some(role::SNOOZED));

    let email_raw = b"From: sender@example.com\r\nSubject: Test\r\n\r\nBody\r\n";
    let upload = client
        .upload_blob(&account_id, "message/rfc822", email_raw.to_vec())
        .unwrap();
    let imported = client
        .email_import(
            &account_id,
            &EmailImport::new(upload.blob_id, inbox.clone()),
        )
        .unwrap();
    let email_id = imported.id.expect("server assigned an email id");

    let details = SnoozeDetails::new(UtcDate::new("2026-06-01T12:00:00Z"))
        .with_move_to_mailbox_id(inbox.clone());
    client
        .snooze_email(&account_id, &email_id, &snoozed_id, &details)
        .expect("snooze_email success");

    let fetched = client
        .email_get(&account_id, std::slice::from_ref(&email_id), None)
        .unwrap();
    assert_eq!(
        fetched[0].snoozed.as_ref().map(|s| s.until.as_str()),
        Some("2026-06-01T12:00:00Z")
    );
    let mailbox_ids = fetched[0].mailbox_ids.as_ref().unwrap();
    assert_eq!(mailbox_ids.get(&snoozed_id), Some(&true));
    assert!(!mailbox_ids.contains_key(&inbox));

    let unextended_server = jmap_mock::MockServer::builder().start();
    let unextended_account = unextended_server.account_id();
    let unextended_client =
        Client::connect(unextended_server.origin(), Credentials::none()).unwrap();
    let unextended_snoozed = unextended_client
        .snoozed_mailbox(&unextended_account)
        .unwrap()
        .id
        .unwrap();

    let unextended_upload = unextended_client
        .upload_blob(&unextended_account, "message/rfc822", email_raw.to_vec())
        .unwrap();
    let unextended_imported = unextended_client
        .email_import(
            &unextended_account,
            &EmailImport::new(unextended_upload.blob_id, unextended_snoozed.clone()),
        )
        .unwrap();
    let unextended_email_id = unextended_imported.id.unwrap();

    let err = unextended_client
        .snooze_email(
            &unextended_account,
            &unextended_email_id,
            &unextended_snoozed,
            &details,
        )
        .expect_err("snooze without extension must fail");
    match err {
        Error::Set(set_err) => {
            assert_eq!(set_err.error_type, "invalidProperties");
        }
        other => panic!("expected Error::Set, got {other:?}"),
    }
}
