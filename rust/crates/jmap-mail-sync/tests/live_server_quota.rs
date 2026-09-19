// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::quotas` against a real JMAP server — the one `Quota/get` call
//! `jmap-mail`'s `get_quota_info_sync` reads, exercised end to end for the
//! first time.
//!
//! `jmap-mail-sync/tests/quota.rs` and `jmap-mail/tests/quota.rs` both only
//! ever exercise `jmap_mock::MockServer`, which necessarily answers exactly
//! the `Quota` shape its own fixtures were given. A real server's actual RFC
//! 9425 implementation had never been checked: whether it advertises the
//! capability at all, and whether whatever it sends back for `resourceType`,
//! `scope` and `types` actually deserializes into `jmap_proto::quota::Quota`.
//!
//! It does, but the check found a real bug, now fixed in the same commit:
//! `quota_data_type::MAIL` held `"Mail"`, but RFC 9425 §4.1's `types` values
//! come from the JMAP Types Names registry, not a quota-specific enum — a
//! real Stalwart test account with a disk quota configured (`stw apply` with
//! an Account's `quotas.maxDiskQuota`) answered `Quota/get` with `types:
//! ["Email", "SieveScript", "FileNode", "CalendarEvent", "ContactCard"]`,
//! never `"Mail"`. So `jmap-mail`'s `applies_to_mail` could only ever have
//! matched through its "absent or empty `types`" branch, never through the
//! `"Mail"` check itself, on any real server. Fixed to `"Email"`; see
//! `jmap-mail/src/quota.rs`'s new regression test, which pins the exact
//! multi-type list this probe observed.
//!
//! A fresh account with no `quotas` ever configured answers `Quota/get` with
//! an empty `list` even though the server still advertises the capability
//! (confirmed the same way) — RFC 9425 has no "every account has a quota"
//! guarantee, so this test tolerates either shape rather than assuming the
//! write-test account has one configured.
//!
//! ## Running it
//!
//! Same environment as the other `jmap-mail-sync` live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_quota -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like the other files,
//! this crate has no such feature, and `#[ignore]` alone already keeps it
//! out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_proto::quota::{quota_resource_type, quota_scope};
use jmap_proto::session::CAPABILITY_MAIL;

/// Mirrors `jmap-mail-sync/tests/live_server.rs::connect_for_write` exactly.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn quotas_from_the_real_server_deserialize_and_classify_cleanly() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let sync = MailSync::new(client, account_id);

    let quotas = sync
        .quotas()
        .expect("Quota/get failed against the real server");

    for quota in &quotas {
        assert!(
            matches!(
                quota.resource_type.as_str(),
                quota_resource_type::OCTETS | quota_resource_type::COUNT
            ),
            "unrecognized resourceType {:?} in {quota:?}",
            quota.resource_type
        );
        assert!(
            matches!(
                quota.scope.as_str(),
                quota_scope::ACCOUNT | quota_scope::DOMAIN | quota_scope::GLOBAL
            ),
            "unrecognized scope {:?} in {quota:?}",
            quota.scope
        );
        assert!(
            quota.hard_limit >= quota.used,
            "used ({}) exceeds hardLimit ({}) in {quota:?}",
            quota.used,
            quota.hard_limit
        );
    }

    eprintln!(
        "real server returned {} quota object(s): {quotas:?}",
        quotas.len()
    );
}
