// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verification of publication readiness documentation and examples for `jmap-proto`.

use std::fs;
use std::path::Path;

use jmap_proto::id::Id;
use jmap_proto::request::Request;
use serde_json::json;

#[test]
fn readme_exists_and_contains_required_sections() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let readme_path = Path::new(manifest_dir).join("README.md");
    let content = fs::read_to_string(&readme_path)
        .expect("README.md must exist in crate root for publication readiness");

    assert!(
        content.contains("# evolution-jmap-proto") || content.contains("# jmap-proto"),
        "README must identify the crate"
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
fn minimal_working_example_roundtrips() {
    let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
        .call(
            "Core/echo",
            &json!({
                "message": "hello jmap"
            }),
            "c0",
        )
        .expect("call builder should succeed");

    let serialized = serde_json::to_string_pretty(&request)
        .expect("serialization of minimal example request must succeed");

    let deserialized: Request = serde_json::from_str(&serialized)
        .expect("deserialization of minimal example request must succeed");

    assert_eq!(deserialized.using.len(), 2);
    assert_eq!(deserialized.method_calls.len(), 1);
    let inv = &deserialized.method_calls[0];
    assert_eq!(inv.name, "Core/echo");
    assert_eq!(inv.call_id, "c0");
    assert_eq!(inv.arguments["message"], "hello jmap");

    let account_id = Id::from("acc123");
    assert_eq!(account_id.as_str(), "acc123");
}
