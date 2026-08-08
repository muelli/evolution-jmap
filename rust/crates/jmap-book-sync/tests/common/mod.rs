// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A mock server holding two address books, so that "only this book" is
//! something the tests can actually observe.

// Each test binary compiles this module separately and uses a subset of it.
#![allow(dead_code)]

use jmap_book_sync::BookSync;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::ContactCard;
use serde_json::Value;

pub struct Fixture {
    pub server: MockServer,
    pub account_id: Id,
    pub ours: Id,
    pub theirs: Id,
}

impl Fixture {
    pub fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let (ours, theirs) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            (
                account.seed_address_book("Personal", true),
                account.seed_address_book("Shared", false),
            )
        };
        Self {
            server,
            account_id,
            ours,
            theirs,
        }
    }

    pub fn client(&self) -> Client {
        Client::connect(self.server.origin(), Credentials::none()).unwrap()
    }

    /// A [`BookSync`] over the "Personal" book.
    pub fn sync(&self) -> BookSync {
        BookSync::new(self.client(), self.account_id.clone(), self.ours.clone())
    }

    /// Create a card directly, bypassing the code under test.
    pub fn seed(&self, book: &Id, full_name: &str, email: &str) -> Id {
        self.client()
            .contact_create(
                &self.account_id,
                &ContactCard::simple(book.clone(), full_name, email),
            )
            .unwrap()
            .id
            .expect("server assigned id")
    }

    /// Patch a card directly, bypassing the code under test.
    pub fn patch(&self, id: &Id, patch: Value) {
        self.client()
            .contact_update(&self.account_id, id, patch)
            .unwrap();
    }

    pub fn card(&self, id: &Id) -> ContactCard {
        self.client()
            .contact_get(&self.account_id, std::slice::from_ref(id))
            .unwrap()
            .list
            .into_iter()
            .next()
            .expect("card exists")
    }
}
