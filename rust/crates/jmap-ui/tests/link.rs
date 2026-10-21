// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests of `AccountLink::call` on a link with no token to refresh.

use std::cell::Cell;

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_ui::link::AccountLink;
use jmap_ui::session_cache::AccountFeatures;

#[test]
fn a_401_on_a_link_with_no_token_to_refresh_is_not_sent_again() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let features = AccountFeatures::from_session(client.session()).expect("a mail account");
    let link = AccountLink::without_refresh(client, features);

    let attempts = Cell::new(0);
    let answer: Result<(), Error> = link.call(|_| {
        attempts.set(attempts.get() + 1);
        Err(Error::Http {
            status: 401,
            problem: None,
        })
    });

    assert!(matches!(answer, Err(Error::Http { status: 401, .. })));
    assert_eq!(
        attempts.get(),
        1,
        "a password or API token cannot be refreshed"
    );
}
