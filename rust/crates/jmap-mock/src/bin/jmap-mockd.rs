// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standalone mock JMAP server for manual testing (curl, Evolution).
//!
//! ```text
//! jmap-mockd [--port N] [--basic user:pass] [--bearer TOKEN] [--oauth2]
//! ```
//!
//! Serves one account (`A1`, alice@example.com) pre-seeded with an inbox
//! containing two messages, a sending identity, an address book, and a
//! calendar.

use jmap_mock::{EmailSeed, MockServer};
use serde_json::json;

/// The fixed `client_id` `--oauth2`'s registration endpoint hands back —
/// this binary has no accounts to distinguish clients by, so one constant is
/// as good as a generated one and is easier to grep for in a log.
const OAUTH2_CLIENT_ID: &str = "jmap-mockd-oauth2-client";

fn main() {
    let mut port: u16 = 8080;
    let mut oauth2 = false;
    let mut builder = MockServer::builder();

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--port" => {
                port = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| usage("--port needs a number"));
            }
            "--basic" => {
                let value = arguments
                    .next()
                    .unwrap_or_else(|| usage("--basic needs user:pass"));
                let (user, password) = value
                    .split_once(':')
                    .unwrap_or_else(|| usage("--basic needs user:pass"));
                builder = builder.basic_auth(user, password);
            }
            "--bearer" => {
                let token = arguments
                    .next()
                    .unwrap_or_else(|| usage("--bearer needs a token"));
                builder = builder.bearer_token(&token);
            }
            "--oauth2" => oauth2 = true,
            "--help" | "-h" => usage(""),
            other => usage(&format!("unknown argument: {other}")),
        }
    }

    // Off by default, matching every other opt-in behaviour here: a mock
    // that always advertised OAuth 2.0 would leave a manual test with no way
    // to check the "this deployment offers none" path. RFC 8414's document
    // and RFC 7591 registration are both real network round trips a client
    // can drive end to end against this one process — see
    // `jmap_config::oauth2_setup::discover_and_register`, which this mirrors
    // for a standalone run rather than an in-process `MockServer`.
    if oauth2 {
        builder = builder
            .oauth_authorization_server(|origin| {
                json!({
                    "issuer": origin,
                    "authorization_endpoint": format!("{origin}/oauth/authorize"),
                    "token_endpoint": format!("{origin}/oauth/token"),
                    "registration_endpoint": format!("{origin}/oauth/register"),
                    "response_types_supported": ["code"],
                    "grant_types_supported": ["authorization_code", "refresh_token"],
                    "code_challenge_methods_supported": ["S256"],
                })
            })
            .oauth_client_registration(|_request| (201, json!({"client_id": OAUTH2_CLIENT_ID})));
    }

    let server = builder.port(port).start();

    // Demo data, so the server is interesting straight away.
    {
        let state = server.state();
        let mut state = state.lock().expect("state lock");
        let account = state
            .account_mut(&server.account_id())
            .expect("default account exists");
        account.seed_identity("Alice", "alice@example.com");
        account.seed_address_book("Personal", true);
        account.seed_calendar("Personal", true);
        let inbox = account.seed_mailbox("Inbox", Some(jmap_proto::mail::role::INBOX));
        account.seed_mailbox("Sent", Some(jmap_proto::mail::role::SENT));
        account.seed_mailbox("Drafts", Some(jmap_proto::mail::role::DRAFTS));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Welcome to jmap-mockd",
            "This message was seeded at startup.",
            "2026-08-01T10:00:00Z",
        ));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Carol", "carol@example.com"),
            "Second message",
            "Another seeded message.",
            "2026-08-02T09:30:00Z",
        ));
    }

    println!("jmap-mockd listening on {}", server.origin());
    println!("session: {}/.well-known/jmap", server.origin());
    println!("account: A1 (alice@example.com) — Ctrl-C to stop");

    // Serve until killed; the server thread does the work.
    loop {
        std::thread::park();
    }
}

fn usage(error: &str) -> ! {
    if !error.is_empty() {
        eprintln!("error: {error}\n");
    }
    eprintln!("usage: jmap-mockd [--port N] [--basic user:pass] [--bearer TOKEN] [--oauth2]");
    std::process::exit(if error.is_empty() { 0 } else { 2 });
}
