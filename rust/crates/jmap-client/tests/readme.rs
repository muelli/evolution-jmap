// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verification of publication readiness documentation and examples for `jmap-client`.

use std::fs;
use std::path::Path;
use std::time::Duration;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials};

struct MockTransport;

impl Transport for MockTransport {
    fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_string()),
            body: br#"{
                "capabilities": {
                    "urn:ietf:params:jmap:core": {
                        "maxSizeUpload": 50000000,
                        "maxConcurrentUpload": 4,
                        "maxSizeRequest": 10000000,
                        "maxConcurrentRequests": 4,
                        "maxCallsInRequest": 16,
                        "maxObjectsInGet": 500,
                        "maxObjectsInSet": 500,
                        "collationAlgorithms": ["i;ascii-numeric", "i;ascii-casemap", "i;octet"]
                    }
                },
                "accounts": {
                    "acc1": {
                        "name": "user@example.com",
                        "isPersonal": true,
                        "isReadOnly": false,
                        "accountCapabilities": {
                            "urn:ietf:params:jmap:core": {}
                        }
                    }
                },
                "primaryAccounts": {
                    "urn:ietf:params:jmap:core": "acc1"
                },
                "username": "user@example.com",
                "apiUrl": "https://example.com/api",
                "downloadUrl": "https://example.com/download/{blobId}",
                "uploadUrl": "https://example.com/upload",
                "eventSourceUrl": "https://example.com/events",
                "state": "init-state"
            }"#
            .to_vec(),
            final_url: req.url.to_string(),
        })
    }
}

#[test]
fn readme_exists_and_contains_required_sections() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let readme_path = Path::new(manifest_dir).join("README.md");
    let content = fs::read_to_string(&readme_path)
        .expect("README.md must exist in crate root for publication readiness");

    assert!(
        content.contains("# evolution-jmap-client") || content.contains("# jmap-client"),
        "README must identify the crate"
    );
    assert!(
        content.contains("## Minimal Working Example") || content.contains("## Example"),
        "README must provide a minimal working example"
    );
    assert!(
        content.contains("## Common Workflows"),
        "README must document common workflows"
    );
    assert!(
        content.contains("## Feature flags"),
        "README must document feature flags"
    );
    assert!(
        content.contains("## Minimum Supported Rust Version") || content.contains("## MSRV"),
        "README must document MSRV"
    );
    assert!(
        content.contains("## License"),
        "README must document licensing"
    );
    assert!(
        content.contains("## Scope") || content.contains("Scope statement"),
        "README must document scope statement ('do whatever Stalwart does')"
    );
    assert!(
        content.contains("What is not implemented") || content.contains("what is not implemented"),
        "README must document what is not implemented"
    );
    assert!(
        content.contains("## Documentation gaps") || content.contains("TODO"),
        "README must document missing documentation status"
    );
}

#[test]
fn minimal_working_example_executes() {
    let client = Client::builder()
        .transport(MockTransport)
        .timeout(Duration::from_secs(10))
        .connect("https://example.com", Credentials::bearer("secret-token"))
        .expect("client connection with mock transport should succeed");

    assert!(!client.is_anonymous());
    assert_eq!(client.session().username, "user@example.com");
    assert_eq!(
        client
            .primary_account("urn:ietf:params:jmap:core")
            .expect("primary account")
            .as_str(),
        "acc1"
    );
}
