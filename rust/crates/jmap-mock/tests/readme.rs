// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs;
use std::path::Path;

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::mail::Mailbox;
use jmap_proto::session::CAPABILITY_MAIL;

#[test]
fn readme_exists_and_covers_release_topics() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let readme_path = Path::new(manifest_dir).join("README.md");
    let content = fs::read_to_string(&readme_path)
        .expect("README.md must exist in crate root for publication readiness");

    assert!(
        content.contains("# evolution-jmap-mock"),
        "README must identify the crate"
    );
    assert!(
        content.contains("## What It Is"),
        "README must explain what the crate provides"
    );
    assert!(
        content.contains("## Modes"),
        "README must summarize server modes"
    );
    assert!(
        content.contains("## jmap-mockd from a shell"),
        "README must show shell usage"
    );
    assert!(
        content.contains("## jmap-mockd from Python"),
        "README must show Python usage"
    );
    assert!(
        content.contains("## Example: test a client against the mock server"),
        "README must include a client test example"
    );
}

#[test]
fn changelog_exists_and_has_unreleased() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let changelog_path = Path::new(manifest_dir).join("CHANGELOG.md");
    let content = fs::read_to_string(&changelog_path)
        .expect("CHANGELOG.md must exist in crate root for publication readiness");

    assert!(
        content.contains("## [Unreleased]"),
        "CHANGELOG must contain an [Unreleased] section"
    );
}

#[test]
fn example_client_can_create_and_fetch_a_mailbox() {
    let server = MockServer::builder().start();

    let client = Client::connect(server.origin(), Credentials::none())
        .expect("client should connect to the in-memory mock server");
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("mock session should include a primary mail account");

    client
        .mailbox_create(&account_id, &Mailbox::new("Client example mailbox"))
        .expect("Mailbox/set create should succeed against the mock server");

    let all_mailboxes = client
        .mailbox_get(&account_id)
        .expect("Mailbox/get should succeed against the mock server");
    assert!(
        all_mailboxes
            .list
            .iter()
            .any(|mailbox| mailbox.name == "Client example mailbox"),
        "created mailbox should be visible through Mailbox/get"
    );
}
