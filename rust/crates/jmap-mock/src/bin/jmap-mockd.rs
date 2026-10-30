// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Run a standalone mock JMAP server for manual tests with curl or Evolution.

use std::collections::BTreeMap;

use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarRights};
use jmap_proto::contacts::{AddressBookRights, ContactCard};
use jmap_proto::mail::{EmailAddress, EmailHeader, Mailbox, MailboxRights};
use serde::Deserialize;
use serde_json::json;

/// Return this `client_id` from the `--oauth2` registration endpoint.
const OAUTH2_CLIENT_ID: &str = "jmap-mockd-oauth2-client";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeedFile {
    #[serde(default)]
    accounts: Vec<SeedAccount>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeedAccount {
    id: String,
    name: String,
    #[serde(default = "default_true")]
    is_personal: bool,
    #[serde(default)]
    mailboxes: Vec<SeedMailbox>,
    #[serde(default)]
    emails: Vec<SeedEmail>,
    #[serde(default)]
    cards: Vec<ContactCard>,
    #[serde(default)]
    events: Vec<CalendarEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeedMailbox {
    id: String,
    name: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    is_subscribed: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeedEmail {
    mailbox_id: String,
    from: SeedAddress,
    subject: String,
    text_body: String,
    received_at: String,
    #[serde(default)]
    to: Vec<SeedAddress>,
    #[serde(default)]
    cc: Vec<SeedAddress>,
    #[serde(default)]
    bcc: Vec<SeedAddress>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    headers: Vec<SeedHeader>,
}

#[derive(Debug, Deserialize)]
struct SeedAddress {
    #[serde(default)]
    name: Option<String>,
    email: String,
}

#[derive(Debug, Deserialize)]
struct SeedHeader {
    name: String,
    value: String,
}

impl SeedAddress {
    fn to_email_address(&self) -> EmailAddress {
        EmailAddress::new(self.name.as_deref(), &self.email)
    }
}

fn default_true() -> bool {
    true
}

fn main() {
    let mut port: u16 = 8080;
    let mut oauth2 = false;
    let mut seed_spoofed_shared_account = false;
    let mut seed_shared_account = false;
    let mut seed_path: Option<String> = None;
    let mut builder = MockServer::builder();

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--port" => {
                let value = next_argument(&mut arguments, "--port needs a number");
                port = parse_u16(value, "--port needs a number");
            }
            "--basic" => {
                let value = next_argument(&mut arguments, "--basic needs user:pass");
                let (user, password) = value
                    .split_once(':')
                    .unwrap_or_else(|| usage("--basic needs user:pass"));
                builder = builder.basic_auth(user, password);
            }
            "--bearer" => {
                let token = next_argument(&mut arguments, "--bearer needs a token");
                builder = builder.bearer_token(&token);
            }
            "--bearer-as" => {
                let value = next_argument(&mut arguments, "--bearer-as needs token:principalId");
                let (token, principal) = value
                    .split_once(':')
                    .unwrap_or_else(|| usage("--bearer-as needs token:principalId"));
                builder = builder.bearer_token_as(token, Id::new(principal));
            }
            "--without-capability" => {
                let capability = next_argument(
                    &mut arguments,
                    "--without-capability needs a capability URN",
                );
                builder = builder.without_capability(&capability);
            }
            "--no-sieve" => {
                builder = builder.no_sieve();
            }
            "--without-primary-accounts" => {
                builder = builder.without_primary_accounts();
            }
            "--calls-in-request" => {
                let value = next_argument(&mut arguments, "--calls-in-request needs a number");
                builder =
                    builder.calls_in_request(parse_u64(value, "--calls-in-request needs a number"));
            }
            "--no-calls-in-request" => {
                builder = builder.no_calls_in_request();
            }
            "--oauth2" => oauth2 = true,
            "--changes-page-size" => {
                let value = next_argument(&mut arguments, "--changes-page-size needs a number");
                builder = builder
                    .changes_page_size(parse_u64(value, "--changes-page-size needs a number"));
            }
            "--changes-never-terminates" => {
                builder = builder.changes_never_terminates();
            }
            "--stalwart-shaped-unrecognized-changes-state" => {
                builder = builder.stalwart_shaped_unrecognized_changes_state();
            }
            "--objects-in-get" => {
                let value = next_argument(&mut arguments, "--objects-in-get needs a number");
                builder =
                    builder.objects_in_get(parse_u64(value, "--objects-in-get needs a number"));
            }
            "--query-page-size" => {
                let value = next_argument(&mut arguments, "--query-page-size needs a number");
                builder =
                    builder.query_page_size(parse_u64(value, "--query-page-size needs a number"));
            }
            "--query-never-terminates" => {
                builder = builder.query_never_terminates();
            }
            "--query-hides-limit" => {
                builder = builder.query_hides_limit();
            }
            "--size-request" => {
                let value = next_argument(&mut arguments, "--size-request needs a number");
                builder = builder.size_request(parse_u64(value, "--size-request needs a number"));
            }
            "--no-size-request" => {
                builder = builder.no_size_request();
            }
            "--size-upload" => {
                let value = next_argument(&mut arguments, "--size-upload needs a number");
                builder = builder.size_upload(parse_u64(value, "--size-upload needs a number"));
            }
            "--no-size-upload" => {
                builder = builder.no_size_upload();
            }
            "--sieve-extensions" => {
                let value = next_argument(
                    &mut arguments,
                    "--sieve-extensions needs a comma-separated list",
                );
                builder = builder.sieve_extensions(parse_csv_list(&value));
            }
            "--email-get-includes-unrequested" => {
                builder = builder.email_get_includes_unrequested();
            }
            "--contact-photo-uses-file-uri" => {
                builder = builder.contact_photo_uses_file_uri();
            }
            "--contact-photo-data-uri-as-blob-id" => {
                builder = builder.contact_photo_data_uri_as_blob_id();
            }
            "--calendar-event-link-uses-file-uri" => {
                builder = builder.calendar_event_link_uses_file_uri();
            }
            "--session-via-redirect" => {
                builder = builder.session_via_redirect();
            }
            "--well-known-redirect-to" => {
                let origin = next_argument(
                    &mut arguments,
                    "--well-known-redirect-to needs an origin URL",
                );
                builder = builder.well_known_redirect_to(origin);
            }
            "--advertise-origin" => {
                let origin =
                    next_argument(&mut arguments, "--advertise-origin needs an origin URL");
                builder = builder.advertise_origin(origin);
            }
            "--shared-account-name-copies-user" => {
                builder = builder.shared_account_name_copies_user();
                seed_spoofed_shared_account = true;
            }
            "--share" => {
                seed_shared_account = true;
            }
            "--download-via-redirect-to" => {
                let origin = next_argument(
                    &mut arguments,
                    "--download-via-redirect-to needs an origin URL",
                );
                builder = builder.download_via_redirect_to(origin);
            }
            "--reject-download-accept-json" => {
                builder = builder.reject_download_accept_json();
            }
            "--terse-submission-create" => {
                builder = builder.terse_submission_create();
            }
            "--terse-contact-create" => {
                builder = builder.terse_contact_create();
            }
            "--max-contact-size" => {
                let value = next_argument(&mut arguments, "--max-contact-size needs a number");
                builder =
                    builder.max_contact_size(parse_u64(value, "--max-contact-size needs a number"));
            }
            "--new-collections-default-unsubscribed" => {
                builder = builder.new_collections_default_unsubscribed();
            }
            "--terse-calendar-event-create" => {
                builder = builder.terse_calendar_event_create();
            }
            "--without-scheduling" => {
                builder = builder.without_scheduling();
            }
            "--terse-collection-create" => {
                builder = builder.terse_collection_create();
            }
            "--id-omitting-collection-create" => {
                builder = builder.id_omitting_collection_create();
            }
            "--reject-collection-create" => {
                builder = builder.reject_collection_create();
            }
            "--max-delayed-send" => {
                let value = next_argument(&mut arguments, "--max-delayed-send needs a number");
                builder =
                    builder.max_delayed_send(parse_u64(value, "--max-delayed-send needs a number"));
            }
            "--snooze-extension" => {
                builder = builder.snooze_extension();
            }
            "--sieve-vacation-script" => {
                builder = builder.sieve_vacation_script();
            }
            "--tasks" => {
                builder = builder.tasks();
            }
            "--seed" => {
                let path = next_argument(&mut arguments, "--seed needs a JSON file path");
                seed_path = Some(path);
            }
            "--help" | "-h" => usage(""),
            other => usage(&format!("unknown argument: {other}")),
        }
    }

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
            .oauth_client_registration(|_request| (201, json!({"client_id": OAUTH2_CLIENT_ID})))
            .oauth_token(|fields| {
                let grant_type = fields.get("grant_type").map(String::as_str);
                if grant_type != Some("authorization_code") {
                    return (400, json!({"error": "unsupported_grant_type"}));
                }
                (
                    200,
                    json!({
                        "access_token": "mock-access-token",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                    }),
                )
            });
    }

    let server = builder.port(port).start();

    match seed_path {
        Some(path) => seed_from_file(&server, &path).unwrap_or_else(|error| usage(&error)),
        None => seed_default_data(&server, seed_spoofed_shared_account, seed_shared_account),
    }

    println!("jmap-mockd listening on {}", server.origin());
    println!(
        "listening: {}",
        json!({
            "origin": server.origin(),
            "session": format!("{}/.well-known/jmap", server.origin()),
        })
    );
    println!("session: {}/.well-known/jmap", server.origin());
    println!("account: A1 (alice@example.com), Ctrl-C to stop");

    loop {
        std::thread::park();
    }
}

fn parse_u16(value: String, usage_message: &str) -> u16 {
    value.parse().unwrap_or_else(|_| usage(usage_message))
}

fn parse_u64(value: String, usage_message: &str) -> u64 {
    value.parse().unwrap_or_else(|_| usage(usage_message))
}

fn next_argument(arguments: &mut impl Iterator<Item = String>, usage_message: &str) -> String {
    arguments.next().unwrap_or_else(|| usage(usage_message))
}

fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn seed_default_data(
    server: &MockServer,
    seed_spoofed_shared_account: bool,
    seed_shared_account: bool,
) {
    let state = server.state();
    let mut state = state.lock().expect("state lock");
    if seed_spoofed_shared_account && !seed_shared_account {
        let shared = state.add_account("A2", "sharer@example.test");
        shared.is_personal = false;
    }
    if seed_shared_account {
        seed_shared_account_data(&mut state);
    }
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

fn seed_shared_account_data(state: &mut jmap_mock::ServerState) {
    let grantee_principal_id = Id::from("P-grantee");
    let owner_principal_id = Id::from("P-owner");

    if let Some(account) = state.account_mut(&Id::from("A1")) {
        account.current_user_principal_id = Some(grantee_principal_id.clone());
    }

    let shared = state.add_account("A2", "owner@example.test");
    shared.is_personal = false;
    shared.current_user_principal_id = Some(owner_principal_id);

    let shared_mailbox_id =
        shared.seed_mailbox("Shared Inbox", Some(jmap_proto::mail::role::INBOX));
    let _private_mailbox_id = shared.seed_mailbox("Private", None);
    let shared_book_id = shared.seed_address_book("Shared", false);
    let _private_book_id = shared.seed_address_book("Private", false);
    let shared_calendar_id = shared.seed_calendar("Shared", false);
    let _private_calendar_id = shared.seed_calendar("Private", false);

    if let Some(mailbox) = shared.mailboxes.get_mut(&shared_mailbox_id) {
        mailbox.share_with = Some(BTreeMap::from([(
            grantee_principal_id.clone(),
            MailboxRights::read_only(),
        )]));
    }
    if let Some(book) = shared.address_books.get_mut(&shared_book_id) {
        book.share_with = Some(BTreeMap::from([(
            grantee_principal_id.clone(),
            AddressBookRights::read_only(),
        )]));
    }
    if let Some(calendar) = shared.calendars.get_mut(&shared_calendar_id) {
        calendar.share_with = Some(BTreeMap::from([(
            grantee_principal_id,
            CalendarRights::read_only(),
        )]));
    }
}

fn seed_from_file(server: &MockServer, path: &str) -> Result<(), String> {
    let raw =
        std::fs::read(path).map_err(|error| format!("cannot read seed file {path}: {error}"))?;
    let seed: SeedFile = serde_json::from_slice(&raw)
        .map_err(|error| format!("cannot parse seed file {path}: {error}"))?;

    if seed.accounts.is_empty() {
        return Err("seed file needs at least one account".to_owned());
    }

    let state = server.state();
    let mut state = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.accounts.clear();

    for account_seed in seed.accounts {
        let account_id = Id::new(account_seed.id.clone());
        let account = state.add_account(account_id, account_seed.name);
        account.is_personal = account_seed.is_personal;

        let mut mailbox_ids = BTreeMap::new();
        for mailbox_seed in account_seed.mailboxes {
            let mailbox_id = Id::new(mailbox_seed.id.clone());
            let mailbox = {
                let mut mailbox = Mailbox::default();
                mailbox.id = Some(mailbox_id.clone());
                mailbox.name = mailbox_seed.name;
                mailbox.parent_id = mailbox_seed.parent_id.map(Id::new);
                mailbox.role = mailbox_seed.role;
                mailbox.sort_order = Some(0);
                mailbox.is_subscribed = Some(mailbox_seed.is_subscribed.unwrap_or(true));
                mailbox
            };
            account.mailboxes.seed_with_id(mailbox_id.clone(), mailbox);
            mailbox_ids.insert(mailbox_seed.id, mailbox_id);
        }

        for email_seed in account_seed.emails {
            let mailbox_id = mailbox_ids
                .get(&email_seed.mailbox_id)
                .cloned()
                .unwrap_or_else(|| Id::new(email_seed.mailbox_id.clone()));
            if !account.mailboxes.contains(&mailbox_id) {
                return Err(format!(
                    "seed email references unknown mailbox id {}",
                    email_seed.mailbox_id
                ));
            }

            let from_name = email_seed
                .from
                .name
                .as_deref()
                .unwrap_or(&email_seed.from.email);
            let mut seed = EmailSeed::new(
                mailbox_id,
                (from_name, &email_seed.from.email),
                &email_seed.subject,
                &email_seed.text_body,
                &email_seed.received_at,
            );
            if !email_seed.to.is_empty() {
                seed.to = email_seed
                    .to
                    .iter()
                    .map(SeedAddress::to_email_address)
                    .collect();
            }
            seed.cc = email_seed
                .cc
                .iter()
                .map(SeedAddress::to_email_address)
                .collect();
            seed.bcc = email_seed
                .bcc
                .iter()
                .map(SeedAddress::to_email_address)
                .collect();
            seed.keywords = email_seed.keywords;
            seed.headers = email_seed
                .headers
                .into_iter()
                .map(|header| EmailHeader::new(header.name, header.value))
                .collect();
            account.seed_email(seed);
        }

        for mut card in account_seed.cards {
            let card_id = card
                .id
                .clone()
                .unwrap_or_else(|| account.contact_cards.alloc_id());
            card.id = Some(card_id.clone());
            account.contact_cards.seed_with_id(card_id, card);
        }

        for mut event in account_seed.events {
            let event_id = event
                .id
                .clone()
                .unwrap_or_else(|| account.calendar_events.alloc_id());
            event.id = Some(event_id.clone());
            account.calendar_events.seed_with_id(event_id, event);
        }
    }

    Ok(())
}

fn usage(error: &str) -> ! {
    if !error.is_empty() {
        eprintln!("error: {error}\n");
    }
    eprintln!(
        "usage: jmap-mockd [--port N] [--basic user:pass] [--bearer TOKEN] [--bearer-as TOKEN:PRINCIPAL] [--oauth2] \
         [--without-capability URN] [--no-sieve] [--without-primary-accounts] [--calls-in-request N] [--no-calls-in-request] \
         [--changes-page-size N] [--changes-never-terminates] [--stalwart-shaped-unrecognized-changes-state] \
         [--objects-in-get N] [--query-page-size N] [--query-never-terminates] [--query-hides-limit] \
         [--size-request N] [--no-size-request] [--size-upload N] [--no-size-upload] [--sieve-extensions A,B,C] \
         [--email-get-includes-unrequested] [--contact-photo-uses-file-uri] [--contact-photo-data-uri-as-blob-id] [--calendar-event-link-uses-file-uri] \
         [--session-via-redirect] [--well-known-redirect-to ORIGIN] [--advertise-origin ORIGIN] \
         [--shared-account-name-copies-user] [--download-via-redirect-to ORIGIN] [--reject-download-accept-json] \
         [--terse-submission-create] [--terse-contact-create] [--max-contact-size N] \
         [--new-collections-default-unsubscribed] [--terse-calendar-event-create] [--without-scheduling] \
         [--terse-collection-create] [--id-omitting-collection-create] [--reject-collection-create] \
         [--max-delayed-send N] [--snooze-extension] [--sieve-vacation-script] [--tasks] [--share] [--seed PATH]"
    );
    std::process::exit(if error.is_empty() { 0 } else { 2 });
}
