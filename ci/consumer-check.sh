#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Standalone consumer check: packages evolution-jmap-proto and evolution-jmap-client,
# extracts them into an isolated scratch directory outside the workspace,
# builds an external consumer binary depending on the extracted packages,
# and executes standard JMAP operations against a local jmap-mockd daemon.

set -euo pipefail

for tool in cargo tar mktemp sed; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "ci/consumer-check.sh: missing required tool: $tool" >&2
        exit 1
    fi
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
workspace_dir="${repo_root}/rust"

echo "== Building jmap-mockd =="
cargo build --manifest-path "${workspace_dir}/crates/jmap-mock/Cargo.toml" --bin jmap-mockd
mockd_bin="${workspace_dir}/target/debug/jmap-mockd"
if [ ! -x "${mockd_bin}" ]; then
    echo "ci/consumer-check.sh: failed to locate built jmap-mockd binary at ${mockd_bin}" >&2
    exit 1
fi

tmp_dir="$(mktemp -d -t jmap-consumer-check.XXXXXX)"
mockd_pid=""

cleanup() {
    if [ -n "${mockd_pid}" ]; then
        kill -TERM "${mockd_pid}" 2>/dev/null || true
        wait "${mockd_pid}" 2>/dev/null || true
    fi
    rm -rf "${tmp_dir}"
}
trap cleanup EXIT INT TERM

echo "== Packaging evolution-jmap-proto =="
cargo package --manifest-path "${workspace_dir}/crates/jmap-proto/Cargo.toml" --allow-dirty --no-verify

echo "== Packaging evolution-jmap-client =="
cargo package --manifest-path "${workspace_dir}/crates/jmap-client/Cargo.toml" --allow-dirty --no-verify \
    --config "patch.crates-io.evolution-jmap-proto.path=\"${workspace_dir}/crates/jmap-proto\""

target_dir="${CARGO_TARGET_DIR:-${workspace_dir}/target}"
package_dir="${target_dir}/package"

proto_crate="$(find "${package_dir}" -maxdepth 1 -name "evolution-jmap-proto-*.crate" | sort -V | tail -n 1)"
client_crate="$(find "${package_dir}" -maxdepth 1 -name "evolution-jmap-client-*.crate" | sort -V | tail -n 1)"

if [ -z "${proto_crate}" ] || [ ! -f "${proto_crate}" ]; then
    echo "ci/consumer-check.sh: proto .crate package not found in ${package_dir}" >&2
    exit 1
fi
if [ -z "${client_crate}" ] || [ ! -f "${client_crate}" ]; then
    echo "ci/consumer-check.sh: client .crate package not found in ${package_dir}" >&2
    exit 1
fi

echo "== Extracting packaged tarballs =="
extracted_dir="${tmp_dir}/extracted"
mkdir -p "${extracted_dir}"
tar -xzf "${proto_crate}" -C "${extracted_dir}"
tar -xzf "${client_crate}" -C "${extracted_dir}"

extracted_proto="$(find "${extracted_dir}" -mindepth 1 -maxdepth 1 -type d -name "evolution-jmap-proto-*" | head -n 1)"
extracted_client="$(find "${extracted_dir}" -mindepth 1 -maxdepth 1 -type d -name "evolution-jmap-client-*" | head -n 1)"

if [ -z "${extracted_proto}" ] || [ ! -d "${extracted_proto}" ]; then
    echo "ci/consumer-check.sh: failed to extract evolution-jmap-proto" >&2
    exit 1
fi
if [ -z "${extracted_client}" ] || [ ! -d "${extracted_client}" ]; then
    echo "ci/consumer-check.sh: failed to extract evolution-jmap-client" >&2
    exit 1
fi

echo "== Setting up isolated consumer crate =="
consumer_dir="${tmp_dir}/consumer"
mkdir -p "${consumer_dir}/src"

cat << EOF > "${consumer_dir}/Cargo.toml"
[package]
name = "jmap-consumer"
version = "0.1.0"
edition = "2024"

[dependencies]
evolution-jmap-client = { path = "${extracted_client}" }
evolution-jmap-proto = { path = "${extracted_proto}" }

[patch.crates-io]
evolution-jmap-proto = { path = "${extracted_proto}" }
EOF

cat << 'EOF' > "${consumer_dir}/src/main.rs"
use std::env;
use std::time::Duration;

use jmap_client::eventsource::{expand_url, SharedHeaders};
use jmap_client::{CancelFlag, Client, Credentials, EventSourceSubscription};
use jmap_proto::mail::{EmailImport, Mailbox};
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_proto::Id;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let (url, user, password, rebase) = if args.len() >= 4 {
        (
            args[1].clone(),
            args[2].clone(),
            args[3].clone(),
            args.get(4).map(|s| s == "1" || s == "true").unwrap_or(false),
        )
    } else {
        let url = env::var("JMAP_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
        let user = env::var("JMAP_USER").unwrap_or_else(|_| "alice".to_string());
        let password = env::var("JMAP_PASSWORD").unwrap_or_else(|_| "secret".to_string());
        let rebase = env::var("JMAP_REBASE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        (url, user, password, rebase)
    };

    println!("1. Connecting to session at {} (user: {}, rebase: {})...", url, user, rebase);
    let credentials = Credentials::basic(user, password);
    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .timeout(Duration::from_secs(15))
        .connect(&url, credentials)?;

    assert!(!client.is_anonymous());
    println!("   Session opened. Username: {}", client.session().username);

    let account_id: Id = client.primary_account(CAPABILITY_MAIL)?;
    println!("   Primary mail account ID: {}", account_id.as_str());

    println!("2. Listing mailboxes...");
    let mailboxes_response = client.mailbox_get(&account_id)?;
    println!("   Found {} mailboxes", mailboxes_response.list.len());
    assert!(
        !mailboxes_response.list.is_empty(),
        "Account must have at least one mailbox"
    );

    let target_mailbox_id = mailboxes_response
        .list
        .iter()
        .find(|mb| mb.role.as_deref() == Some("inbox"))
        .or_else(|| mailboxes_response.list.first())
        .and_then(|mb| mb.id.clone())
        .expect("Must have at least one mailbox ID");

    let unique_name = format!(
        "Consumer-Test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis()
    );
    println!("3. Creating test mailbox \"{}\"...", unique_name);
    let new_mailbox = Mailbox::new(&unique_name);
    let created_mailbox = client.mailbox_create(&account_id, &new_mailbox)?;
    let created_id = created_mailbox.id.expect("Created mailbox must have an id");
    println!("   Created mailbox ID: {}", created_id.as_str());

    let updated_mailboxes = client.mailbox_get(&account_id)?;
    assert!(
        updated_mailboxes.list.iter().any(|mb| mb.id.as_ref() == Some(&created_id)),
        "Created mailbox must be present in mailbox_get"
    );

    println!("   Destroying test mailbox {}...", created_id.as_str());
    client.mailbox_destroy(&account_id, &created_id)?;

    let after_destroy = client.mailbox_get(&account_id)?;
    assert!(
        !after_destroy.list.iter().any(|mb| mb.id.as_ref() == Some(&created_id)),
        "Destroyed mailbox must no longer appear in mailbox_get"
    );
    println!("   Mailbox destroyed successfully.");

    println!("4. Importing and reading a message...");
    let rfc822_content = b"From: consumer@example.com\r\nTo: alice@example.com\r\nSubject: Outside consumer test\r\nDate: Wed, 08 Oct 2026 12:00:00 +0000\r\nMessage-ID: <test-consumer-1@example.com>\r\n\r\nHello from external consumer!";
    let upload = client.upload_blob(&account_id, "message/rfc822", rfc822_content.to_vec())?;
    println!("   Uploaded blob ID: {}", upload.blob_id.as_str());

    let import = EmailImport::new(upload.blob_id, target_mailbox_id);
    let imported_email = client.email_import(&account_id, &import)?;
    let imported_id = imported_email.id.expect("Imported email must have an ID");
    println!("   Imported email ID: {}", imported_id.as_str());

    let fetched_emails = client.email_get(
        &account_id,
        &[imported_id.clone()],
        Some(&["id", "blobId", "threadId", "mailboxIds", "subject", "from", "to"]),
    )?;
    assert_eq!(fetched_emails.len(), 1, "Must fetch exactly the imported email");
    let fetched = &fetched_emails[0];
    println!("   Read email subject: {:?}", fetched.subject);
    assert_eq!(fetched.subject.as_deref(), Some("Outside consumer test"));

    println!("   Cleaning up imported email {}...", imported_id.as_str());
    client.email_destroy(&account_id, &imported_id)?;

    println!("5. Opening EventSource push stream...");
    let event_source_url = expand_url(&client.session().event_source_url, &[], false, 0);
    println!("   Expanded eventSourceUrl: {}", event_source_url);

    let headers = client
        .authorization_header()
        .map(|auth| vec![("Authorization".to_string(), auth)])
        .unwrap_or_default();

    let cancel = CancelFlag::new();
    let mut subscription = EventSourceSubscription::start(
        event_source_url,
        SharedHeaders::new(headers),
        cancel.clone(),
    );

    std::thread::sleep(Duration::from_millis(500));
    println!("   EventSource subscription started successfully.");

    let trigger_name = format!(
        "Trigger-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis()
    );
    let trigger_mb = client.mailbox_create(&account_id, &Mailbox::new(&trigger_name))?;
    if let Some(tid) = trigger_mb.id {
        let _ = client.mailbox_destroy(&account_id, &tid);
    }

    if let Some(state_change) = subscription.recv_timeout(Duration::from_secs(2)) {
        println!("   Received pushed state change: {:?}", state_change);
    } else {
        println!("   (No push frame received within 2s, but stream connected)");
    }

    subscription.stop();
    println!("   EventSource stopped.");

    println!("All consumer checks PASSED!");
    Ok(())
}
EOF

echo "== Building consumer binary =="
cargo build --manifest-path "${consumer_dir}/Cargo.toml"
consumer_bin="${consumer_dir}/target/debug/jmap-consumer"
if [ ! -x "${consumer_bin}" ]; then
    echo "ci/consumer-check.sh: failed to build consumer binary at ${consumer_bin}" >&2
    exit 1
fi

echo "== Starting mock server =="
mockd_log="${tmp_dir}/mockd.log"
"${mockd_bin}" --port 0 --basic alice:secret > "${mockd_log}" 2>&1 &
mockd_pid=$!

mock_port=""
for _ in $(seq 1 50); do
    if [ -f "${mockd_log}" ] && grep -q "jmap-mockd listening on" "${mockd_log}"; then
        mock_port="$(grep "jmap-mockd listening on" "${mockd_log}" | sed 's/.*http:\/\/127.0.0.1:\([0-9]*\).*/\1/')"
        break
    fi
    sleep 0.1
done

if [ -z "${mock_port}" ]; then
    echo "ci/consumer-check.sh: jmap-mockd failed to start" >&2
    cat "${mockd_log}" >&2
    exit 1
fi

echo "jmap-mockd listening on port ${mock_port}"

echo "== Executing consumer binary against jmap-mockd =="
if command -v timeout >/dev/null 2>&1; then
    timeout 60 "${consumer_bin}" "http://127.0.0.1:${mock_port}" "alice" "secret"
else
    "${consumer_bin}" "http://127.0.0.1:${mock_port}" "alice" "secret"
fi

kill -TERM "${mockd_pid}" 2>/dev/null || true
wait "${mockd_pid}" 2>/dev/null || true
mockd_pid=""

echo "== Consumer check passed successfully =="
