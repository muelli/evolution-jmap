// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Quota/get` against a live mock server — what `MailSync::quotas` hands to
//! `jmap-mail`'s `get_quota_info_sync`.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::quota::{Quota, quota_data_type, quota_resource_type, quota_scope};

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }
}

#[test]
fn every_seeded_quota_comes_back() {
    let fixture = Fixture::start();

    let quotas = fixture.sync().quotas().unwrap();

    // Every fresh account is seeded with one Mail/octets/account quota — see
    // `jmap-mock`'s `AccountState::new` — so the account-level list is not
    // empty from the moment it exists.
    assert_eq!(quotas.len(), 1);
    assert_eq!(quotas[0].resource_type, quota_resource_type::OCTETS);
    assert_eq!(quotas[0].scope, quota_scope::ACCOUNT);
    assert_eq!(quotas[0].data_types, vec![quota_data_type::MAIL.to_owned()]);
}

#[test]
fn a_second_seeded_quota_comes_back_too() {
    let fixture = Fixture::start();
    {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.account_id).unwrap();
        // `seed_with_id` rather than `seed`: the account's own construction
        // already seeded `Q1` through `seed_with_id`, which the `Store`'s
        // `next_id` counter knows nothing about, so `seed` would allocate
        // `Q1` again and silently overwrite it.
        account.quotas.seed_with_id(
            Id::from("Q2"),
            Quota::new(
                "Q2",
                "Message count",
                quota_resource_type::COUNT,
                0,
                10_000,
                quota_scope::ACCOUNT,
                [quota_data_type::MAIL],
            ),
        );
    }

    let quotas = fixture.sync().quotas().unwrap();

    assert_eq!(quotas.len(), 2);
    assert!(
        quotas
            .iter()
            .any(|quota| quota.resource_type == quota_resource_type::COUNT)
    );
}
