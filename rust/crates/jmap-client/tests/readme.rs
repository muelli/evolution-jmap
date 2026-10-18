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

#[test]
fn changelog_exists_and_documents_unreleased_breaking_changes() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let changelog_path = Path::new(manifest_dir).join("CHANGELOG.md");
    let content = fs::read_to_string(&changelog_path)
        .expect("CHANGELOG.md must exist in crate root for publication readiness");

    assert!(
        content.contains("## [Unreleased]"),
        "CHANGELOG must contain an [Unreleased] section"
    );
    assert!(
        content.contains("Client::upload_blob"),
        "CHANGELOG must document planned breaking change for Client::upload_blob"
    );
}

fn collect_rs_files(dir: &Path, acc: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, acc);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                acc.push(path);
            }
        }
    }
}

#[test]
fn no_stale_stalwart_v1_references_in_client_or_tests() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mut files = Vec::new();
    for subdir in &["src", "tests"] {
        let dir = Path::new(manifest_dir).join(subdir);
        collect_rs_files(&dir, &mut files);
    }

    let mut violations = Vec::new();
    for file in files {
        if file.ends_with("tests/readme.rs") {
            continue;
        }
        let content = fs::read_to_string(&file).expect("source file must be readable");
        for (line_idx, line) in content.lines().enumerate() {
            // Ignore the literal wire response constant containing Stalwart's hardcoded sieve capability string
            if line.contains("STALWART_UNAUTHENTICATED_SESSION") {
                continue;
            }
            if line.contains("Stalwart v1.0.0") {
                violations.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    line_idx + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Found stale 'Stalwart v1.0.0' references:\n{}",
        violations.join("\n")
    );
}
