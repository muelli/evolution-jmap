// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::quotas` against a real JMAP server. This is the one
//! `Quota/get` call that `get_quota_info_sync` in `jmap-mail` reads.
//!
//! `jmap-mail-sync/tests/quota.rs` and `jmap-mail/tests/quota.rs` use only
//! `jmap_mock::MockServer`, which returns the `Quota` shape of its fixtures.
//! This test checks that the `Quota` objects of a real RFC 9425 server
//! deserialize into `jmap_proto::quota::Quota`, and that their `resourceType`
//! and `scope` values are known values.
//!
//! RFC 9425 §4.1 takes the `types` values from the JMAP Types Names registry.
//! A Stalwart account with a disk quota (`stw apply` with
//! `quotas.maxDiskQuota`) returned `types: ["Email", "SieveScript",
//! "FileNode", "CalendarEvent", "ContactCard"]`. Thus
//! `quota_data_type::MAIL` is `"Email"`, not `"Mail"`. A regression test in
//! `jmap-mail/src/quota.rs` holds this list.
//!
//! An account with no configured quota returns an empty `list`, but the
//! server still advertises the capability. RFC 9425 does not require a quota
//! for each account, thus this test accepts an empty list.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER` and `JMAP_LIVE_SERVER_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_quota -- --ignored
//! ```
//!
//! This crate has no `live-server` feature. `#[ignore]` keeps the test out of
//! a plain `cargo test`.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. It panics when
//! that variable is set but `JMAP_LIVE_SERVER_WRITE_PASSWORD` or
//! `JMAP_LIVE_SERVER_URL` is unset.

use jmap_mail_sync::MailSync;
use jmap_proto::quota::{quota_resource_type, quota_scope};
use jmap_proto::session::CAPABILITY_MAIL;
use jmap_test_support::connect_for_write;

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
